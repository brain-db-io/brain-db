# Plan: Phase 24 — Task 05, LLM cache sweeper

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1
**Depends on:** 24.0 (§27/03 §"LLM cache sweeper"),
                21.x (LLM cache table).

---

## 1. Scope

Periodic low-priority worker that maintains the per-shard
LLM extractor response cache (`llm_cache.redb`, phase 21.5):

- **TTL expiry** — drop entries past their TTL (default 90 d,
  spec §25/00 §"Retention").
- **LRU eviction** — when total size > capacity, evict oldest
  rows by `last_used_at_unix_nanos` until under cap.

The cache is opt-in per deployment (the `LlmCacheDb` slot on
`OpsContext` is `None` for substrate-only deployments). When
the slot is `None`, the worker is a no-op.

Concrete deliverables:

1. **`brain-metadata::llm_cache_ops::sweep`** (new fn on the
   existing `llm_cache` module):
   - `sweep_expired(wtxn, ttl_seconds, now_ns, batch_cap, dry_run) -> SweepSummary`
   - `enforce_capacity(wtxn, max_bytes, batch_cap, dry_run) -> SweepSummary`
2. **`brain-workers/src/workers/llm_cache_sweeper.rs`** (new)
   — `LlmCacheSweeper` calling both ops in sequence.
3. **Config** in `WorkerConfig`:
   - `BRAIN_LLM_CACHE_TTL_SECONDS` (default 90 d = 7 776 000).
   - `BRAIN_LLM_CACHE_MAX_BYTES` (default 1 GiB; 0 = unlimited).
   - `BRAIN_LLM_CACHE_SWEEPER_PERIOD_SECONDS` (default 3 600).
4. **Metrics**: `sweeper_swept_total{worker="llm_cache", reason}` (reasons: `ttl_expired`, `capacity`), `llm_cache_size_bytes`, `llm_cache_entries`.

## 2. Spec references

- `spec/25_provenance_versioning/00_purpose.md`
  §"Retention" — 90 d default.
- `spec/26_knowledge_storage/00_purpose.md` §"LLM cache" —
  table layout.
- `spec/27_knowledge_workers/03_sweeper_workers.md`
  (24.0) §"LLM cache sweeper" — worker mechanics.

## 3. External validation

| Item | Source | Status |
|---|---|---|
| `LLM_CACHE_TABLE` layout | `brain-metadata::tables::llm_cache` | shipped 21.x |
| `LlmCacheDb` handle | `brain-metadata::llm_cache::LlmCacheDb` | shipped |
| `OpsContext.llm_cache` slot | `brain-ops::context` | shipped |

## 4. Architecture sketch

```
brain-metadata/src/llm_cache.rs                       (extended)
  pub fn sweep_expired(
      wtxn: &WriteTransaction,
      ttl_seconds: u64,
      now_ns: u64,
      batch_cap: usize,
      dry_run: bool,
  ) -> Result<SweepSummary, Error> {
      let cutoff_ns = now_ns.saturating_sub(ttl_seconds * 1_000_000_000);
      let mut t = wtxn.open_table(LLM_CACHE_TABLE)?;
      let mut to_remove = Vec::new();
      for entry in t.iter()? {
          let (k, v) = entry?;
          let row = v.value();
          if row.created_at_unix_nanos <= cutoff_ns {
              to_remove.push(k.value().to_vec());
              if to_remove.len() == batch_cap { break }
          }
      }
      if !dry_run {
          for k in &to_remove { t.remove(&k.as_slice())?; }
      }
      Ok(SweepSummary { scanned: ..., deleted: ..., dry_run_would_delete: ... })
  }

  pub fn enforce_capacity(
      wtxn,
      max_bytes: u64,
      batch_cap: usize,
      dry_run: bool,
  ) -> Result<SweepSummary, Error> {
      // 1. Sum existing sizes (we maintain a separate METADATA_TABLE row
      //    with running total, updated on every insert / remove).
      // 2. If under cap, return.
      // 3. Iterate by `last_used_at_unix_nanos` (secondary index — see Risk §6).
      // 4. Remove oldest until under cap or batch_cap reached.
  }

brain-workers/src/workers/llm_cache_sweeper.rs        (new)
  pub struct LlmCacheSweeper { config: LlmCacheSweeperConfig }
  impl Worker for LlmCacheSweeper {
      fn run<'a>(&'a self, ctx: &'a WorkerContext)
          -> Pin<Box<dyn Future<Output=Result<(), WorkerError>> + 'a>>
      { Box::pin(async move {
          let Some(cache) = ctx.llm_cache.as_ref() else {
              return Ok(()); // substrate-only deployment — no-op.
          };
          let mut guard = cache.lock();
          let wtxn = guard.write_txn()?;
          let exp_summary = sweep_expired(&wtxn, self.ttl, now_ns(), self.batch_cap, self.dry_run)?;
          let cap_summary = enforce_capacity(&wtxn, self.max_bytes, self.batch_cap, self.dry_run)?;
          wtxn.commit()?;
          metrics::record_sweep_two_pass(exp_summary, cap_summary);
          Ok(())
      })}
  }
```

### Size-budget tracking

redb has no built-in "table size". Two options:

- **Sum sizes per iteration** (current approach) — accurate
  but O(n) per sweep tick. For 10K-entry cache @ ~1 KB each
  this is sub-millisecond.
- **Running counter in `METADATA_TABLE`** — fast but adds a
  dependency on every cache write being paired with a counter
  update.

v1 uses **sum per sweep tick** (Option 1). The running-counter
optimisation is post-v1 if metrics show contention.

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| TTL + capacity (this plan) | Spec contract | Two passes per tick | ✓ |
| TTL only | Simpler | Cache grows unbounded for active hot keys | rejected |
| Capacity only | Simpler | Stale cache never expires | rejected |
| Pre-emptive eviction on insert | Bounded size at all times | Couples write path to eviction; tail latency hit | sweeper-based eviction is async |
| Approximate LRU | Faster | Loses recency information | exact LRU via timestamp scan; cache size is small enough |
| Separate worker per pass | Smaller per-tick footprint | Two schedule slots; cache is small enough for one pass | unify |

## 6. Risks / open questions

- **Risk:** No secondary index on `last_used_at_unix_nanos` means LRU eviction scans the whole table. **Mitigation:** at 10K-100K cache entries this is ms-level work; budget allows it. Post-v1 we add a secondary index if metrics show pressure.
- **Risk:** `last_used_at` not updated on read (only on insert). **Mitigation:** v1 LRU effectively uses `created_at`. Acceptable for a 90-d TTL cache where most evictions are TTL-driven. Document in code + spec.
- **Open question:** Should the sweeper persist a "high-water mark" so it can skip TTL pass when nothing's expired? **Resolution:** out of scope; cost is negligible.

## 7. Test plan

Unit tests in `llm_cache_ops`:
- `sweep_expired_drops_old`.
- `sweep_expired_keeps_recent`.
- `enforce_capacity_evicts_oldest_first`.
- `enforce_capacity_noop_when_under_cap`.
- `dry_run_doesnt_mutate`.

Unit tests in `llm_cache_sweeper.rs`:
- Worker is no-op when `ctx.llm_cache.is_none()`.
- Worker calls both passes when enabled.

Integration test `brain-workers/tests/llm_cache_sweep.rs`:
- Seed 100 entries with mixed `created_at`; 30 past TTL.
- Sweep with TTL=90d; assert 30 deleted, 70 retained.
- Add 50 more (now total 120); set max_bytes that triggers eviction; sweep; assert oldest evicted first.

## 8. Commit shape

```
feat(metadata,workers): 24.5 — LLM cache sweeper

- brain-metadata/src/llm_cache.rs: new sweep_expired +
  enforce_capacity fns.
- brain-workers/src/workers/llm_cache_sweeper.rs (new):
  TTL + LRU passes; no-op when llm_cache slot is None
  (substrate-only deployments).
- brain-workers/src/config.rs: cache-sweeper config keys.
- Tests: 5 unit (ops) + 2 unit (worker) + 1 integration.

Verified: cargo zigbuild --target x86_64-unknown-linux-gnu
--workspace --tests; cargo clippy -- -D warnings.
```

## 9. Confirmation

1. **TTL + capacity in one worker tick** — two passes, single wtxn.
2. **Per-tick whole-table scan** for both passes — secondary indexes deferred.
3. **`last_used_at` not refreshed on read** in v1 — effective LRU is by `created_at`. Documented.
4. **No-op when `OpsContext.llm_cache` is None** — substrate-only deployments unaffected.
5. **Defaults**: TTL 90 d, max_bytes 1 GiB, cadence hourly.
