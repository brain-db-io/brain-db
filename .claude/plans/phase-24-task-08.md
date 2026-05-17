# Plan: Phase 24 — Task 08, Schema migration runner

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1
**Depends on:** 24.0 (§27/04 §"Schema migration worker"),
                24.1 (worker_checkpoints + backfill walker),
                §19 schema DSL,
                §25 re-extraction workflow.

---

## 1. Scope

When `SCHEMA_UPLOAD` declares a new version that invalidates
extracted state (typically a non-breaking version bump that
adds extractors or tightens schemas), the migration worker
**re-extracts** affected memories under the new version and
**supersedes** the old statements per §25/00 §"Re-extraction
workflow".

24.8 is essentially **a backfill driven by a SCHEMA_UPLOAD
event**, with the migration-plan diff logic (phase 19 already
computes which `(memory, extractor)` pairs need re-extraction)
mapped onto the 24.1 backfill walker.

Concrete deliverables:

1. **`brain-core/src/knowledge/migration.rs`** (new) —
   `MigrationPlan { items: Vec<MigrationItem> }`,
   `MigrationItem { memory_id, extractor_id, reason }`.
2. **`brain-workers/src/workers/schema_migration.rs`** (new)
   — `SchemaMigrationWorker` consuming a per-shard queue of
   `MigrationPlan`s, walking each item under the
   `worker_checkpoints` table (worker_id = "schema_migration").
3. **Re-extraction diff** in
   `brain-metadata::reextract_ops` (new module):
   - `reextract_memory(wtxn, memory_id, extractor_id, ctx) -> ReextractOutcome`
   - `ReextractOutcome` variants per §25/00:
     - `Refreshed { statement_id, new_confidence }`
     - `Superseded { old, new }`
     - `Created { new }`
     - `PotentiallyRetracted { statement_id }`
4. **Trigger plumbing**: `handle_schema_upload` (post-commit,
   24.0's §27/04) builds the migration plan and enqueues
   into the worker. If `dry_run=true`, the plan is returned
   in the response but not enqueued.
5. **`SCHEMA_UPLOAD` response extension**: include
   `migration_summary: Option<MigrationSummary>` with item
   counts so callers see what'll run.
6. **Audit rows** per §25/00 §"Re-extraction workflow" — one
   `AuditOp::Extracted` per re-extracted memory, one
   `AuditOp::Superseded` per supersede, one
   `AuditOp::SchemaUpgraded` per migration.

## 2. Spec references

- `spec/21_schema_dsl/00_purpose.md` §"Migration semantics" —
  what kinds of schema changes invalidate what.
- `spec/25_provenance_versioning/00_purpose.md`
  §"Re-extraction workflow" — diff semantics + outcomes per
  statement kind (Fact / Preference / Event).
- `spec/27_knowledge_workers/04_state_carrying_workers.md`
  (24.0) §"Schema migration worker" — worker mechanics +
  shared checkpoint table.

## 3. External validation

| Item | Source | Status |
|---|---|---|
| `handle_schema_upload` | `brain-ops::ops::knowledge_schema` | shipped |
| `SchemaUploadResponse.migration_summary_blob` | `brain-protocol::knowledge::schema_resp` | exists (blob, not summary) |
| Migration plan computation | phase 19 internals | shipped (verify shape) |
| Extractor pipeline run | `brain-ops::ops::extractor_pipeline::run_for_memory` | shipped |
| `worker_checkpoints` table | 24.1 | new |
| Statement supersede op | `brain-metadata::statement_ops::statement_supersede` | shipped |

## 4. Architecture sketch

```
brain-core/src/knowledge/migration.rs                 (new)
  pub struct MigrationPlan {
      pub from_version: u32,
      pub to_version: u32,
      pub namespace: String,
      pub items: Vec<MigrationItem>,
  }
  pub struct MigrationItem {
      pub memory_id: MemoryId,
      pub extractor_id: ExtractorId,
      pub reason: MigrationReason,
  }
  pub enum MigrationReason {
      ExtractorVersionBump,
      SchemaVersionBump,
      NewExtractor,
  }
  pub struct MigrationSummary {
      pub total_items: u32,
      pub by_reason: ByReasonCounts,
  }

brain-metadata/src/reextract_ops.rs                   (new)
  pub fn reextract_memory(
      wtxn: &WriteTransaction,
      memory_id: MemoryId,
      extractor_id: ExtractorId,
      pipeline_ctx: &ExtractorContext,
      now_ns: u64,
  ) -> Result<ReextractOutcome, ReextractError>

  pub enum ReextractOutcome {
      Refreshed { statement_id: StatementId, new_confidence: f32 },
      Superseded { old: StatementId, new: StatementId },
      Created { new: StatementId },
      PotentiallyRetracted { statement_id: StatementId },
      NoOp,
  }

brain-workers/src/workers/schema_migration.rs         (new)
  pub struct SchemaMigrationWorker {
      pending: Mutex<VecDeque<MigrationPlan>>,
  }
  impl Worker for SchemaMigrationWorker {
      fn run(&self, ctx) {
          let Some(plan) = self.dequeue() else { return Ok(()); }
          // Per-item walk; check checkpoint; re-extract; mark.
          // Same shape as 24.1's BackfillWorker but item-source
          // is the plan, not a memory range.
      }
  }

brain-ops/src/ops/knowledge_schema.rs                 (one edit)
  // Post-commit, build plan:
  let plan = compute_migration_plan(&old_version, &new_version, ctx)?;
  resp.migration_summary = Some(plan.summary());
  if !req.dry_run {
      ctx.schema_migration_dispatcher.try_send(plan)?;
  }
```

### Shared with 24.1

The per-item walk is identical to backfill's. v1 keeps the
worker types separate (clearer for ops + metrics) but the
**checkpoint discipline + cooperative yield + cancel flag**
helper lives in a `brain-workers::workers::common` module
that both call into. That helper lands here or in 24.1 —
whichever ships first picks the home; the other uses it.

### Per-statement-kind handling

Per §25/00 §"Re-extraction workflow":
- **Events**: non-destructive — always `Created`.
- **Preferences**: supersession applies; outcome
  `Superseded` or `Refreshed`.
- **Facts**: contradicting new fact → both stored; same-
  direction → `Refreshed` (confidence update only).

The diff logic lives in `reextract_ops::reextract_memory`,
parameterised by `StatementKind`.

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Reuse 24.1 backfill walker (this plan) | DRY; same checkpoint table; same cooperative yield | Two workers but one shared helper | ✓ |
| Single worker `BackfillOrMigration` | One worker | Conflates user-driven vs schema-driven flows; metrics noisier | rejected |
| Inline migration in `handle_schema_upload` | No queue plumbing | Blocks the upload response on potentially many seconds of re-extraction | spec contract: async |
| Auto-trigger migration on every schema upload | Smooth | Surprises operators; large LLM cost | trigger only when `migration_summary.total_items > 0`; admin can preview via dry-run |
| Per-statement-kind re-extraction logic in worker | Local | Belongs in metadata ops alongside other statement-mutation primitives | reextract_ops |

## 6. Risks / open questions

- **Risk:** A migration covering a million memories with LLM extractors costs real money. **Mitigation:** the `MigrationSummary` returned by `SCHEMA_UPLOAD` lets operators preview (with dry-run); migration also respects the LLM cost budget (skips items over budget; metric).
- **Risk:** Concurrent SCHEMA_UPLOAD during a running migration. **Mitigation:** migration plans queue; each is processed in order; the second migration's diff includes any rows the first left in `Failed` state.
- **Open question:** What if `compute_migration_plan` fails (schema diff is too complex)? **Resolution:** `SCHEMA_UPLOAD` still succeeds (the schema lands); migration plan returns `Err` and is surfaced in `migration_summary_blob` for the operator to inspect.

## 7. Test plan

Unit tests in `reextract_ops`:
- `event_kind_always_creates_new`.
- `preference_supersedes_when_value_changes`.
- `fact_refreshes_when_value_matches`.
- `fact_creates_when_value_differs_above_threshold`.

Unit tests in `schema_migration.rs`:
- Worker dequeues plans in FIFO order.
- Checkpoint resume — re-running a plan skips completed items.
- Dry-run plan never reaches the worker.

Integration test `brain-server/tests/schema_migration_e2e.rs`:
- Upload schema v1 + 100 memories + extractor → 100 statements.
- Upload schema v2 (extractor version bump).
- Wait for the migration worker to drain.
- Assert: 100 new statements created (Refreshed), originals updated `extractor_version`; `STATEMENT_LIST { stale_only: Some(true) }` returns 0.

## 8. Commit shape

```
feat(core,metadata,workers,ops,server): 24.8 — schema migration runner

- brain-core/src/knowledge/migration.rs (new):
  MigrationPlan / MigrationItem / MigrationSummary types.
- brain-metadata/src/reextract_ops.rs (new): per-kind diff
  logic; ReextractOutcome.
- brain-workers/src/workers/schema_migration.rs (new):
  Background-priority worker; reuses worker_checkpoints from
  24.1.
- brain-workers/src/workers/common.rs (new): shared helper
  for checkpoint walk + cooperative yield + cancel.
- brain-ops/src/ops/knowledge_schema.rs: build plan post-
  commit; enqueue unless dry_run.
- brain-protocol/src/knowledge/schema_resp.rs:
  SchemaUploadResponse.migration_summary field.
- Tests: 4 unit (reextract_ops) + 3 unit (worker) + 1 E2E.

Verified: cargo zigbuild --target x86_64-unknown-linux-gnu
--workspace --tests; cargo clippy -- -D warnings;
cargo test -p brain-protocol --lib.
```

## 9. Confirmation

1. **Async worker** triggered by post-commit SCHEMA_UPLOAD.
2. **Per-item via the shared `worker_checkpoints` table** (24.1).
3. **Per-kind diff logic** in `reextract_ops`: Event always creates; Preference supersedes; Fact refreshes/creates.
4. **`dry_run=true`** returns plan in response without enqueueing.
5. **Migration respects LLM cost budget** — over-budget items skip with metric.
