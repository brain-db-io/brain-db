# Plan: Phase 24 — Task 03, Supersession sweeper

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1
**Depends on:** 24.0 (§27/03 §"Supersession sweeper").

---

## 1. Scope

Periodic low-priority worker that **hard-deletes superseded
statements + relations** older than the retention window
(default forever — operator opt-in). Spec §25/00 §"Retention"
sets the policy ("Superseded Statements/Relations: Forever"
default; configurable per deployment).

The sweeper:

- Scans `STATEMENTS_TABLE` and `RELATIONS_TABLE` for rows
  with `superseded_by.is_some() AND now - superseded_at >=
  retention`.
- Hard-deletes them; emits an `AuditOp::Tombstoned` row with
  reason `SupersededRetentionExpired`.
- Runs per-batch (≤ 256 rows) under a single redb wtxn;
  yields cooperatively.
- Honours dry-run (just count + log; don't delete).
- Cadence: daily by default.

Concrete deliverables:

1. **`brain-workers/src/workers/supersession_sweeper.rs`** (new)
   — `SupersessionSweeper` implementing `Worker`.
2. **`brain-metadata::sweeper_ops`** (new module) — pure
   scan-and-delete operations:
   `sweep_superseded_statements(wtxn, retention, batch, dry_run)`
   `sweep_superseded_relations(wtxn, retention, batch, dry_run)`
   Each returns a `SweepSummary { scanned, deleted, dry_run_would_delete }`.
3. **Config** in `WorkerConfig`:
   - `BRAIN_SUPERSESSION_RETENTION_SECONDS` (default 0 = never sweep).
   - `BRAIN_SUPERSESSION_SWEEPER_PERIOD_SECONDS` (default 86 400).
   - `BRAIN_SUPERSESSION_SWEEPER_DRY_RUN` (default false; emerges as true automatically when retention=0).
4. **Default-off behaviour**: if `retention_seconds == 0`,
   the worker logs once at startup, marks itself disabled,
   and returns `Ok(())` on every tick.
5. **Metrics**: `sweeper_swept_total{worker="supersession", kind}`, `sweeper_skipped_total{worker, reason}`, `sweeper_latency_seconds{worker}`.

## 2. Spec references

- `spec/25_provenance_versioning/00_purpose.md` §"Retention" — default forever.
- `spec/27_knowledge_workers/03_sweeper_workers.md` (24.0) §"Supersession sweeper" + §"Shared invariants".
- `spec/27_knowledge_workers/00_purpose.md` §"Scheduling priorities and budgets" — Low lane (5 % of shard time).

## 3. External validation

| Item | Source | Status |
|---|---|---|
| Worker trait | `brain-workers::worker::Worker` | shipped |
| `STATEMENTS_TABLE` superseded fields | `brain-metadata::tables::knowledge::statement` | shipped |
| Audit append | `brain-metadata::audit_ops` | shipped |
| `WorkerConfig` plumbing | `brain-workers::config::WorkerConfig` | shipped |

## 4. Architecture sketch

```
brain-metadata/src/sweeper_ops.rs                     (new)
  pub struct SweepSummary {
      pub scanned: u64,
      pub deleted: u64,
      pub dry_run_would_delete: u64,
  }

  pub fn sweep_superseded_statements(
      wtxn: &WriteTransaction,
      retention_seconds: u64,
      now_unix_nanos: u64,
      batch_cap: usize,
      dry_run: bool,
  ) -> Result<SweepSummary, SweeperError> {
      let mut t = wtxn.open_table(STATEMENTS_TABLE)?;
      let mut summary = SweepSummary::default();
      let cutoff_ns = now_unix_nanos.saturating_sub(retention_seconds * 1_000_000_000);
      let mut to_delete = Vec::new();
      for entry in t.iter()? {
          let (k, v) = entry?;
          let row = v.value();
          summary.scanned += 1;
          if row.superseded_by != [0u8; 16] && row.superseded_at_unix_nanos <= cutoff_ns {
              to_delete.push(k.value());
              if to_delete.len() == batch_cap { break }
          }
      }
      if dry_run {
          summary.dry_run_would_delete = to_delete.len() as u64;
      } else {
          for key in to_delete {
              t.remove(&key)?;
              summary.deleted += 1;
              // audit row written by caller in the same wtxn.
          }
      }
      Ok(summary)
  }

  pub fn sweep_superseded_relations(...) -> ...     // symmetric

brain-workers/src/workers/supersession_sweeper.rs     (new)
  pub struct SupersessionSweeper { config: SweeperConfig }
  pub struct SweeperConfig {
      pub retention_seconds: u64,
      pub batch_cap: usize,
      pub dry_run: bool,
  }
  impl Worker for SupersessionSweeper {
      const KIND: WorkerKind = WorkerKind::SupersessionSweeper;
      fn run<'a>(&'a self, ctx: &'a WorkerContext)
          -> Pin<Box<dyn Future<Output=Result<(), WorkerError>> + 'a>>
      { Box::pin(async move {
          if self.config.retention_seconds == 0 {
              tracing::debug!("supersession sweeper disabled (retention=0)");
              return Ok(());
          }
          self.sweep_once(ctx).await
      })}
  }
  impl SupersessionSweeper {
      async fn sweep_once(&self, ctx: &WorkerContext) { ... }
  }
```

### Batching

The worker performs **one batch per tick** (256 rows). With
default daily cadence this clears modest backlogs across
several days; operators that need faster clearance lower the
period or raise the batch cap.

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Default off (retention=0; this plan) | Safe — no surprise deletions | Operator must opt in to see any work | ✓ — matches spec §25/00 "Forever" default |
| Default 30 d retention | Less storage growth out of the box | Surprise data loss for users who don't read docs | rejected |
| One large txn per tick | Faster | Long-running wtxn blocks writers | batch cap 256 |
| Time-bounded sweep vs. row-bounded | Predictable wall-time | Variable progress depending on row size | row-bounded (deterministic & metric-friendly) |
| Sweep statements + relations in one fn | Less code | Different table schemas; harder to test | two parallel fns sharing `SweepSummary` |

## 6. Risks / open questions

- **Risk:** A misconfigured `retention=1 second` deletes a long chain in one tick. **Mitigation:** batch cap + dry-run mode + startup-log of the effective retention.
- **Risk:** Audit log growth from sweep operations. **Mitigation:** audit-sweeper (24.7) handles audit retention; expected steady state.
- **Open question:** Should we keep a one-row-per-chain "tombstone marker" so chain_root → history queries don't 404? **Resolution:** v1 hard-deletes; chain history beyond retention is gone (spec §25/00 binding). Operators who want forever-history set retention=0.

## 7. Test plan

Unit tests in `sweeper_ops.rs`:
- `sweep_zero_retention_does_nothing_when_disabled_caller` (worker layer enforces the gate; ops fn requires caller to skip when retention=0).
- `sweep_retains_unsupreseded` — superseded_by zeros not collected.
- `sweep_retains_within_window` — superseded but newer than cutoff stays.
- `sweep_deletes_eligible_up_to_batch_cap`.
- `dry_run_counts_without_deleting`.

Unit tests in `supersession_sweeper.rs`:
- Disabled worker is no-op.
- Active worker calls into ops fn with expected args.

Integration test `brain-workers/tests/supersession_sweep.rs`:
- 100-statement fixture with 60 superseded (50 old, 10 fresh); retention=1 day; run sweeper; assert 50 deleted, 10 retained, 40 untouched.

## 8. Commit shape

```
feat(metadata,workers): 24.3 — supersession sweeper

- brain-metadata/src/sweeper_ops.rs (new): pure scan-and-delete
  ops for superseded statements + relations.
- brain-workers/src/workers/supersession_sweeper.rs (new):
  periodic Low-priority worker; default off (retention=0).
- brain-workers/src/config.rs: SweeperConfig + env wiring.
- Tests: 5 unit (ops) + 2 unit (worker) + 1 integration.

Verified: cargo zigbuild --target x86_64-unknown-linux-gnu
--workspace --tests; cargo clippy -- -D warnings.
```

## 9. Confirmation

1. **Default off** (`retention_seconds=0`) — operator opt-in only; spec §25/00 binding.
2. **Batch cap 256 per tick**, daily cadence default — predictable wtxn footprint.
3. **Dry-run mode** logs counts without deleting; useful for first-time operators.
4. **Hard delete** (no tombstone marker); chain history past retention is gone.
5. **One audit row per deletion**, same wtxn — keeps audit-sweep predictable.
