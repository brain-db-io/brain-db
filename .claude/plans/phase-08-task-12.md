# Sub-task 8.12 — Embedder cache eviction worker

**Spec:** `spec/11_background_workers/08_misc_workers.md` §4
**Phase doc:** `docs/phases/phase-08-workers.md` §8.12
**Done when:** Stale entries evicted; cache size bound respected.

---

## 1. Honest scope

`brain_embed::CachingDispatcher<D>` already ships:
- LRU cache of `(text_hash → vector)` with default capacity 10 000.
- `CacheStats` snapshot.
- LRU is automatic; the spec §4 worker only adds **age-based prune** ("entries older than 7 days").

What brain-embed **doesn't** expose:
- A `prune_older_than(Duration)` method on `CachingDispatcher`.
- A way for `brain-workers` to reach the cache without holding a concrete `CachingDispatcher` (brain-ops's `OpsContext.executor.embedder` is `Arc<dyn Dispatcher>` — no cache access through the trait).

Two paths:

| Option | Cost | End state |
| ------ | ---- | --------- |
| (a) Add `prune_older_than` to `CachingDispatcher` + thread an `Arc<CachingDispatcher>` through OpsContext | brain-embed touched, OpsContext field, type-leak through brain-ops | Worker calls it directly |
| (b) **Pluggable seam pattern** (matches 8.5/8.8) — `CacheEvictionSource` trait + `DisabledCacheEvictionSource` default. Phase 9 wires a real impl when admin tooling needs it | brain-workers-only | Worker counts evictions via the source |

**Plan picks (b).** Same shape as the HNSW maintenance / WAL retention workers. brain-embed stays untouched.

---

## 2. The seam

```rust
// crates/brain-workers/src/cache_evict.rs

#[derive(Debug, thiserror::Error)]
pub enum CacheEvictionError {
    #[error("cache eviction source disabled")]
    Disabled,
    #[error("cache eviction source failed: {0}")]
    Failed(String),
}

pub type PruneFuture<'a> =
    Pin<Box<dyn Future<Output = Result<usize, CacheEvictionError>> + Send + 'a>>;

/// Pluggable seam. Production injects an impl backed by
/// `CachingDispatcher::prune_older_than` (Phase 9). v1's default is a
/// no-op.
pub trait CacheEvictionSource: Send + Sync + 'static {
    fn prune_older_than(&self, max_age: Duration) -> PruneFuture<'_>;
}

pub struct DisabledCacheEvictionSource;
```

Same `Pin<Box<Future>>` pattern as 8.5 / 8.8. No `async-trait` dep.

---

## 3. Defaults

Spec §4.2 — entries older than **7 days** are removed. Configurable per worker. v1 default: 7d.

`WorkerKind::EmbedderCacheEvict` defaults from 8.1: 1m interval, batch_size 5_000, max_runtime 2s.

---

## 4. `CacheEvictionWorker`

```rust
pub const DEFAULT_CACHE_MAX_AGE: Duration = Duration::from_secs(7 * 24 * 3600);

pub struct CacheEvictionWorker {
    config: WorkerConfig,
    max_age: Duration,
    source: Arc<dyn CacheEvictionSource>,
}

impl CacheEvictionWorker {
    pub fn new(source: Arc<dyn CacheEvictionSource>) -> Self;
    pub fn with_config(self, cfg: WorkerConfig) -> Self;
    pub fn with_max_age(self, age: Duration) -> Self;
}
```

Cycle:
```rust
async fn do_cycle(&self, ctx) -> Result<usize, WorkerError> {
    if !self.config().enabled || self.config().batch_size == 0 { return Ok(0); }
    if ctx.is_shutdown() { return Ok(0); }
    match self.source.prune_older_than(self.max_age).await {
        Ok(n) => Ok(n),
        Err(CacheEvictionError::Disabled) => Ok(0),
        Err(CacheEvictionError::Failed(e)) => Err(WorkerError::Ops(format!("cache prune: {e}"))),
    }
}
```

The whole cycle is one trait call — bounded by the source's own impl. v1 default returns 0; Phase 9's CachingDispatcher-backed impl bounds work internally (batch_size + max_runtime stays in the worker config but is informational).

---

## 5. File-by-file plan

| File | Action | Notes |
| ---- | ------ | ----- |
| `crates/brain-workers/src/cache_evict.rs` | NEW | Source trait, DisabledCacheEvictionSource, CacheEvictionWorker |
| `crates/brain-workers/src/lib.rs` | Edit | Re-export |
| `crates/brain-workers/tests/cache_evict.rs` | NEW | ~9 tests |

No brain-embed change. No spec / wire / other-crate changes.

---

## 6. Tests

### Source (3)
1. `disabled_source_returns_disabled` — DisabledCacheEvictionSource → Err(Disabled).
2. `stub_source_returns_provided_count` — wraps a fixed return.
3. `failed_source_propagates_as_worker_error` — Failed → WorkerError::Ops.

### Cycle (3)
4. `cycle_with_disabled_source_returns_zero`.
5. `cycle_returns_source_count` — stub returns 12 → cycle returns 12.
6. `cycle_calls_source_with_configured_max_age` — stub captures the Duration; default cycle calls with 7d, custom with overridden value.

### Worker integration (3)
7. `worker_registers_with_correct_kind_and_default_cadence` — 1m interval.
8. `disabled_worker_via_config_does_not_run`.
9. `custom_max_age_honoured` — worker built with `with_max_age(1h)` passes 1h to the source.

Total: 9 tests.

---

## 7. Risks

| Risk | Mitigation |
| ---- | ---------- |
| Worker is essentially a thin trait-call wrapper — limited v1 value | Documented; v1's pluggable seam is the right architectural shape. Phase 9's `CachingDispatcher::prune_older_than` plus an `Arc<CachingDispatcher>` plumbed through OpsContext closes the loop |
| Spec §4.1 says LRU is auto on access — worker is supplementary | Spec §4.2 — the worker's job is the **age-based** prune, distinct from LRU bound enforcement |

---

## 8. Done criteria

- [ ] CacheEvictionSource + DisabledCacheEvictionSource + CacheEvictionWorker shipped.
- [ ] 9 tests pass first run.
- [ ] `cargo test --workspace` green; clippy + fmt clean.
- [ ] Commit subject: `feat(brain-workers): embedder cache eviction worker (sub-task 8.12)`.

~250 LOC impl + ~300 LOC tests. Small.

Out of scope (Phase 9): brain-embed `CachingDispatcher::prune_older_than` impl, OpsContext plumbing, ADMIN_CACHE_CLEAR trigger.
