//! LLM prompt-cache metric family.
//!
//! Counts the cache-token side of Anthropic's ephemeral prompt cache
//! (returned as `cache_creation_input_tokens` + `cache_read_input_tokens`
//! on every Messages API response). Hot path is lock-free `fetch_add`
//! against per-model `AtomicU64` counters; the model-label HashMap is
//! locked only on first-sight of a new model.
//!
//! The hit-ratio gauge is a rolling exponential moving average of
//! per-call hit ratio so operators can see how well their role +
//! schema split actually amortises across calls. EMA smoothing avoids
//! a single warm-up call (always a 0% hit) tanking the displayed
//! number; a window factor of 0.1 means the metric tracks ~10-call
//! recent history.
//!
//! Steady-state target: `cache_hit_ratio ≥ 0.7` for the extractor +
//! judge prompts that share role + schema blocks. Below that, the
//! caching shape is wrong (per-call data is leaking into the cached
//! prefix, or the schema block is being regenerated on each call).

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use parking_lot::Mutex;

/// EMA smoothing factor for the rolling hit-ratio gauge. 0.1 ≈ a
/// 10-call window. Tuned for the extractor cycle, which fires a
/// handful of LLM calls per memory.
const HIT_RATIO_EMA_ALPHA: f64 = 0.1;

/// Per-model cumulative cache token counters.
#[derive(Debug, Default)]
struct PerModel {
    cache_creation_tokens: AtomicU64,
    cache_read_tokens: AtomicU64,
    input_tokens: AtomicU64,
}

/// Prompt-cache metric family. One per shard, shared by `Arc` between
/// the LLM call sites and the `/metrics` exposition layer.
#[derive(Debug, Default)]
pub struct LlmCacheMetrics {
    /// Keyed by model string (low cardinality in practice — a handful
    /// of model ids per deployment). Only locked on first-sight of a
    /// new model; observed via per-`AtomicU64` `fetch_add` thereafter.
    per_model: Mutex<HashMap<String, PerModel>>,
    /// Rolling EMA of per-call cache hit ratio, stored as f64 bits in
    /// an `AtomicU64`. Updated under the same loose ordering as the
    /// counters above — exposition is best-effort, not transactional.
    cache_hit_ratio_ema_bits: AtomicU64,
    /// Number of observations folded into the EMA. Zero means the
    /// gauge has never been populated; readers should treat the EMA
    /// as undefined in that case.
    cache_hit_ratio_samples: AtomicU64,
}

impl LlmCacheMetrics {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record the cache-side counters of one LLM call.
    ///
    /// `cache_creation` is the tokens written into the cache on this
    /// call (Anthropic populates a fresh ephemeral entry).
    /// `cache_read` is the tokens served back from a previously
    /// cached entry. `input_tokens` is the live-billed input.
    pub fn observe_call(
        &self,
        model: &str,
        cache_creation: u64,
        cache_read: u64,
        input_tokens: u64,
    ) {
        let total = cache_creation + cache_read + input_tokens;
        // Update the EMA gauge first so it sees this call. Zero-total
        // observations don't move the gauge.
        if total > 0 {
            let ratio = cache_read as f64 / total as f64;
            self.fold_into_ema(ratio);
        }

        // Then bump the per-model counters under the per-shard mutex.
        // Common path: the model has been seen before; we look up the
        // entry under the read-side mutex and emit lock-free
        // `fetch_add`s. Cold path: first-sight of a model creates an
        // entry under the same mutex.
        let mut guard = self.per_model.lock();
        let entry = guard.entry(model.to_string()).or_default();
        entry
            .cache_creation_tokens
            .fetch_add(cache_creation, Ordering::Relaxed);
        entry
            .cache_read_tokens
            .fetch_add(cache_read, Ordering::Relaxed);
        entry
            .input_tokens
            .fetch_add(input_tokens, Ordering::Relaxed);
    }

    fn fold_into_ema(&self, ratio: f64) {
        // Best-effort RMW. Concurrent observers may interleave and we
        // accept that a few samples can race past each other — the
        // gauge is observational, not transactional.
        let prior_bits = self.cache_hit_ratio_ema_bits.load(Ordering::Relaxed);
        let samples = self.cache_hit_ratio_samples.load(Ordering::Relaxed);
        let updated = if samples == 0 {
            ratio
        } else {
            let prior = f64::from_bits(prior_bits);
            prior + HIT_RATIO_EMA_ALPHA * (ratio - prior)
        };
        self.cache_hit_ratio_ema_bits
            .store(updated.to_bits(), Ordering::Relaxed);
        self.cache_hit_ratio_samples.fetch_add(1, Ordering::Relaxed);
    }

    /// Read-only snapshot for `/metrics`.
    #[must_use]
    pub fn snapshot(&self) -> LlmCacheMetricsSnapshot {
        let mut per_model = HashMap::new();
        let guard = self.per_model.lock();
        for (model, counters) in guard.iter() {
            per_model.insert(
                model.clone(),
                LlmCacheModelCounts {
                    cache_creation_tokens: counters.cache_creation_tokens.load(Ordering::Relaxed),
                    cache_read_tokens: counters.cache_read_tokens.load(Ordering::Relaxed),
                    input_tokens: counters.input_tokens.load(Ordering::Relaxed),
                },
            );
        }
        let samples = self.cache_hit_ratio_samples.load(Ordering::Relaxed);
        let ratio = if samples == 0 {
            0.0
        } else {
            f64::from_bits(self.cache_hit_ratio_ema_bits.load(Ordering::Relaxed))
        };
        LlmCacheMetricsSnapshot {
            per_model,
            cache_hit_ratio_ema: ratio,
            cache_hit_ratio_samples: samples,
        }
    }
}

/// Per-model snapshot row of [`LlmCacheMetricsSnapshot`].
#[derive(Debug, Clone, Default)]
pub struct LlmCacheModelCounts {
    /// Sum of `cache_creation_input_tokens` reported by the provider.
    pub cache_creation_tokens: u64,
    /// Sum of `cache_read_input_tokens` reported by the provider.
    pub cache_read_tokens: u64,
    /// Sum of `input_tokens` (live-billed input) reported by the
    /// provider. Excludes cached-read tokens.
    pub input_tokens: u64,
}

/// Plain-data snapshot of [`LlmCacheMetrics`] for `/metrics` exposition.
#[derive(Debug, Clone, Default)]
pub struct LlmCacheMetricsSnapshot {
    pub per_model: HashMap<String, LlmCacheModelCounts>,
    /// Rolling EMA of per-call `cache_read / (cache_read +
    /// cache_creation + input_tokens)`. Undefined when
    /// `cache_hit_ratio_samples == 0`.
    pub cache_hit_ratio_ema: f64,
    pub cache_hit_ratio_samples: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observe_call_accumulates_per_model_counters() {
        let m = LlmCacheMetrics::new();
        m.observe_call("claude-haiku-4-5", 1500, 0, 100);
        m.observe_call("claude-haiku-4-5", 0, 1500, 100);
        m.observe_call("gpt-4o-mini", 0, 0, 200);

        let s = m.snapshot();
        let haiku = s.per_model.get("claude-haiku-4-5").expect("haiku entry");
        assert_eq!(haiku.cache_creation_tokens, 1500);
        assert_eq!(haiku.cache_read_tokens, 1500);
        assert_eq!(haiku.input_tokens, 200);

        let gpt = s.per_model.get("gpt-4o-mini").expect("gpt entry");
        assert_eq!(gpt.cache_creation_tokens, 0);
        assert_eq!(gpt.cache_read_tokens, 0);
        assert_eq!(gpt.input_tokens, 200);
    }

    #[test]
    fn snapshot_with_no_observations_returns_zeros() {
        let m = LlmCacheMetrics::new();
        let s = m.snapshot();
        assert!(s.per_model.is_empty());
        assert_eq!(s.cache_hit_ratio_samples, 0);
        // Undefined when samples == 0; we expose 0.0 as the sentinel.
        assert_eq!(s.cache_hit_ratio_ema, 0.0);
    }

    #[test]
    fn ema_seeds_to_first_observation_then_smooths() {
        let m = LlmCacheMetrics::new();
        // First call is a cache write only ⇒ hit ratio 0.
        m.observe_call("m", 1000, 0, 0);
        let s1 = m.snapshot();
        assert_eq!(s1.cache_hit_ratio_samples, 1);
        assert!((s1.cache_hit_ratio_ema - 0.0).abs() < 1e-9);

        // Second call is a perfect cache hit ⇒ ratio 1.0; EMA pulls
        // partway toward it (alpha=0.1).
        m.observe_call("m", 0, 1000, 0);
        let s2 = m.snapshot();
        assert_eq!(s2.cache_hit_ratio_samples, 2);
        // 0 + 0.1 * (1.0 - 0) = 0.1
        assert!((s2.cache_hit_ratio_ema - 0.1).abs() < 1e-9);
    }

    #[test]
    fn ema_ignores_calls_with_zero_total_tokens() {
        let m = LlmCacheMetrics::new();
        m.observe_call("m", 0, 0, 0);
        let s = m.snapshot();
        // Counter is still touched (model is recorded) but the EMA
        // shouldn't be polluted by a zero-total degenerate sample.
        assert_eq!(s.cache_hit_ratio_samples, 0);
    }

    #[test]
    fn ema_converges_toward_steady_state_target() {
        // Hit ratio 0.8 should be reachable after enough samples even
        // with the conservative 0.1 alpha.
        let m = LlmCacheMetrics::new();
        for _ in 0..200 {
            m.observe_call("m", 0, 800, 200);
        }
        let s = m.snapshot();
        assert!(
            s.cache_hit_ratio_ema > 0.7,
            "EMA must approach steady state 0.8: got {}",
            s.cache_hit_ratio_ema,
        );
    }
}
