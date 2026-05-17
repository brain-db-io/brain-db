# Plan: Phase 24 — Task 00, Spec backfill (§27/03 + §27/04)

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1 (one `docs(spec): 24.0 — …` commit)

> **Why this opens the phase.** The session-memory rule
> [[feedback_spec_first_workflow]] is binding: before any code,
> the matching spec must be at implementation depth. Phase 22.0
> and 23.0 followed the same pattern — surface the spec gaps
> the phase's sub-tasks will collide with, fill them once, then
> let the per-sub-task plans cite a stable spec section.

---

## 1. Scope

Two new normative spec files plus one targeted amendment.
Closes the documentation gap between §27/00's worker-table
("here are all the workers") and the per-sub-task plans for
24.1–24.8 ("how do they actually behave"). After 24.0, every
phase-24 sub-task can cite a spec section at the depth §22 /
§23 expected from us.

### 1.1 `spec/27_knowledge_workers/03_sweeper_workers.md` (new)

Normative spec for the five **periodic low-priority sweepers**:

- **Supersession sweeper** (24.3) — hard-delete chain-old
  statements/relations past retention.
- **Audit log sweeper** (24.7) — drop audit rows past
  retention (default 90 d per §25/00 §"Retention").
- **LLM cache sweeper** (24.5) — TTL expiry + LRU eviction on
  the per-shard `llm_cache.redb`.
- **Stale extraction detector** (24.4) — flag statements with
  `schema_version` or `extractor_version` behind the current
  registry.
- **Entity GC** (24.6) — off-by-default; tombstone entities
  with no active inbound references after grace.

Shared discipline:

- Cadence: configurable per worker; defaults daily for
  audit/supersession, hourly for LLM cache + stale detector,
  daily for entity GC (when enabled).
- Priority: `Low` per §27/00 §"Scheduling priorities and
  budgets" (≤ 5% of shard time).
- Batch size: bounded per pass to keep redb wtxn small;
  defaults documented per worker.
- Dry-run mode: each sweeper exposes `dry_run: bool` so
  operators can audit before destructive sweeps. Phase 14
  acceptance tests run in dry-run by default.
- Metrics: `sweeper_swept_total{worker}`,
  `sweeper_skipped_total{worker, reason}`,
  `sweeper_latency_seconds{worker}`.
- Idempotency: re-running a sweeper after a partial pass is
  safe (each delete is conditional on the retention predicate
  re-evaluated at run time).
- Restart semantics: sweepers are stateless — they re-scan
  the relevant table from scratch each run. No checkpoint
  table.

Per-worker normative bits (one section each):

- Trigger / cadence default / config env var.
- Retention predicate.
- Bounded-batch scan procedure.
- Audit row written (where applicable per §25/00).
- Failure handling (warn + skip vs warn + fatal).

### 1.2 `spec/27_knowledge_workers/04_state_carrying_workers.md` (new)

Normative spec for the three **state-carrying workers** —
periodic by *trigger* rather than clock, with persistent
checkpoint state:

- **Backfill worker** (24.1) — admin `ADMIN_BACKFILL`
  trigger. Per-(memory_id, extractor_id) checkpoint table in
  redb so a restart resumes mid-run. Respects priority budget
  (§27/00 §"Scheduling priorities and budgets" — Background
  lane, 20% of shard time).
- **FORGET cascade worker** (24.2) — triggered by every
  `FORGET` (soft or hard). Performs the cascade described in
  §25/00 §"Cascading effects of FORGET":
  - Evidence list update on dependent statements/relations.
  - Confidence recompute (per §25/00 §"Confidence
    aggregation").
  - Optional tombstone with `reason = SourceMemoryForgotten`
    when evidence becomes empty and confidence < threshold.
  - Audit row per cascade step.
  - Soft FORGET → soft cascade (revertible during grace);
    hard FORGET → hard cascade (irrevocable).
- **Schema migration worker** (24.8) — triggered by
  successful `SCHEMA_UPLOAD` when the new version invalidates
  any extracted state. Executes the migration plan computed
  in phase 19 (re-extract per §25/00 §"Re-extraction
  workflow"). Resumable via the backfill checkpoint table.

Shared discipline:

- **Checkpoint table layout.** New redb table
  `worker_checkpoints` with composite key
  `(worker_id, item_key)` and value
  `{ status, started_at, completed_at, attempts, last_error }`.
  Workers consult before each unit of work; mark complete on
  success; skip on `Completed`.
- Retry policy: each item retried up to N times (default 3)
  with exponential backoff; failed items recorded but the
  worker continues (so a bad memory doesn't stall the
  pipeline).
- Cancellation: admin can cancel a running backfill /
  migration; in-flight item completes; subsequent items
  abort cleanly.
- Restart semantics: on shard restart, workers re-attach to
  their checkpoint and resume from the first
  `Pending`/`Failed` item. No work is lost; idempotency
  guarantees from §27/00 §"Idempotency reminders" ensure
  re-runs are safe.
- Metrics: `worker_progress{worker, status}`,
  `worker_items_total{worker, status}`,
  `worker_latency_seconds{worker}`,
  `worker_resume_total{worker}`.

### 1.3 Amendment — `spec/27_knowledge_workers/00_purpose.md`

The worker table (current §27/00 §"New worker types added
here") is a useful summary but its trigger/priority/back-
pressure columns are thin for the sweepers + cascade. Add
forward links so future readers can jump from the table to
the new §27/03 + §27/04 detail:

```diff
 | **Pattern extractor** | On ENCODE | Foreground (sync) | None |
+...
 | **FORGET cascade** | On Memory FORGET | Background | None (rare events) |
+| (see §27/04 §"FORGET cascade worker")                                   |
 | **Supersession sweeper** | Periodic | Low | None |
+| (see §27/03 §"Supersession sweeper")                                    |
```

Realised as a single "see §27/03 for sweepers; §27/04 for
backfill + cascade + migration" paragraph appended below the
table, not as table-column noise.

## 2. Spec references

- [`spec/25_provenance_versioning/00_purpose.md`](../../spec/25_provenance_versioning/00_purpose.md) §"Cascading effects of FORGET" + §"Confidence aggregation" + §"Stale extraction detection" + §"Re-extraction workflow" + §"Retention".
- [`spec/27_knowledge_workers/00_purpose.md`](../../spec/27_knowledge_workers/00_purpose.md) §"New worker types added here" + §"Scheduling priorities and budgets" + §"Idempotency reminders for workers".
- [`spec/27_knowledge_workers/01_extractor_workers.md`](../../spec/27_knowledge_workers/01_extractor_workers.md) — the **shape** model for §27/03 and §27/04 (one normative-spec file per worker family, with cross-refs to §00).
- [`spec/27_knowledge_workers/02_text_indexer_workers.md`](../../spec/27_knowledge_workers/02_text_indexer_workers.md) — same shape model.
- [`spec/21_schema_dsl/00_purpose.md`](../../spec/21_schema_dsl/00_purpose.md) — migration semantics for §27/04 §"Schema migration worker".
- [`spec/22_extractors/05_audit.md`](../../spec/22_extractors/05_audit.md) — audit-row shape that the cascade / backfill / migration workers write.
- [`spec/26_knowledge_storage/00_purpose.md`](../../spec/26_knowledge_storage/00_purpose.md) §"LLM cache" — table layout for the LLM cache sweeper.

## 3. External validation

Not applicable — 24.0 is documentation-only. No new external
dependencies; no behaviour changes.

## 4. Architecture sketch

### §27/03 outline

```
27.03 Sweeper Workers (phase 24.3 / 24.4 / 24.5 / 24.6 / 24.7)

1. Two families, one discipline
   - Periodic low-priority — clock-triggered, no
     checkpoint, idempotent re-scans.
2. Shared invariants
   - Cadence, batch size, dry-run, metrics, idempotency,
     failure handling.
3. Supersession sweeper
   - Cadence: daily (override BRAIN_SUPERSESSION_SWEEPER_PERIOD).
   - Retention: default forever (configurable per
     deployment).
   - Scan: range over STATEMENTS_TABLE by chain_root +
     version; delete entries with `superseded_by.is_some()
     AND now - tombstoned_at >= retention`.
   - Per-batch wtxn cap: 256 deletes.
4. Audit log sweeper
   - Cadence: daily.
   - Retention: 90 days default.
   - Scan: range over AUDIT_TABLE by timestamp (UUIDv7
     ordered).
5. LLM cache sweeper
   - Cadence: hourly.
   - TTL: 90 days (matches §25/00 §"Retention").
   - LRU eviction when over capacity.
   - Per-shard redb table: LLM_CACHE_TABLE (phase 21.4).
6. Stale extraction detector
   - Cadence: hourly.
   - Predicate: §25/00 §"Stale extraction detection" formula.
   - Effect: writes `stale_flag` on the row (does NOT
     re-extract; that's the schema migration worker's job).
   - Output queryable via admin.
7. Entity GC worker
   - Off by default; opt-in via BRAIN_ENTITY_GC_ENABLED.
   - Cadence: daily.
   - Eligibility: entity with no active inbound statements
     AND no active inbound relations for ≥ grace_period
     (default 30 days).
   - Reversal during grace.
   - Audit row per tombstone.
8. Metrics
9. Failure handling
10. Restart semantics
11. Open questions (link to §27/07)
```

### §27/04 outline

```
27.04 State-carrying Workers (phase 24.1 / 24.2 / 24.8)

1. Three workers, one checkpoint table
2. The `worker_checkpoints` table
   - Key: (worker_id: &str, item_key: &[u8])
   - Value: WorkerCheckpointRow { status, attempts,
     timestamps, last_error }
3. Backfill worker
   - Trigger: `ADMIN_BACKFILL` opcode (separate phase or
     deferred).
   - Per-(memory_id, extractor_id) granularity.
   - Priority lane: Background (20% budget).
   - Resume semantics.
   - Cancellation.
4. FORGET cascade worker
   - Trigger: FORGET_REQ (sub-handler enqueues).
   - Per-statement cascade procedure.
   - Soft vs hard cascade.
   - Audit rows.
   - Rollback on FORGET revert (within grace).
5. Schema migration worker
   - Trigger: SCHEMA_UPLOAD (phase 19 emits a migration
     plan; this worker executes it).
   - Plan format (refer to §21/00).
   - Per-(memory_id, extractor_id) re-extraction unit.
6. Metrics
7. Failure handling
8. Restart semantics
9. Open questions
```

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Two new files (this plan) | Matches §27/01 + §27/02 grouping (one file per family); each sub-task plan can cite a focused section | More files | ✓ |
| One mega `03_phase_24_workers.md` | Single file | 800+ LOC; harder to navigate; doesn't match §27/01/§27/02 shape | rejected |
| Per-worker files (8 new files) | Maximally focused | Over-fragmented; the sweepers share discipline that's natural to colocate | rejected |
| Defer backfill to per-sub-task plans | Less doc work up front | Each plan would re-invent the wheel; spec-first rule forbids | rejected |
| New `§16/02 §2.12` for v1.0 acceptance perf | Symmetric with §2.9 / §2.10 | The acceptance gate (§31/00) already lists the perf criteria with concrete numbers; duplicating them risks drift | use §31/00 + cross-ref from §16/02 |
| Append entries to §27/00 worker table only | Minimal change | §27/00 is overview-shaped; sub-task implementers can't cite specific spec lines | rejected |

## 6. Risks / open questions

- **Risk:** Adding two ~250-LOC normative files risks
  documentation drift if the implementation changes shape
  later. **Mitigation:** the same risk applied to §27/02 /
  §23/02 / §23/03 / §23/04 in 22.0 / 23.0; we kept those in
  sync by editing the spec alongside code. Same discipline
  applies here.
- **Risk:** The `worker_checkpoints` table design pre-commits
  shape that the implementation may want to vary. **Mitigation:**
  describe the **shape** (key composition, status enum,
  retry/cancel semantics) but leave field-level rkyv types
  to the implementation; phase-22/23 specs took the same
  liberty.
- **Open question:** Does the FORGET cascade worker need a
  trigger queue or does it run inline in `handle_forget`?
  **Resolution:** queue (§27/04 §"FORGET cascade worker" §"Trigger").
  Inline cascade would block FORGET's response; queued is
  the §25/00 §"Cascading effects of FORGET" contract
  ("triggering FORGET returns immediately; the cascade
  processes in background").
- **Open question:** Should `ADMIN_BACKFILL` get a wire
  opcode in phase 24, or remain a CLI / admin HTTP
  endpoint? **Resolution:** the spec docs the worker's
  inputs without committing to a wire opcode. 24.1's
  per-sub-task plan picks the surface.

## 7. Test plan

24.0 is documentation-only. No code, no tests. Verified by:

- Both new files lint cleanly (no broken Markdown links).
- All cross-refs resolve (`grep -r '\[\`./0[34]_'
  spec/27_knowledge_workers/` returns no broken paths).
- The amendment paragraph at the end of §27/00 lands
  cleanly.

## 8. Commit shape

Single commit:

```
docs(spec): 24.0 — §27/03 + §27/04 + §27/00 amendment

Spec backfill that lifts phase 24's sub-tasks (24.1–24.8) from
"covered by §27/00's table" to implementation depth. Same
pattern that 22.0 + 23.0 used to seed each phase.

- spec/27_knowledge_workers/03_sweeper_workers.md (new, ~250
  LOC): normative spec for the five periodic low-priority
  sweepers — supersession, audit, LLM cache, stale
  extraction, entity GC. Shared discipline (cadence,
  batch size, dry-run, metrics, idempotency, restart).
- spec/27_knowledge_workers/04_state_carrying_workers.md
  (new, ~250 LOC): normative spec for the three workers
  with persistent checkpoint state — backfill, FORGET
  cascade, schema migration. Documents the shared
  `worker_checkpoints` table, resume semantics,
  cancellation, retry policy.
- spec/27_knowledge_workers/00_purpose.md: append a
  "see §27/03 and §27/04 for detailed mechanics" paragraph
  below the worker table.
```

No code; no clippy / test gates; no tag movement.

## 9. Confirmation

Please confirm:

1. **Two new spec files** (`§27/03 Sweeper Workers`, `§27/04
   State-carrying Workers`) at the same depth as §27/01 +
   §27/02 — versus a single mega-file or per-worker
   fragments.
2. **`worker_checkpoints` table is the v1 checkpoint
   substrate** for backfill + schema migration + (any future
   resumable worker). Backfill / FORGET-cascade rollback
   semantics defined per-worker, not as a shared transactional
   primitive.
3. **FORGET cascade is queued, not inline** — matches §25/00's
   "triggering FORGET returns immediately" contract.
4. **Entity GC stays off by default** (env-flag opt-in) and
   the spec documents the eligibility predicate but leaves
   the grace value configurable.
5. **`ADMIN_BACKFILL` wire opcode is out of scope for 24.0** —
   the spec describes the worker's inputs; 24.1's per-sub-
   task plan picks the surface.

After approval: write the two spec files + the §27/00
amendment, lint, commit. No tag; phase exit lives in 24.12.
