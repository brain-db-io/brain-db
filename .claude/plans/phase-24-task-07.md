# Plan: Phase 24 — Task 07, Audit log sweeper

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1
**Depends on:** 24.0 (§27/03 §"Audit log sweeper").

---

## 1. Scope

Periodic low-priority worker that **hard-deletes audit rows
past retention** (default 90 days per spec §25/00
§"Retention"). Audit rows are append-only with UUIDv7 ids
(time-ordered), so range deletion is straightforward.

Concrete deliverables:

1. **`brain-metadata::audit_ops::sweep_expired`** (new fn) —
   pure op:
   ```rust
   pub fn sweep_expired(
       wtxn,
       retention_seconds: u64,
       now_ns: u64,
       batch_cap: usize,
       dry_run: bool,
   ) -> Result<SweepSummary, Err>
   ```
2. **`brain-workers/src/workers/audit_sweeper.rs`** (new) —
   `AuditLogSweeper` running on the Low priority lane, daily
   default cadence.
3. **Config**:
   - `BRAIN_AUDIT_RETENTION_SECONDS` (default 7 776 000 = 90 d).
   - `BRAIN_AUDIT_SWEEPER_PERIOD_SECONDS` (default 86 400).
   - `BRAIN_AUDIT_SWEEPER_BATCH_CAP` (default 1024).
4. **`AuditOp::Tombstoned`** rows describing merge-log
   reasons are NOT swept. Merge audit kept forever (spec
   §25/00 §"Retention": "Merge logs — Forever").
5. **Metrics**: `sweeper_swept_total{worker="audit"}`,
   `sweeper_skipped_total{worker, reason}` (reasons:
   `merge_log_skipped`).

## 2. Spec references

- `spec/25_provenance_versioning/00_purpose.md`
  §"Retention" — 90 d default; merge logs forever.
- `spec/27_knowledge_workers/03_sweeper_workers.md` (24.0)
  §"Audit log sweeper" — worker mechanics.

## 3. External validation

| Item | Source | Status |
|---|---|---|
| `AUDIT_TABLE` (UUIDv7 keys) | `brain-metadata::tables::knowledge::audit` | shipped |
| `AuditRow.operation` enum | `brain-metadata::tables::knowledge::audit` | shipped |
| Audit append | `brain-metadata::audit_ops::append_audit` | shipped |

## 4. Architecture sketch

```
brain-metadata/src/audit_ops.rs                       (extended)
  pub fn sweep_expired(
      wtxn: &WriteTransaction,
      retention_seconds: u64,
      now_ns: u64,
      batch_cap: usize,
      dry_run: bool,
  ) -> Result<SweepSummary, AuditError> {
      let cutoff_ns = now_ns.saturating_sub(retention_seconds * 1_000_000_000);
      let mut t = wtxn.open_table(AUDIT_TABLE)?;
      let mut to_remove = Vec::new();
      let mut summary = SweepSummary::default();
      for entry in t.iter()? {
          let (k, v) = entry?;
          let row = v.value();
          summary.scanned += 1;
          if row.timestamp_unix_nanos > cutoff_ns { continue }
          // Spec §25/00: merge logs forever.
          if matches!(row.operation_tag, AuditOpTag::Merged | AuditOpTag::Unmerged) {
              summary.skipped_merge_log += 1;
              continue;
          }
          to_remove.push(k.value());
          if to_remove.len() == batch_cap { break }
      }
      if !dry_run {
          for k in &to_remove { t.remove(k)?; summary.deleted += 1; }
      } else {
          summary.dry_run_would_delete = to_remove.len() as u64;
      }
      Ok(summary)
  }

brain-workers/src/workers/audit_sweeper.rs            (new)
  pub struct AuditLogSweeper { config: AuditSweeperConfig }
  impl Worker for AuditLogSweeper { ... }
```

### UUIDv7 range optimisation

UUIDv7 keys are time-prefixed. A future optimisation: scan
only the range `[0..cutoff_uuid]` rather than the whole
table. Out of scope for v1 (full scan is < 100 ms at 100K
rows; batch cap kicks in before then).

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Time-prefix range scan (UUIDv7) | Faster | More code; needs careful cutoff conversion | defer (post-v1 micro-opt) |
| Full scan + filter (this plan) | Simple; correct | O(n) per tick | ✓ — n bounded, batch cap |
| Default 30 d retention | Less storage | Loses 60 d of post-incident forensic value | follow spec (90 d) |
| Sweep all audit ops including merge | Less code | Loses merge history forever; violates §25/00 | exempt merge logs |
| Per-table audit sweeper (one per kind) | Maximal granularity | One audit table — single sweeper is enough | unify |

## 6. Risks / open questions

- **Risk:** A burst of audit activity pushes the table to millions of rows; one daily sweep can't keep up. **Mitigation:** the `batch_cap=1024` is per-tick; operators can either raise the cap or shorten the cadence. Steady-state at production load is small enough.
- **Open question:** Should "extraction" audit rows be retention-policed separately (they're the bulk by volume)? **Resolution:** v1 uses one retention for all non-merge ops. Per-op retention is post-v1 if needed.

## 7. Test plan

Unit tests:
- `sweep_drops_old_rows`.
- `sweep_keeps_recent_rows`.
- `sweep_skips_merge_audit_rows`.
- `sweep_respects_batch_cap`.
- `dry_run_doesnt_mutate`.

Integration:
- `brain-workers/tests/audit_sweep_e2e.rs` — seed 100 audit rows mixed across operations + timestamps; run sweep with 90 d retention; assert old non-merge gone, recent kept, all merge kept.

## 8. Commit shape

```
feat(metadata,workers): 24.7 — audit log sweeper

- brain-metadata/src/audit_ops.rs: new sweep_expired fn;
  merge / unmerge rows exempt per §25/00.
- brain-workers/src/workers/audit_sweeper.rs (new):
  daily-default Low-priority worker.
- brain-workers/src/config.rs: audit-sweeper config keys.
- Tests: 5 unit + 1 E2E.

Verified: cargo zigbuild --target x86_64-unknown-linux-gnu
--workspace --tests; cargo clippy -- -D warnings.
```

## 9. Confirmation

1. **90-day retention default**, env-overridable.
2. **Merge / Unmerge audit rows exempt** per spec §25/00 (forever).
3. **Batch cap 1024 per tick**; full table scan.
4. **UUIDv7 range optimisation deferred** to post-v1.
5. **One unified worker** (not per-op-type).
