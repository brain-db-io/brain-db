# Plan: Phase 24 — Task 01, Backfill worker

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1
**Depends on:** 24.0 (spec backfill — §27/04 ships the
`worker_checkpoints` design).

---

## 1. Scope

Implement the **backfill worker** described in §27/04
§"Backfill worker". An operator (CLI or admin HTTP) requests
re-extraction of a `(memory_range, extractor_set)`; the worker
walks the range under the priority budget, re-runs the named
extractors against each memory, and writes the resulting
statements / relations. The walk is **resumable** via the
shared `worker_checkpoints` redb table — restart picks up at
the first `Pending` row, idempotency from §27/00 §"Idempotency
reminders" makes re-runs of completed work no-ops.

Concrete deliverables (one commit):

1. **`crates/brain-metadata/src/tables/worker_checkpoints.rs`**
   (new) — `WORKER_CHECKPOINTS_TABLE` per §27/04 §"The
   `worker_checkpoints` table" with composite key + row
   layout (rkyv-archivable).
2. **`crates/brain-workers/src/workers/backfill.rs`** (new) —
   `BackfillWorker` matching the existing `Worker` trait
   (sibling of `wal_retention`, `consolidation`, etc.).
3. **`brain-workers::workers::backfill` re-exports** in
   `src/workers/mod.rs` + `src/lib.rs`.
4. **`BackfillRequest` / `BackfillProgress` types** in
   `brain-core::worker_state` (new module) so the CLI /
   admin layer can hand a typed request to the worker.
5. **No wire opcode in this commit.** The operator surface
   (CLI subcommand or admin HTTP endpoint) lands separately;
   24.0 §6 deferred that choice. The worker accepts a typed
   `BackfillRequest` so either driver can plug in later.

## 2. Spec references

- `spec/27_knowledge_workers/04_state_carrying_workers.md`
  (24.0) §"Backfill worker" — trigger, granularity, priority,
  resume semantics.
- `spec/27_knowledge_workers/00_purpose.md` §"Scheduling
  priorities and budgets" — Background lane (20 % of shard
  time).
- `spec/25_provenance_versioning/00_purpose.md` §"Re-extraction
  workflow" — diff vs supersede / contradiction semantics
  for re-extracted statements.
- `spec/16_benchmarks_acceptance/02_latency_targets.md`
  §2.10 — recall must stay under target while a backfill is
  running (worker yields cooperatively).

## 3. External validation

| Item | Source | Status |
|---|---|---|
| Worker trait + scheduler | `brain-workers::worker::Worker` | shipped |
| Extractor dispatch | `brain-ops::ops::extractor_pipeline::run_for_memory` | shipped |
| `MEMORIES_TABLE` range iteration | `brain-metadata::tables::memory::MEMORIES_TABLE` | shipped |
| redb write txn discipline | `brain-metadata::MetadataDb::write_txn` | shipped |

## 4. Architecture sketch

```
brain-core/src/worker_state/mod.rs                    (new)
  pub struct BackfillRequest {
      pub request_id: BackfillId,
      pub memory_range: BackfillRange,                // ById(start..end) | All
      pub extractor_ids: SmallVec<[ExtractorId; 4]>,
      pub priority: WorkerPriority,                   // overrides default
      pub dry_run: bool,
  }
  pub struct BackfillProgress {
      pub completed: u64,
      pub failed: u64,
      pub skipped_already_completed: u64,
      pub last_processed_memory_id: Option<MemoryId>,
  }

brain-metadata/src/tables/worker_checkpoints.rs       (new)
  pub const WORKER_CHECKPOINTS_TABLE:
      TableDefinition<'_, (&str, &[u8]), WorkerCheckpointRow>
  pub struct WorkerCheckpointRow {
      pub status: u8,                                 // 0=Pending 1=Completed 2=Failed
      pub attempts: u32,
      pub started_at_unix_nanos: u64,
      pub completed_at_unix_nanos: u64,
      pub last_error: Option<String>,
  }
  impl_redb_rkyv_value!(WorkerCheckpointRow, "brain_metadata::WorkerCheckpointRow::v1");
  pub mod ops {
      pub fn get(txn, worker_id, item_key) -> Result<Option<Row>, _>
      pub fn mark_started(txn, worker_id, item_key, now) -> Result<(), _>
      pub fn mark_completed(txn, worker_id, item_key, now) -> Result<(), _>
      pub fn mark_failed(txn, worker_id, item_key, err, now) -> Result<(), _>
      pub fn list_pending(rtxn, worker_id, limit) -> Result<Vec<(Vec<u8>, Row)>, _>
  }

brain-workers/src/workers/backfill.rs                 (new)
  pub struct BackfillWorker {
      shared: Arc<BackfillState>,
  }
  struct BackfillState {
      pending_requests: Mutex<VecDeque<BackfillRequest>>,
      current: Mutex<Option<RunningBackfill>>,
  }
  impl Worker for BackfillWorker {
      const KIND: WorkerKind = WorkerKind::Backfill;
      fn run<'a>(&'a self, ctx: &'a WorkerContext)
          -> Pin<Box<dyn Future<Output=Result<(), WorkerError>> + 'a>>
      { Box::pin(async move {
          if let Some(req) = self.dequeue().await { self.run_one(req, ctx).await }
          Ok(())
      })}
  }
  impl BackfillWorker {
      async fn run_one(&self, req: BackfillRequest, ctx: &WorkerContext) { ... }
      async fn process_memory(&self, memory_id, extractors, ctx) { ... }
  }
```

### Per-item flow

```
for memory_id in memory_range:
    for ext_id in extractors:
        item_key = (memory_id_bytes || ext_id_bytes)
        match checkpoint.get("backfill", item_key)? {
            Some(Completed)  => skipped += 1; continue,
            Some(Failed) if attempts >= MAX => skipped += 1; continue,
            _ => mark_started(..)
        }
        if dry_run {
            mark_completed(..); continue
        }
        match extractor_pipeline::run_for_memory(memory_id, [ext_id], ctx).await {
            Ok(_)  => { mark_completed(..); completed += 1 }
            Err(e) => { mark_failed(.., e); failed += 1 }
        }
        // Cooperative yield (every N items) — keeps RECALL latency stable.
```

### Cancellation

`BackfillRequest` carries a `request_id`; admin sends an
`AdminCancelBackfill(request_id)` that flips a per-request
cancel flag (in `RunningBackfill`). The worker checks the
flag at each item boundary; the current item completes,
then the worker returns.

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Per-(memory, extractor) checkpoints (this plan) | Most granular resume; matches §27/04 | One redb row per item × extractor (storage cost) | ✓ — match spec |
| Per-memory checkpoint only | Less storage | Re-runs already-completed extractors after restart | rejected |
| In-memory queue only | Simplest | No resume on restart | rejected — spec violation |
| Single backfill at a time vs. concurrent | Fewer race conditions | Limits operator parallelism | v1: single concurrent; queue handles request backlog |
| Wire opcode in this commit | One-stop landing | 24.0 deferred the surface; CLI is enough for v1 | defer |
| Inline yield (no scheduler integration) | Simpler | Burns CPU; doesn't respect priority budget | use the scheduler's cooperative yield via `Worker` trait |

## 6. Risks / open questions

- **Risk:** A bad extractor causes every item to fail, churning the checkpoint table. **Mitigation:** `MAX_ATTEMPTS = 3` with exponential backoff at the per-item level; after a request reaches `failure_rate > 50% over first 100 items`, the worker aborts with a single warn log and surfaces the failure rate in `BackfillProgress`.
- **Risk:** Concurrent SCHEMA_UPLOAD during a backfill invalidates the extractor set. **Mitigation:** the worker pins the `extractor_version` at request start; mid-backfill schema updates queue a separate migration request (24.8 handles).
- **Open question:** Should the worker emit a `BackfillCompleted` event on the change feed? **Resolution:** yes; matches existing audit-event discipline (§25/00). Lands as one new `KnowledgeEventPayload::BackfillCompleted` variant.

## 7. Test plan

Unit tests in `backfill.rs`:
- `dequeue_returns_in_order` — FIFO queue behaviour.
- `dry_run_marks_complete_without_extractor_dispatch` — uses a mock extractor pipeline that panics on call.
- `failed_item_records_attempts_and_skips_after_cap`.
- `cancel_flag_aborts_at_next_item_boundary`.

Unit tests in `worker_checkpoints/ops.rs`:
- Round-trip insert + get.
- `list_pending` respects limit + worker_id filter.
- `mark_failed` increments attempts monotonically.

Integration test `brain-workers/tests/backfill_integration.rs`:
- Build a 100-memory fixture with one pattern extractor.
- Submit a backfill request covering all 100.
- Stop the worker mid-run (after 30 items).
- Restart; assert it picks up at 31, completes all 100, statement count matches.

## 8. Commit shape

```
feat(metadata,core,workers): 24.1 — backfill worker

- brain-metadata/src/tables/worker_checkpoints.rs (new):
  WORKER_CHECKPOINTS_TABLE + WorkerCheckpointRow + ops helpers.
- brain-core/src/worker_state.rs (new): BackfillRequest /
  BackfillRange / BackfillProgress / BackfillId / WorkerPriority.
- brain-workers/src/workers/backfill.rs (new): BackfillWorker
  implementing Worker; per-(memory, extractor) checkpoint
  walk; cooperative yield; cancellation; dry-run.
- brain-workers/src/workers/mod.rs + lib.rs: register +
  re-export.
- Tests: 4 unit + 1 integration; 100-memory restart-resume
  scenario.

Verified: cargo zigbuild --target x86_64-unknown-linux-gnu
--workspace --tests; cargo clippy -- -D warnings;
cargo test -p brain-workers --lib.
```

## 9. Confirmation

1. **Per-(memory, extractor) checkpoint granularity** vs. per-memory.
2. **`WORKER_CHECKPOINTS_TABLE` is shared** across backfill / migration / future resumable workers — composite key disambiguates by `worker_id`.
3. **Single concurrent backfill in v1** — additional requests queue; concurrent execution is post-v1.
4. **No wire opcode in this commit** — admin surface lives behind a CLI subcommand or admin HTTP route added separately.
5. **MAX_ATTEMPTS = 3 with abort-on-50%-failure-rate** for safety against bad-extractor blast radius.
