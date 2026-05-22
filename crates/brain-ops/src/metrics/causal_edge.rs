//! `CausalEdgeWorker` metric family.

use std::sync::atomic::{AtomicU64, Ordering};

use super::histograms::{WorkerHistogram, WorkerHistogramSnapshot, DEFAULT_CYCLE_BUCKETS_SECONDS};

/// Why the CausalEdgeWorker skipped writing a `Caused` edge for an
/// enqueued statement. Operators read these to triage "why are no
/// causal edges landing?" without trawling logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CausalSkipReason {
    /// The statement's predicate isn't in the resolved causal whitelist.
    /// This is the dominant outcome when no schema is declared, or when
    /// the declared schema doesn't include any causal predicates.
    NonCausalPredicate = 0,
    /// Statement confidence is below the worker's floor.
    LowConfidence = 1,
    /// Statement has no evidence memories to anchor the effect side.
    NoEvidence = 2,
    /// Statement object isn't an entity or memory ref — `Value(_)` and
    /// `Statement(_)` variants don't produce a memory→memory edge in v1.
    ObjectNotEntity = 3,
    /// No related statement on the cause-side entity, so there's nothing
    /// to walk back from `object` to a cause-anchoring memory.
    NoRelatedStatement = 4,
    /// Statement row vanished (race with FORGET cascade).
    StatementMissing = 5,
}

#[derive(Debug)]
pub struct CausalEdgeMetrics {
    drops_total: AtomicU64,
    edges_written_total: AtomicU64,
    skipped_non_causal_predicate: AtomicU64,
    skipped_low_confidence: AtomicU64,
    skipped_no_evidence: AtomicU64,
    skipped_object_not_entity: AtomicU64,
    skipped_no_related_statement: AtomicU64,
    skipped_statement_missing: AtomicU64,
    /// Gauge — how many causal predicates the worker resolved on this
    /// shard. Bumped on first successful resolve; 0 means the worker
    /// has no causal vocabulary and every drained enqueue no-ops.
    predicate_whitelist_resolved: AtomicU64,
    cycle_duration_seconds: WorkerHistogram,
}

impl CausalEdgeMetrics {
    #[must_use]
    pub fn new() -> Self {
        Self {
            drops_total: AtomicU64::new(0),
            edges_written_total: AtomicU64::new(0),
            skipped_non_causal_predicate: AtomicU64::new(0),
            skipped_low_confidence: AtomicU64::new(0),
            skipped_no_evidence: AtomicU64::new(0),
            skipped_object_not_entity: AtomicU64::new(0),
            skipped_no_related_statement: AtomicU64::new(0),
            skipped_statement_missing: AtomicU64::new(0),
            predicate_whitelist_resolved: AtomicU64::new(0),
            cycle_duration_seconds: WorkerHistogram::new(DEFAULT_CYCLE_BUCKETS_SECONDS),
        }
    }

    pub fn inc_drop(&self) {
        self.drops_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_edges_written(&self, n: u64) {
        self.edges_written_total.fetch_add(n, Ordering::Relaxed);
    }

    pub fn inc_skip(&self, reason: CausalSkipReason) {
        let c = match reason {
            CausalSkipReason::NonCausalPredicate => &self.skipped_non_causal_predicate,
            CausalSkipReason::LowConfidence => &self.skipped_low_confidence,
            CausalSkipReason::NoEvidence => &self.skipped_no_evidence,
            CausalSkipReason::ObjectNotEntity => &self.skipped_object_not_entity,
            CausalSkipReason::NoRelatedStatement => &self.skipped_no_related_statement,
            CausalSkipReason::StatementMissing => &self.skipped_statement_missing,
        };
        c.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_whitelist_resolved(&self, n: u64) {
        self.predicate_whitelist_resolved
            .store(n, Ordering::Relaxed);
    }

    pub fn observe_cycle_duration(&self, seconds: f64) {
        self.cycle_duration_seconds.observe(seconds);
    }

    #[must_use]
    pub fn snapshot(&self) -> CausalEdgeMetricsSnapshot {
        CausalEdgeMetricsSnapshot {
            drops_total: self.drops_total.load(Ordering::Relaxed),
            edges_written_total: self.edges_written_total.load(Ordering::Relaxed),
            skipped_non_causal_predicate: self.skipped_non_causal_predicate.load(Ordering::Relaxed),
            skipped_low_confidence: self.skipped_low_confidence.load(Ordering::Relaxed),
            skipped_no_evidence: self.skipped_no_evidence.load(Ordering::Relaxed),
            skipped_object_not_entity: self.skipped_object_not_entity.load(Ordering::Relaxed),
            skipped_no_related_statement: self.skipped_no_related_statement.load(Ordering::Relaxed),
            skipped_statement_missing: self.skipped_statement_missing.load(Ordering::Relaxed),
            predicate_whitelist_resolved: self.predicate_whitelist_resolved.load(Ordering::Relaxed),
            cycle_duration_seconds: self.cycle_duration_seconds.snapshot(),
        }
    }
}

impl Default for CausalEdgeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct CausalEdgeMetricsSnapshot {
    pub drops_total: u64,
    pub edges_written_total: u64,
    pub skipped_non_causal_predicate: u64,
    pub skipped_low_confidence: u64,
    pub skipped_no_evidence: u64,
    pub skipped_object_not_entity: u64,
    pub skipped_no_related_statement: u64,
    pub skipped_statement_missing: u64,
    pub predicate_whitelist_resolved: u64,
    pub cycle_duration_seconds: WorkerHistogramSnapshot,
}
