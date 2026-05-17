# Plan: Phase 24 — Task 02, FORGET cascade worker

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1
**Depends on:** 24.0 (§27/04 §"FORGET cascade worker"),
                24.1 (worker_checkpoints scaffold).

---

## 1. Scope

When a memory is forgotten, dependent statements + relations
must (a) drop the memory from their evidence lists, (b)
recompute confidence per §25/00 §"Confidence aggregation",
and (c) tombstone with `reason = SourceMemoryForgotten` when
evidence empties and confidence falls below threshold. Today
`handle_forget` updates the substrate slot + WAL; the cascade
side is unimplemented.

Phase 24.2 makes that real. The cascade runs **asynchronously**
in a background worker (spec §25/00 contract: "the triggering
FORGET returns immediately; the cascade processes in
background").

Concrete deliverables (one commit):

1. **`brain-workers/src/workers/forget_cascade.rs`** (new) —
   `ForgetCascadeWorker` consuming a per-shard queue of
   `ForgetCascadeJob { memory_id, mode: ForgetMode }`.
2. **Queue plumbing**: a bounded flume channel on
   `OpsContext` (`forget_cascade_dispatcher: Option<Arc<...>>`)
   that `handle_forget` writes to post-commit. Same shape
   as 22.3's `MemoryTextDispatcher`.
3. **Cascade engine** in
   `brain-metadata::cascade_ops` — pure operations over a
   `WriteTransaction`:
   - `statements_referencing_memory(rtxn, mid)` → Vec<StatementId>
   - `drop_evidence(wtxn, statement_id, mid)` → updates the row.
   - `recompute_confidence(statement, ...)` per §25/00 formula.
   - `tombstone_for_source_forgotten(wtxn, statement_id, now, mode)`.
4. **Soft vs hard cascade**:
   - Soft FORGET → cascade marks dependent rows
     `pending_tombstone` with the same grace window; revertible
     within grace.
   - Hard FORGET → cascade tombstones immediately (or
     hard-deletes, per §25/00 retention table).
5. **Revert path**: when a soft FORGET is reverted within
   grace (substrate semantics), the cascade worker receives
   `ForgetCascadeJob::Revert { memory_id }` and rolls the
   pending-tombstone flags back.
6. **Audit rows** for each cascade step
   (`AuditOp::Tombstoned` per §25/00).

## 2. Spec references

- `spec/25_provenance_versioning/00_purpose.md`
  §"Cascading effects of FORGET" — cascade flow.
- `spec/25_provenance_versioning/00_purpose.md`
  §"Confidence aggregation across evidence" — recompute
  formula.
- `spec/27_knowledge_workers/04_state_carrying_workers.md`
  §"FORGET cascade worker" — worker mechanics (queued,
  rollback, audit).
- `spec/27_knowledge_workers/00_purpose.md` §"Scheduling
  priorities and budgets" — Background lane.

## 3. External validation

| Item | Source | Status |
|---|---|---|
| `STATEMENTS_TABLE` row layout (evidence + confidence) | `brain-metadata::tables::knowledge::statement` | shipped |
| Audit table writer | `brain-metadata::audit_ops::append_audit` | shipped |
| Statement supersede semantics | `brain-metadata::statement_ops::statement_supersede` | shipped |
| `handle_forget` post-commit hook | `brain-ops::ops::forget::handle_forget` | shipped (needs one new line to enqueue) |
| Confidence aggregation impl | `brain-core::knowledge::confidence::aggregate_confidence` | shipped |

## 4. Architecture sketch

```
brain-core/src/knowledge/cascade.rs                   (new)
  pub struct ForgetCascadeJob {
      pub memory_id: MemoryId,
      pub mode: ForgetMode,                           // Soft | Hard
      pub kind: CascadeKind,                          // Apply | Revert
      pub forgot_at_unix_nanos: u64,
  }

brain-metadata/src/cascade_ops.rs                     (new)
  pub fn evidence_index_for_memory(rtxn, mid)
      -> Result<Vec<StatementId>, Err>            // uses an index helper / scan fallback
  pub fn apply_forget_to_statement(
      wtxn,
      statement_id,
      memory_id,
      mode: ForgetMode,
      threshold: f32,
      now: u64,
  ) -> Result<CascadeOutcome, Err>
  pub enum CascadeOutcome {
      EvidenceDropped { new_confidence: f32 },
      MarkedPendingTombstone { grace_until: u64 },
      Tombstoned,
      Untouched,                                       // not present after recheck
  }
  pub fn revert_forget_for_statement(wtxn, sid, mid, now) -> Result<...>

brain-workers/src/workers/forget_cascade.rs           (new)
  pub struct ForgetCascadeWorker { rx: flume::Receiver<ForgetCascadeJob>, ... }
  impl Worker for ForgetCascadeWorker { ... }
  impl ForgetCascadeWorker {
      async fn process_job(&self, job: ForgetCascadeJob, ctx: &WorkerContext) {
          // 1. Open read txn; gather statement ids dependent on memory_id.
          // 2. Open write txn; for each, apply cascade or revert.
          // 3. Commit; emit audit rows in same txn.
          // 4. Emit `StatementTombstoned` / `StatementSupersededByCascade` change-feed events.
      }
  }

brain-ops/src/ops/forget.rs                           (one-line edit)
  - After successful WAL commit, if dispatcher present:
      ctx.forget_cascade_dispatcher.try_send(
          ForgetCascadeJob { memory_id, mode, kind: Apply, forgot_at }
      )?;
```

### Evidence-index optimisation

Phase-17 spec §"Storage" mentions a `(memory_id) →
Vec<StatementId>` reverse index (`STATEMENT_EVIDENCE_INDEX`).
If that index exists, lookups are O(deg(mid)); otherwise the
worker falls back to a `STATEMENTS_TABLE` scan with a
batched `inline_evidence` predicate. The plan uses whichever
the codebase ships (verify before code).

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Async worker (this plan) | Spec contract; doesn't block FORGET | New worker + queue plumbing | ✓ |
| Inline cascade in `handle_forget` | Simplest | Violates "FORGET returns immediately"; cascades for highly-referenced memories could stall RECALL for hundreds of ms | rejected |
| Per-cascade redb txn (one statement per txn) | Smallest txns | Many commits per FORGET | batch: one txn per FORGET job, capped at 256 statements/txn; spill to a follow-up txn if more |
| Drop evidence + tombstone in same step | Atomic per statement | Mixes two distinct §25/00 concerns | follow spec: evidence-drop is always; tombstone is conditional on confidence-after |
| Skip the revert path in v1 | Less code | Substrate's soft FORGET is reversible during grace; spec mandates cascade tracks it | implement revert (it's just an inverse op) |

## 6. Risks / open questions

- **Risk:** A memory referenced by 10K+ statements creates a huge cascade txn. **Mitigation:** batch in 256-statement chunks; cap total wall-time per job at 5 s; on cap, enqueue a continuation job for the remainder.
- **Risk:** A revert arriving after the apply-cascade has already started races. **Mitigation:** each cascade step is idempotent under the `pending_tombstone` flag; revert is a no-op when no pending flag is set.
- **Open question:** What's the per-kind confidence threshold below which a statement gets tombstoned? **Resolution:** spec §25/00 doesn't pin a number. Default `0.2`; configurable via `BRAIN_CASCADE_CONFIDENCE_THRESHOLD`.

## 7. Test plan

Unit tests in `cascade_ops.rs`:
- `apply_drops_evidence_and_recomputes_confidence`.
- `apply_tombstones_when_evidence_empty_and_below_threshold`.
- `apply_keeps_above_threshold_with_stale_flag` (empty evidence, confidence ≥ threshold).
- `revert_clears_pending_tombstone`.
- `idempotent_apply_twice`.

Unit tests in `forget_cascade.rs`:
- Queue dequeue order.
- Batching: 300-statement cascade → two txns.
- Cancellation on shutdown drains gracefully.

Integration test `brain-server/tests/forget_cascade_e2e.rs`:
- ENCODE 3 memories; STATEMENT_CREATE referencing all three as evidence; FORGET memory 1; assert statement's evidence list shrinks + confidence drops; FORGET memory 2 + 3; assert statement tombstoned with `SourceMemoryForgotten`.

## 8. Commit shape

```
feat(core,metadata,workers,ops,server): 24.2 — FORGET cascade worker

- brain-core/src/knowledge/cascade.rs (new): ForgetCascadeJob /
  CascadeKind types.
- brain-metadata/src/cascade_ops.rs (new): pure cascade ops over
  a WriteTransaction (apply / revert / batched outcome).
- brain-workers/src/workers/forget_cascade.rs (new): bounded-
  queue worker; one txn per job (cap 256 statements);
  continuation job for overflow.
- brain-ops/src/ops/forget.rs: enqueue cascade job post-commit
  (one if-let).
- brain-ops/src/context.rs: forget_cascade_dispatcher slot.
- brain-server/src/shard/mod.rs: spawn the worker at shard
  startup.
- Tests: 5 unit (cascade_ops) + 3 unit (worker) + 1 E2E.

Verified: cargo zigbuild --target x86_64-unknown-linux-gnu
--workspace --tests; cargo clippy -- -D warnings.
```

## 9. Confirmation

1. **Async worker** (not inline cascade) — matches §25/00 "FORGET returns immediately".
2. **Batched per-job txns** with 256-statement cap + continuation; 5 s wall-time cap.
3. **Confidence threshold default 0.2**, env-overridable.
4. **Revert path implemented** for soft FORGET within grace.
5. **One audit row per cascade step** (`AuditOp::Tombstoned` / `Superseded`) regardless of batching.
