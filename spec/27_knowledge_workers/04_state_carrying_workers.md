# 27.04 State-carrying Workers

Normative spec for the three **trigger-driven workers** that
carry persistent checkpoint state across restarts.
Implemented in phase 24:

- 24.1 — `BackfillWorker`.
- 24.2 — `ForgetCascadeWorker`.
- 24.8 — `SchemaMigrationWorker`.

These workers differ from the §27/03 sweepers in three ways:

1. **Triggered, not periodic.** An external event
   (admin RPC, FORGET commit, SCHEMA_UPLOAD commit) places a
   unit of work in a queue. The worker drains the queue at
   the configured priority.
2. **Granular per-item state.** Each "unit of work" within a
   single trigger (e.g. one memory × extractor pair within a
   backfill plan) has its own checkpoint row so the work is
   resumable mid-run.
3. **Restartable.** On shard restart, a worker re-attaches
   to its checkpoint table and resumes from the first
   `Pending` row. Idempotency from §00 §"Idempotency
   reminders" makes re-runs of completed items safe.

Cross-references:
- [`./00_purpose.md`](./00_purpose.md) — worker overview.
- [`../25_provenance_versioning/00_purpose.md`](../25_provenance_versioning/00_purpose.md)
  §"Cascading effects of FORGET" + §"Re-extraction workflow".
- [`../21_schema_dsl/00_purpose.md`](../21_schema_dsl/00_purpose.md)
  §"Migration semantics".

## 1. The `worker_checkpoints` table

Shared across all state-carrying workers.

```rust
pub const WORKER_CHECKPOINTS_TABLE:
    TableDefinition<'_, (&str, &[u8]), WorkerCheckpointRow>
    = TableDefinition::new("worker_checkpoints");

pub struct WorkerCheckpointRow {
    /// 0 = Pending, 1 = Started, 2 = Completed, 3 = Failed.
    pub status: u8,
    pub attempts: u32,
    pub started_at_unix_nanos: u64,
    pub completed_at_unix_nanos: u64,
    pub last_error: Option<String>,
}
```

### 1.1 Key composition

- First component is the worker id (`"backfill"`,
  `"forget_cascade"`, `"schema_migration"`) — a stable
  string constant per worker.
- Second component is the per-item byte key. Composition
  depends on the worker:
  - Backfill: `memory_id.to_be_bytes() ‖ extractor_id.to_le_bytes()`
  - FORGET cascade: `memory_id.to_be_bytes() ‖ statement_id.to_bytes()`
    (one row per (memory, dependent statement) pair).
  - Schema migration: same layout as backfill —
    `memory_id ‖ extractor_id` — because migration is
    fundamentally a re-extraction.

The composite key lets a single redb table host all
workers' checkpoints without name collisions.

### 1.2 Status transitions

```
Pending  ──started──> Started
Started  ──ok─────>  Completed
Started  ──err────>  Failed (attempts++; retry if < max)
Failed   ──retry──>  Started
Completed            (terminal)
```

Workers consult `get(worker_id, item_key)` before each unit
of work:

- `None` → write `Pending` then `Started`; do the work.
- `Pending` / `Started` (stale, e.g. left after crash) →
  treat as fresh work; transition to `Started`.
- `Completed` → skip; count as `skipped_already_completed`.
- `Failed` with `attempts < MAX_ATTEMPTS` → retry; transition
  to `Started`.
- `Failed` with `attempts >= MAX_ATTEMPTS` → skip; count as
  `skipped_failed`.

`MAX_ATTEMPTS = 3` default; configurable.

### 1.3 Retry policy

Exponential backoff between attempts is applied at the
worker level (the checkpoint table stores only the attempt
count). Workers compute the backoff as
`min(60 s, 2^attempts * 100 ms)` before re-enqueueing a
`Failed` item.

### 1.4 Cancellation

Each running plan carries a cancel flag (process-local;
flipped by an admin RPC or by the worker's own abort
logic). Workers check the flag at each item boundary; the
current item completes (so its checkpoint reaches a
terminal state); subsequent items are not processed.

A cancelled plan leaves a mix of `Completed` + `Pending`
rows; a subsequent admin re-enqueue of the same request id
resumes from the first non-`Completed` row.

### 1.5 Cleanup

Completed checkpoints accumulate. v1 keeps them indefinitely
(audit value). Post-v1 a sweeper similar to §27/03 §3 (audit
log) hard-deletes `Completed` rows past a configurable
retention; tracked as an open question.

## 2. Shared discipline

### 2.1 Priority

Background lane per §00 §"Scheduling priorities and
budgets" (≤ 20% of shard time). Cooperative yielding via the
`Worker` trait's `run` future.

### 2.2 Per-item flow

Workers walk their input items via a generic helper
(`brain_workers::workers::common::run_checkpointed`):

```
for item in plan.items:
    if cancelled: break
    let row = checkpoint::get(worker_id, item.key())?;
    match row:
        Some(Completed) => skipped_complete += 1; continue
        Some(Failed { attempts >= MAX }) => skipped_failed += 1; continue
        _ => checkpoint::mark_started(worker_id, item.key(), now)?
    let result = process_item(item, ctx).await;
    match result:
        Ok(()) => checkpoint::mark_completed(worker_id, item.key(), now)?
        Err(e) => checkpoint::mark_failed(worker_id, item.key(), e, now)?
    yield_now().await;
```

The yield between items lets the scheduler interleave
higher-priority work.

### 2.3 Metrics

Per-worker, prefixed `worker_*`:

- `worker_progress{worker, status}` — counter per status
  transition.
- `worker_items_total{worker, status}` — gauge of current
  plan's per-status counts.
- `worker_latency_seconds{worker}` — histogram of per-item
  wall-time.
- `worker_resume_total{worker}` — counter incremented on
  shard startup when a worker resumes a partially-completed
  plan.
- `worker_failure_rate{worker, request_id}` — gauge to drive
  bad-extractor abort logic (§3.3).

## 3. Backfill worker

### 3.1 Trigger

An admin request — wire opcode or CLI subcommand; the spec
documents the worker's input contract, not the surface:

```rust
pub struct BackfillRequest {
    pub request_id: BackfillId,                    // UUIDv7
    pub memory_range: BackfillRange,               // ById | All
    pub extractor_ids: SmallVec<[ExtractorId; 4]>,
    pub priority: WorkerPriority,                  // overrides default
    pub dry_run: bool,
}

pub enum BackfillRange {
    All,
    ById { start: MemoryId, end: MemoryId },
}
```

### 3.2 Per-item granularity

`(memory_id, extractor_id)` — the smallest replayable unit.
Two memories × three extractors → six checkpoint rows.

### 3.3 Bad-extractor abort

After the first 100 items, if `failed / processed > 0.5`,
the worker aborts the plan with a single `warn` log + a
`BackfillAborted { reason: HighFailureRate }` event on the
change feed. Prevents a misconfigured extractor from
spending hours hammering a million memories.

### 3.4 Concurrency

v1 runs **one backfill at a time per shard**. Additional
requests queue on `BackfillWorker.pending_requests`.
Multiple shards run independently.

### 3.5 Dry-run

Dry-run marks each item `Completed` without invoking the
extractor pipeline. Used for plan validation + cost preview
before live runs.

### 3.6 Cancellation

`AdminCancelBackfill(request_id)` flips the running plan's
cancel flag. The current item's extractor call completes;
the worker writes that item's checkpoint to a terminal state
and stops dequeueing further items.

## 4. FORGET cascade worker

### 4.1 Trigger

`handle_forget` enqueues one `ForgetCascadeJob` per FORGET
**post-commit**:

```rust
pub struct ForgetCascadeJob {
    pub memory_id: MemoryId,
    pub mode: ForgetMode,           // Soft | Hard
    pub kind: CascadeKind,          // Apply | Revert
    pub forgot_at_unix_nanos: u64,
}
```

### 4.2 Per-job procedure

```
1. Open a read txn; gather statement_ids + relation_ids
   whose evidence contains `memory_id`.
2. For each dependent record:
     checkpoint::mark_started("forget_cascade",
                              memory_id ‖ record_id, now)
3. Open a write txn (batched, ≤ 256 records per txn).
4. For each record in the batch:
     a. Drop `memory_id` from `evidence`.
     b. Recompute `confidence` per §25/00.
     c. If evidence.is_empty():
          - confidence >= threshold:
              mark `stale_evidence` flag; keep row.
          - else:
              tombstone with reason=SourceMemoryForgotten;
              audit row.
     d. mark_completed in the same wtxn.
5. Commit. If more than 256 dependents remain, enqueue a
   continuation job for the leftover.
```

### 4.3 Soft vs hard cascade

- **Soft FORGET** (the substrate's default with a grace
  window): the cascade marks dependent rows with the same
  grace expiry. If the FORGET is reverted within grace, the
  cascade receives a `CascadeKind::Revert` job and rolls
  back the pending-tombstone flag on each affected row.
- **Hard FORGET**: the cascade hard-tombstones immediately.

### 4.4 Confidence threshold

`BRAIN_CASCADE_CONFIDENCE_THRESHOLD = 0.2` default. Below
this, an empty-evidence statement is tombstoned; above this,
it survives with the `stale_evidence` flag set (operator can
re-extract or accept the staleness).

### 4.5 Continuation jobs

A FORGET against a heavily-referenced memory (e.g. 10K
statements) is split across multiple jobs of up to 256
dependents each. Continuation jobs carry the same
`memory_id` + a `start_after_record_id` cursor so the worker
resumes correctly under cancellation.

### 4.6 Audit rows

One `AuditOp::Tombstoned` per dependent that gets
tombstoned. One `AuditOp::Superseded` per dependent that
gets `stale_evidence` (because the row still exists but its
content is now derived from a smaller evidence set).

## 5. Schema migration worker

### 5.1 Trigger

Post-commit hook on `handle_schema_upload`. When the new
schema version invalidates existing extraction state (per
§21/00 §"Migration semantics"), the handler:

1. Computes a `MigrationPlan { items: Vec<MigrationItem> }`
   where each item is a `(memory_id, extractor_id)` pair
   that needs re-extraction.
2. Returns the plan summary in the `SchemaUploadResponse`.
3. **If not dry-run**, enqueues the plan on the migration
   worker.

### 5.2 Per-item procedure

```
for item in plan.items:
    checkpoint::mark_started("schema_migration",
                             item.memory_id ‖ item.extractor_id, now)
    let outcome = reextract_memory(
        wtxn, item.memory_id, item.extractor_id, ctx, now,
    )?;
    audit_row_for(outcome);
    checkpoint::mark_completed(...)?
    yield_now().await;
```

`reextract_memory` returns a `ReextractOutcome` per §25/00
§"Re-extraction workflow":

- `Refreshed { statement_id, new_confidence }` — same kind
  + subject + predicate + object as an existing statement;
  bump version + confidence.
- `Superseded { old, new }` — preference / fact with same
  identity but new value.
- `Created { new }` — no matching existing statement
  (new extractor or new content).
- `PotentiallyRetracted { statement_id }` — old statement
  no longer produced by the new extractor; flagged for
  operator review.
- `NoOp` — checkpoint state says already-done; skip.

### 5.3 Cost budget

Schema migrations that touch LLM extractors respect the
per-extractor cost budget. Items over budget are skipped
with a `skipped_over_budget` metric; operators can re-run
after raising the budget.

### 5.4 Cancellation

`AdminCancelSchemaMigration(request_id)`. Same shape as
backfill cancellation.

### 5.5 Audit rows

One `AuditOp::SchemaUpgraded` per plan, written once at
plan start. Per-item audit rows per the
`ReextractOutcome`'s normal extraction-audit semantics
(`AuditOp::Extracted` / `AuditOp::Superseded`).

## 6. Open questions

Tracked in [`./07_open_questions.md`](./07_open_questions.md):

- Sweep of `Completed` checkpoint rows past retention.
- Multi-item-per-txn batching for backfill /
  schema-migration (current v1: one item per txn for clean
  isolation; batching is a perf optimisation post-v1).
- Concurrent backfill / migration plans within a shard.
- Per-extractor parallelism within a single migration plan.

These do not block phase 24 exit.
