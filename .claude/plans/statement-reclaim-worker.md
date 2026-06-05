# Plan: statement physical-reclamation (GC) worker

## Problem

`STATEMENT_RETRACT` (0x0144) is the hard-delete variant of statement removal, but
`crates/brain-metadata/src/statement/tombstone.rs:78` only tombstones — the TODO at
`:74` is unwired. Retracted statement rows and their secondary-index entries never
leave redb. Over a long-running deployment this is unbounded growth and it leaves
the **tombstone-grace-then-reclaim invariant** (CLAUDE.md §5.6) with a hole on the
typed-graph side: memories reclaim (`slot_reclaim`), entities reclaim (`entity_gc`),
statements do not.

## Spec grounding

- Reclamation contract: `spec/10_metadata/02_table_layout.md:587-595` (§19.12
  "Per-retract reclamation") — tombstone, then zero-out after `RETRACT_GRACE_NANOS`
  via the periodic GC worker; at reclaim remove from **all tables except** the audit
  row in `entity_resolution_audit` (discriminator `STATEMENT_RETRACTED`); strip
  `STATEMENTS_BY_EVIDENCE_TABLE` too.
- Statement lifecycle: `spec/02_data_model/07_statement.md:569` (retract "removes the
  row after the grace period"), `:124` (FORGET_STATEMENT same grace semantics).
- Sweeper discipline (the contract this worker MUST satisfy):
  `spec/15_background_workers/06_typed_graph_workers.md:348-389` — low-priority lane,
  clock-triggered `BRAIN_<NAME>_PERIOD_SECONDS`, bounded batches (small wtxn),
  mandatory `dry_run` returning `SweepSummary { scanned, deleted,
  dry_run_would_delete, skipped_reasons }`, `sweeper_*` metrics, stateless re-scan
  (no persistent cursor), per-row warn-and-continue.
- Closest code template: the **supersession sweeper** —
  `crates/brain-metadata/src/extractor/sweep.rs:30-95` +
  `crates/brain-workers/src/workers/supersession_sweeper.rs`. Same `extractor::sweep`
  home, same `SweepSummary`/dry-run/batch discipline, already sweeps both statements
  and relations.
- TOCTOU pattern to copy: `entity_gc` two-phase rtxn-collect → wtxn-recheck
  (`spec/15_background_workers/06_typed_graph_workers.md:527`, `:944-950`).

## Index footprint a reclaim must clean (from §19.9, table_layout.md:530-553)

1. `STATEMENTS_TABLE` (primary)
2. `STATEMENTS_BY_SUBJECT_TABLE`
3. `STATEMENTS_BY_PREDICATE_TABLE`
4. `STATEMENTS_BY_OBJECT_ENTITY_TABLE` (only if object is Entity)
5. `STATEMENTS_BY_EVENT_TIME_TABLE` (only if kind == Event)
6. `STATEMENTS_BY_EVIDENCE_TABLE` (§19.12 explicitly: strip these)
7. `STATEMENT_CHAIN_TABLE` (see decision 3 — chain invariant)
8. `evidence_overflow` row (only if EvidenceRef::Overflow)

HNSW + tantivy already handled at **tombstone** time (the StatementTextIndexer maps
Tombstone→Delete; the HNSW worker filters tombstoned rows), so reclaim needs no extra
derived-index work beyond the standard rebuild cycle.

## Design (mirrors supersession sweeper + entity_gc)

- New `reclaim_retracted_statements(grace, batch_cap, dry_run) -> SweepSummary` in
  `crates/brain-metadata/src/extractor/sweep.rs` (alongside the existing supersession
  sweeper). Eligibility: row is tombstoned AND the tombstone reason marks a *retract*
  (not a plain tombstone or supersession) AND `now - tombstoned_at >= grace`.
- Two-phase: rtxn collect candidate ids (bounded by `batch_cap`), then wtxn
  re-check-and-delete each (TOCTOU-safe), one small commit per batch.
- New worker `crates/brain-workers/src/workers/statement_reclaim.rs` modeled on
  `supersession_sweeper.rs`: clock-triggered `BRAIN_STATEMENT_RECLAIM_PERIOD_SECONDS`,
  enable flag, grace knob, batch cap, `sweeper_*` metrics, warn-and-continue.
- Write the `STATEMENT_RETRACTED` audit row in the same wtxn before delete.
- Generic-ready: factor so a future `RELATION_RETRACT` reclaim
  (open question R-OQ-5) reuses the same machinery (supersession sweeper already does
  both — follow that shape).

## Decisions needing owner sign-off (spec is unresolved / conflicting)

1. **Grace value.** §19.12 + the retention table (`10_metadata/00_purpose.md:373`) say
   `RETRACT_GRACE_NANOS` **30 days**; the wire-frame prose
   (`04_wire_protocol/08_typed_graph_frames.md:649`) says **7 days, same as memory**.
   CLAUDE.md §5.6 states the *memory* default is 7 days. **Recommend 30 days** (named
   constant + retention table win; the wire-frame line is stale prose) — operator
   override via `BRAIN_STATEMENT_RECLAIM_GRACE_SECONDS`.

2. **WAL record for reclaim.** No `StatementReclaim` WAL kind exists
   (`wal/kinds.rs:55-60` only has Create/Supersede/Tombstone). The supersession
   sweeper already deletes from `STATEMENTS_TABLE` with **no WAL record** — redb's own
   commit is the durability point for background sweeps. **Recommend: no WAL record**
   (match supersession-sweeper precedent; reclaim is idempotent re-derivation, safe to
   redo on recovery). This brushes invariant §5.1 (WAL-before-ack) in letter, but
   that invariant governs the *ack path*, not background sweepers — confirm.

3. **Mid-chain retract + chain invariant.** `STATEMENT_CHAIN_TABLE` requires dense
   `1..=N` versions, no gaps (`02_data_model/07_statement.md:441`). Reclaiming a
   *mid-chain* statement would punch a hole. Superseded rows are kept forever so the
   supersession sweeper never hits this; retract can. **Recommend: only physically
   reclaim chain-tail or standalone rows; for a mid-chain retracted row, keep the
   chain entry as a tombstone and reclaim the rest.** (Alternative: forbid retract on
   non-current chain members at the write path.) Confirm which.

## Test plan

- Unit (in `extractor/sweep.rs` tests): eligibility (grace not elapsed → skipped;
  elapsed → deleted; superseded-not-retracted → skipped), dry-run returns
  `dry_run_would_delete` without mutating, batch cap honored, all 7 tables + overflow
  cleaned, audit row written, mid-chain row handled per decision 3.
- Worker test (`statement_reclaim.rs`): clock trigger, disabled flag no-ops, metrics
  emitted, warn-and-continue on a poisoned row.
- Chaos/grace-window: retract → reclaim cycle survives restart (recovery sees the row
  gone, stays gone).
- Closes the brain-workers coverage gap flagged in the audit for this path.

## Verify

Docker (Linux-only): `just docker-verify` (build + clippy -D warnings + test).
brain-workers tests need `--test-threads=1` per the known OOM note.

## Commit shape

1. `feat(brain-metadata): reclaim_retracted_statements sweeper + tests`
2. `feat(brain-workers): statement-reclaim background worker + wire into shard`
(No `Co-Authored-By` trailer.)
