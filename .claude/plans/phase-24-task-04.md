# Plan: Phase 24 — Task 04, Stale extraction detector

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1
**Depends on:** 24.0 (§27/03 §"Stale extraction detector"),
                §19 schema registry, §20 extractor registry.

---

## 1. Scope

Periodic low-priority worker that **flags** statements whose
`schema_version` or `extractor_version` is behind the current
registry (spec §25/00 §"Stale extraction detection"). The
worker only **sets a flag** — it does NOT re-extract. The
schema-migration worker (24.8) is the side that re-extracts
when an admin triggers a migration plan.

Concrete deliverables:

1. **New flag bit** on `StatementRow.flags` —
   `STATEMENT_FLAG_STALE_EXTRACTION = 1 << 3` (or whatever bit
   is free per `tables/knowledge/statement.rs`).
2. **`brain-metadata::stale_ops`** (new module) — pure ops
   over a redb txn:
   - `current_schema_versions(rtxn) -> HashMap<NamespaceId, u32>`
   - `current_extractor_versions(rtxn) -> HashMap<ExtractorId, u32>`
   - `mark_stale(wtxn, statement_id) -> Result<bool, _>` (returns true on transition).
   - `scan_and_flag_batch(wtxn, current_versions, batch_cap) -> SweepSummary`.
3. **`brain-workers/src/workers/stale_detector.rs`** (new) —
   `StaleExtractionDetector` running on the Low priority
   lane, default cadence hourly.
4. **Admin read-side** — extend `ADMIN_LIST_TOMBSTONED` ?
   No — better to add a new `STATEMENT_LIST_STALE` admin
   query OR repurpose the existing `STATEMENT_LIST` filter
   with `include_stale: bool`. **Decision:** `STATEMENT_LIST`
   already has the right shape; we add a new request field
   `stale_only: Option<bool>` (None = all, Some(true) = stale only, Some(false) = fresh only). Spec backfill is one wire-shape edit.
5. **Metrics**: `sweeper_swept_total{worker="stale_detector"}`, `sweeper_skipped_total{worker, reason}`, `sweeper_latency_seconds{worker}`.

## 2. Spec references

- `spec/25_provenance_versioning/00_purpose.md`
  §"Stale extraction detection" — predicate + behaviour.
- `spec/27_knowledge_workers/03_sweeper_workers.md`
  (24.0) §"Stale extraction detector" — worker mechanics.
- `spec/21_schema_dsl/00_purpose.md` — `schema_version`
  semantics.
- `spec/22_extractors/00_purpose.md` — `extractor_version`
  semantics.

## 3. External validation

| Item | Source | Status |
|---|---|---|
| `StatementRow.schema_version` / `extractor_version` / `flags` | `brain-metadata::tables::knowledge::statement` | shipped |
| Schema registry read | `brain-metadata::schema_store::schema_active` | shipped |
| Extractor registry read | `brain-metadata::extractor_ops::extractor_get` | shipped |

## 4. Architecture sketch

```
brain-metadata/src/tables/knowledge/statement.rs       (one new const)
  pub const STATEMENT_FLAG_STALE_EXTRACTION: u32 = 1 << 3;

brain-metadata/src/stale_ops.rs                        (new)
  pub struct CurrentVersions {
      pub schema_per_namespace: HashMap<String, u32>,
      pub extractor_per_id: HashMap<ExtractorId, u32>,
  }
  pub fn load_current_versions(rtxn) -> Result<CurrentVersions, Err>

  pub fn is_stale(row: &StatementRow, versions: &CurrentVersions) -> bool {
      // §25/00 formula:
      //   statement.schema_version < current.schema_for(statement.namespace)
      //   || statement.extractor_version < current.extractor_for(statement.extractor_id)
  }

  pub fn scan_and_flag_batch(
      wtxn,
      versions: &CurrentVersions,
      batch_cap: usize,
      dry_run: bool,
  ) -> Result<StaleScanSummary, Err>
  pub struct StaleScanSummary {
      pub scanned: u64,
      pub flagged_now: u64,            // transitions
      pub flagged_already: u64,
      pub dry_run_would_flag: u64,
  }

brain-workers/src/workers/stale_detector.rs            (new)
  pub struct StaleExtractionDetector { config: SweeperConfig }
  impl Worker for StaleExtractionDetector { ... }
  impl StaleExtractionDetector {
      async fn detect_once(&self, ctx: &WorkerContext) {
          let rtxn = ctx.metadata.read_txn();
          let versions = load_current_versions(&rtxn)?;
          drop(rtxn);
          let wtxn = ctx.metadata.write_txn();
          let summary = scan_and_flag_batch(&wtxn, &versions, self.batch_cap, self.dry_run)?;
          wtxn.commit()?;
          metrics::record_sweep(WorkerKind::StaleDetector, &summary);
      }
  }

brain-protocol/src/knowledge/statement_req.rs          (one field)
  pub struct StatementListRequest {
      ...existing...
      pub stale_only: Option<bool>,
  }
  // Server-side filter in handle_statement_list applies the flag check.
```

### Idempotency

Scanning the whole table on every tick is wasteful. Two
optimisations both kept post-v1:

- **Watermark by schema_version**: track the highest
  schema_version seen per namespace; only re-scan when a
  newer version exists. Out of scope for v1 (premature).
- **Lazy flag on read**: compute stale-ness during
  `STATEMENT_GET`. Tempting but breaks the operator's "show
  me what's stale" answer. Deferred.

v1 sticks with full-table scan, batched.

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Flag + report (this plan) | Spec contract; cheap | Doesn't auto-re-extract | ✓ — re-extraction is 24.8's job |
| Auto-re-extract on detection | Convenient | Spec says "manual or via migration worker"; surprise LLM cost | rejected |
| Lazy flag on read | Zero background work | Operator can't list stale without scanning | rejected (operator query is the use case) |
| Watermark by schema_version | Skips already-flagged scans | Premature optimisation; full scan @ 100K stmts is sub-second | defer |
| New `STATEMENT_LIST_STALE` admin opcode | Explicit | `STATEMENT_LIST` already takes filters | extend existing |

## 6. Risks / open questions

- **Risk:** Flag stuck at `true` after re-extraction. **Mitigation:** the schema-migration worker (24.8) clears the flag on each statement it re-extracts (within the same wtxn).
- **Open question:** Should `is_stale` include tombstoned statements? **Resolution:** no — flag the active set only; tombstoned rows are excluded from the scan.
- **Open question:** Per-statement namespace lookup overhead. **Resolution:** `predicates_table.get(predicate_id)` exists; cache the (`predicate_id → namespace`) map in `CurrentVersions` at the start of the scan.

## 7. Test plan

Unit tests in `stale_ops.rs`:
- `is_stale_schema_behind` — older schema_version → true.
- `is_stale_extractor_behind` — older extractor_version → true.
- `is_stale_current_is_fresh` — false.
- `scan_and_flag_batch_transitions` — counts `flagged_now` vs `flagged_already`.
- `dry_run_doesnt_mutate`.

Unit tests in `stale_detector.rs`:
- Worker loops the scan once per tick.

Integration test `brain-server/tests/stale_detection_e2e.rs`:
- Upload schema v1 + create 10 statements.
- Upload schema v2 (non-breaking, version bump).
- Run the detector.
- `STATEMENT_LIST { stale_only: Some(true) }` returns all 10.

## 8. Commit shape

```
feat(metadata,workers,protocol,ops): 24.4 — stale extraction detector

- brain-metadata/src/tables/knowledge/statement.rs: new flag
  bit STATEMENT_FLAG_STALE_EXTRACTION = 1 << 3.
- brain-metadata/src/stale_ops.rs (new): is_stale predicate +
  scan_and_flag_batch over STATEMENTS_TABLE.
- brain-workers/src/workers/stale_detector.rs (new):
  StaleExtractionDetector running at Low priority, hourly
  default.
- brain-protocol/src/knowledge/statement_req.rs:
  StatementListRequest.stale_only: Option<bool>.
- brain-ops/src/ops/knowledge_statement.rs: apply stale_only
  filter in handle_statement_list.
- Tests: 5 unit (ops) + 1 unit (worker) + 1 E2E.

Verified: cargo zigbuild --target x86_64-unknown-linux-gnu
--workspace --tests; cargo clippy -- -D warnings;
cargo test -p brain-protocol --lib.
```

## 9. Confirmation

1. **Flag-only** (no auto re-extract) — re-extraction is 24.8.
2. **New flag bit** on `StatementRow.flags`; no new column.
3. **`STATEMENT_LIST.stale_only`** wire-shape extension — additive field, default `None`.
4. **Full-table scan, batched** — watermark optimisation deferred.
5. **Active rows only** — tombstoned statements never flagged.
