# 27.03 Sweeper Workers

Normative spec for the five **periodic low-priority sweepers**
introduced by §00 (rows for supersession sweeper, audit log
sweeper, cache sweeper, stale extraction detection, entity GC).
Implemented in phase 24:

- 24.3 — `SupersessionSweeper`.
- 24.4 — `StaleExtractionDetector`.
- 24.5 — `LlmCacheSweeper`.
- 24.6 — `EntityGcWorker` (off by default).
- 24.7 — `AuditLogSweeper`.

Cross-references:
- [`./00_purpose.md`](./00_purpose.md) — worker overview +
  scheduling priorities.
- [`../25_provenance_versioning/00_purpose.md`](../25_provenance_versioning/00_purpose.md)
  §"Retention" — the retention windows these sweepers enforce.
- [`../26_knowledge_storage/00_purpose.md`](../26_knowledge_storage/00_purpose.md)
  §"LLM cache" — the table layout for the LLM cache sweeper.

## 1. Shared discipline

All five sweepers obey the same invariants. Per-worker
sections below document only what differs from this baseline.

### 1.1 Priority and budget

Low priority lane per §00 §"Scheduling priorities and
budgets" (≤ 5% of shard time). The scheduler yields after
each batch.

### 1.2 Cadence

Clock-triggered. Configurable per worker via
`BRAIN_<NAME>_PERIOD_SECONDS`. The scheduler computes the
next tick from the previous tick's completion time; a slow
tick doesn't accumulate backlog beyond one cycle.

### 1.3 Bounded batches

Each tick performs at most one batch of `batch_cap` rows
(default 256 — varies per worker, documented per section).
Larger backlogs clear across multiple ticks. The batch cap
keeps a single redb `WriteTransaction` small enough that
foreground operations aren't blocked beyond ms-scale.

### 1.4 Dry-run mode

Every sweeper exposes a `dry_run: bool` config. When `true`,
the sweeper scans + counts but skips deletes. Returns a
`SweepSummary { scanned, deleted, dry_run_would_delete,
skipped_reasons }` shape the worker logs and exports as
metrics.

Phase 14's acceptance gate runs in dry-run by default;
operators flip to live mode once metrics confirm the
predicate is what they expect.

### 1.5 Metrics

Per-worker, prefixed `sweeper_*`:

- `sweeper_swept_total{worker, kind}` — counter of deletions
  (per kind where applicable; e.g. supersession sweeper
  distinguishes `statement` vs `relation`).
- `sweeper_skipped_total{worker, reason}` — counter of rows
  scanned-but-not-deleted, by reason (`within_retention`,
  `merge_log_exempt`, `still_referenced`, etc.).
- `sweeper_latency_seconds{worker}` — histogram of per-tick
  wall-time.
- `sweeper_scan_size{worker}` — gauge of last scan's row
  count.

### 1.6 Idempotency

Sweepers re-scan the relevant table from scratch each tick.
There is no persistent cursor; if a tick is interrupted
mid-batch, the next tick re-scans from the beginning. The
batch cap bounds wasted re-scan work.

### 1.7 Failure handling

Per-row failures (redb errors, audit-row write failures)
are logged at `warn` and the sweeper continues with the next
row. A failed tick returns `Err` to the scheduler, which
records the error and schedules the next tick normally —
sweepers are not fail-fatal.

### 1.8 Restart semantics

Sweepers are stateless. On shard restart they re-attach to
the scheduler and run their next tick at the configured
period.

## 2. Supersession sweeper

**Spec source:** §25/00 §"Retention" — superseded statements
+ relations are retained "Forever" by default; operators opt
in to sweeping.

**Cadence default:** daily
(`BRAIN_SUPERSESSION_SWEEPER_PERIOD_SECONDS = 86400`).

**Retention default:** `0` seconds — disabled. The worker
checks the config at the top of each tick; if zero, returns
`Ok(())` immediately.

**Batch cap default:** 256 rows.

**Scan procedure:**

```
for entry in STATEMENTS_TABLE.iter():
    if entry.superseded_by != [0; 16]
       AND now_ns - entry.superseded_at_ns >= retention_ns:
        delete; audit append AuditOp::Tombstoned
                with reason=SupersededRetentionExpired
    if collected == batch_cap: break

(same for RELATIONS_TABLE)
```

**Audit row:** one per deleted record, in the same wtxn as
the delete. The audit log sweeper (24.7) is responsible for
eventual audit retention of these rows.

**Failure mode:** any redb error aborts the current batch;
next tick retries from the beginning.

## 3. Audit log sweeper

**Spec source:** §25/00 §"Retention" — extraction +
resolution audit logs at 90 d default; merge logs forever.

**Cadence default:** daily
(`BRAIN_AUDIT_SWEEPER_PERIOD_SECONDS = 86400`).

**Retention default:** 90 d
(`BRAIN_AUDIT_RETENTION_SECONDS = 7_776_000`).

**Batch cap default:** 1024 rows (audit rows are small;
larger batches are cheap).

**Merge-log exemption:** rows with
`operation_tag ∈ { Merged, Unmerged }` are skipped
unconditionally. Spec §25/00 binding: merge logs are
"Forever (small, valuable)".

**Scan procedure:**

```
cutoff_ns = now_ns - retention_seconds * 1_000_000_000
for entry in AUDIT_TABLE.iter():
    if entry.timestamp_ns > cutoff_ns: skip
    if matches!(entry.operation_tag, Merged | Unmerged):
        skipped += 1 with reason=merge_log_exempt; continue
    delete
    if collected == batch_cap: break
```

UUIDv7 keys make the audit table time-ordered. A range-scan
optimisation
(`[`0..(cutoff_uuid)`]`) is deferred post-v1; full scan at
production volume is sub-second.

**No audit row** for sweeping the audit table — that would
defeat the purpose.

## 4. LLM cache sweeper

**Spec source:** §25/00 §"Retention" — LLM cache 90 d
default; §26/00 §"LLM cache" — table layout.

**Cadence default:** hourly
(`BRAIN_LLM_CACHE_SWEEPER_PERIOD_SECONDS = 3600`).

**Two passes per tick:**

1. **TTL expiry pass.** Drop rows with
   `created_at_ns <= now_ns - ttl_seconds * 1e9`.
2. **Capacity enforcement pass.** Sum total bytes of the
   `LLM_CACHE_TABLE`. If `> max_bytes`, evict by ascending
   `last_used_at_ns` (or `created_at_ns` — see v1 note) until
   under cap.

Both passes share a single `WriteTransaction` per tick.

**Defaults:**

- `BRAIN_LLM_CACHE_TTL_SECONDS = 7_776_000` (90 d).
- `BRAIN_LLM_CACHE_MAX_BYTES = 1_073_741_824` (1 GiB).
  `0` disables capacity enforcement.

**Batch cap default:** 1024 per pass.

**No-op when slot is None.** If `OpsContext.llm_cache` is
`None` (substrate-only deployment — no LLM extractors
configured), the worker returns `Ok(())` immediately.

**v1 note — `last_used_at`.** v1 does not refresh
`last_used_at_ns` on cache reads. Eviction order is
effectively by `created_at_ns`. Acceptable for the 90 d TTL
the cache uses; touch-on-read is a post-v1 optimisation.

**No audit row** for cache eviction — the cache is derived
state, not a system-of-record.

## 5. Stale extraction detector

**Spec source:** §25/00 §"Stale extraction detection" — flag
statements whose `schema_version` or `extractor_version` is
behind the current registry.

**Cadence default:** hourly
(`BRAIN_STALE_DETECTOR_PERIOD_SECONDS = 3600`).

**Batch cap default:** 512 rows.

**Predicate:**

```
fn is_stale(stmt, current_versions) -> bool {
    let ns = predicate_namespace_of(stmt.predicate_id);
    stmt.schema_version < current_versions.schema_for(ns)
    || stmt.extractor_version
       < current_versions.extractor_for(stmt.extractor_id)
}
```

`current_versions` is loaded once per tick by reading
`SCHEMA_ACTIVE_VERSIONS_TABLE` and the extractor registry,
then cached for the scan.

**Effect:** sets `STATEMENT_FLAG_STALE_EXTRACTION = 1 << 3`
on `StatementRow.flags`. **Does not re-extract.** The
schema-migration worker (24.8) is the side that re-extracts
when an operator triggers a migration plan.

**Transition counting:** `SweepSummary.flagged_now` counts
only `false → true` transitions (rows already flagged are
counted as `flagged_already` for observability).

**Scope:** active rows only. Tombstoned statements are
skipped.

**No audit row** for staleness flagging — the flag itself is
the audit signal; the schema-migration worker emits
`AuditOp::SchemaUpgraded` when re-extraction happens.

## 6. Entity GC worker

**Spec source:** §18 entity lifecycle + §25/00 §"Retention".

**Cadence default:** daily
(`BRAIN_ENTITY_GC_PERIOD_SECONDS = 86400`).

**Off by default** (`BRAIN_ENTITY_GC_ENABLED = false`). The
worker checks the flag at the top of each tick; if false,
returns `Ok(())` immediately and logs `debug` once at
startup.

**Grace period default:** 30 d
(`BRAIN_ENTITY_GC_GRACE_SECONDS = 2_592_000`).

**Eligibility predicate:**

```
entity.tombstoned == false
AND now_ns - entity.created_at_ns >= grace_seconds * 1e9
AND inbound_reference_count(entity_id) == 0
```

where `inbound_reference_count` sums:
- Active statements where `subject = entity_id`.
- Active relations where `from = entity_id`.
- Active relations where `to = entity_id`.
- `entity_mentions` rows pointing at the entity.

Tombstoned-but-within-grace inbound rows count as references
(prevents flapping; they may still be reverted).

**Race-safety:** the worker collects eligible ids under a
`ReadTransaction`, then opens a `WriteTransaction` and
**re-checks** each candidate's inbound count under the wtxn
before tombstoning. A concurrent statement write between
the rtxn and wtxn is caught (the wtxn is the redb single
writer; the re-check sees the new state).

**Reversal on inbound reference.** When a statement /
relation / entity_mention is created targeting an
entity-tombstoned-within-grace, the entity ops layer clears
the tombstone flag and writes an `AuditOp::Restored` row
(implementation lives in entity_ops, documented here as the
contract).

**Audit row:** one `AuditOp::Tombstoned` per swept entity,
reason `EntityGcEligible`.

**Hard delete:** the GC worker only tombstones. Eventual
hard-reclamation rides on the substrate's existing
tombstone-grace-then-reclaim flow.

## 7. Configuration summary

| Env var | Default | Worker |
|---|---|---|
| `BRAIN_SUPERSESSION_RETENTION_SECONDS` | `0` (disabled) | 24.3 |
| `BRAIN_SUPERSESSION_SWEEPER_PERIOD_SECONDS` | `86400` | 24.3 |
| `BRAIN_AUDIT_RETENTION_SECONDS` | `7_776_000` (90 d) | 24.7 |
| `BRAIN_AUDIT_SWEEPER_PERIOD_SECONDS` | `86400` | 24.7 |
| `BRAIN_AUDIT_SWEEPER_BATCH_CAP` | `1024` | 24.7 |
| `BRAIN_LLM_CACHE_TTL_SECONDS` | `7_776_000` (90 d) | 24.5 |
| `BRAIN_LLM_CACHE_MAX_BYTES` | `1073741824` (1 GiB) | 24.5 |
| `BRAIN_LLM_CACHE_SWEEPER_PERIOD_SECONDS` | `3600` | 24.5 |
| `BRAIN_STALE_DETECTOR_PERIOD_SECONDS` | `3600` | 24.4 |
| `BRAIN_ENTITY_GC_ENABLED` | `false` | 24.6 |
| `BRAIN_ENTITY_GC_GRACE_SECONDS` | `2_592_000` (30 d) | 24.6 |
| `BRAIN_ENTITY_GC_PERIOD_SECONDS` | `86400` | 24.6 |

All values are operator-overridable. Tests use small values
(seconds, not days) to keep wall-time bounded.

## 8. Open questions

Tracked in [`./07_open_questions.md`](./07_open_questions.md)
and [`../30_knowledge_open_questions/00_purpose.md`](../30_knowledge_open_questions/00_purpose.md):

- Secondary indexes on `last_used_at_ns` (LLM cache) and
  `timestamp_ns` (audit table) for faster scans.
- Touch-on-read for `last_used_at_ns` to make LLM cache LRU
  exact.
- Per-statement-kind retention windows.
- Watermark optimisation for stale extraction (skip
  re-flagging already-flagged rows).
