# `brain extract`

Inspect and steer the extractor pipeline — the three-tier (pattern →
classifier → LLM) workers that turn memory text into entities,
statements, relations, and mention edges. The verb covers `status`
and `backfill`.

**Knowledge layer prerequisite.** Extraction runs only when a schema
is declared via `SCHEMA_UPLOAD`. On a substrate-only deployment the
extractor pipeline doesn't execute and these subcommands have nothing
to report. See [`spec/11_extractors/00_purpose.md`](../../../../spec/11_extractors/00_purpose.md).

**Status.** Both subcommands today emit a `tracing` warning and then
`todo!()` — the wire ops aren't bound through the SDK. The flag
surfaces and target output shapes below are the contract scripts can
be written against. See [Gated features](#gated-features).

---

## `brain extract status`

Show the extraction-audit row(s) for one memory. Each row is one
extractor invocation against that memory — its outcome, latency,
tier, and any output references it produced. Backed by the
`ExtractionAuditGetReq` / `ExtractionAuditListByMemoryReq` wire ops
(see [`spec/11_extractors/04_audit.md`](../../../../spec/11_extractors/04_audit.md)).

```
brain extract status <MEMORY_ID>
```

### Arguments

`<MEMORY_ID>` — any of the three [`MemoryId` input forms](../output-formats.md#memory-ids):
short (`s2/m17/v1`), long hex (`0x…`), decimal `u128`.

### Output

#### Card (target shape)

```
audit  0190fa12-…  memory s2/m17/v1
  status     = Success
  tier       = classifier  (extractor=2, version=3, schema=1)
  started    = 1779153941479431250 unix-nanos (2026-05-15T09:25:41Z, ~12 s ago)
  completed  = 1779153941512104112 unix-nanos (2026-05-15T09:25:41Z, ~12 s ago)
  duration   = 32.7 ms
  cost       = 0 µUSD
  input_hash = b3a7…f01c

Outputs (3)
  · Entity      0190f8c0-…  Priya
  · Statement   0190f8d0-…  Priya works_at Acme Corp
  · Relation    0190f9a0-…  Priya --[works_at]--> Acme Corp
```

| Field | Meaning |
|---|---|
| `status` | `Success`, `Failure`, `SkippedBudget` (LLM only), `SkippedIdempotency`. Maps `ExtractionStatus` from [spec §11/05](../../../../spec/11_extractors/04_audit.md). |
| `tier` | Which extractor produced the row: `pattern`, `classifier`, or `llm`. |
| `(extractor=…)` | `extractor_id`, version, schema version — the keys that determine idempotency. |
| `duration` | `completed - started` in human form. |
| `cost` | Always `0` for pattern / classifier; populated for LLM rows (phase 21). |
| `input_hash` | BLAKE3 of `memory.text`, first 4 + last 4 hex chars. |
| `Outputs` | `OutputRefRow` rows: kind (`Entity` / `Statement` / `Relation` / `EntityMention`) + id. |

A memory with no audit rows renders `(no rows)` — the pipeline hasn't
run, or didn't find anything to extract. Use `extract backfill` to
re-run.

#### JSON (target shape)

```json
{ "memory_id": "0x000200000000000a00000001…",
  "audits": [
    { "audit_id": "0190fa12-…",
      "extractor_id": 2,
      "extractor_version": 3,
      "schema_version": 1,
      "status": "Success",
      "status_reason": "",
      "started_at_unix_nanos": 1779153941479431250,
      "completed_at_unix_nanos": 1779153941512104112,
      "cost_micro_usd": 0,
      "input_hash": "b3a7…f01c",
      "outputs": [
        { "kind": "Entity",    "id": "0190f8c0-…" },
        { "kind": "Statement", "id": "0190f8d0-…" }
      ] } ] }
```

### Examples

```bash
# Why didn't the LLM tier fire on this memory?
brain extract status s2/m17/v1

# Pluck the latest audit's status only
brain extract status s2/m17/v1 -o "jsonpath={.audits[0].status}"

# Cost roll-up across one memory's history
brain extract status s2/m17/v1 -o json \
  | jq '[.audits[].cost_micro_usd] | add'
```

---

## `brain extract backfill`

Re-run the extractor pipeline against memories that lack a successful
audit row. Operational verb — useful after a schema bump, after a
pipeline crash, or when a new extractor tier is enabled. Backed by
the `ExtractionBackfillReq` wire op.

```
brain extract backfill
        (--memory <ID> | --since <UNIX_NANOS> | --all)
```

### Flags

Exactly one of the three must be supplied — clap's argument-group
enforcement is `scope`.

#### `--memory <ID>`

Backfill a single memory. Accepts any of the three [`MemoryId` input
forms](../output-formats.md#memory-ids).

```bash
brain extract backfill --memory s2/m17/v1
```

#### `--since <UNIX_NANOS>`

Backfill every memory in this shard created at or after the given
unix-nanos timestamp. Pair with `subscribe` if you're filling a known
gap:

```bash
# Backfill everything written in the last hour
brain extract backfill --since $(($(date +%s%N) - 3600000000000))
```

#### `--all`

Backfill every memory that has no successful audit row. Heavy —
intended for ops, not interactive use. The server bounds concurrency
per [`spec/11_extractors/02_triggers.md`](../../../../spec/11_extractors/02_triggers.md);
the client doesn't need to throttle.

### Output

#### Card (target shape)

```
backfill accepted
  scope     = since unix_nanos=1779150341479431250
  selected  = 142 memories
  enqueued  = 142
  skipped   = 0  (idempotent — already have successful audit)
  job_id    = 0190fb01-…
```

Backfill is fire-and-forget at the wire layer. Follow the resulting
extractions via `brain subscribe` to watch `ExtractionCompleted` /
`ExtractionFailed` events as they fire.

#### JSON (target shape)

```json
{ "scope": "since unix_nanos=1779150341479431250",
  "selected_count": 142,
  "enqueued_count": 142,
  "skipped_count": 0,
  "job_id": "0190fb01-…" }
```

### Examples

```bash
# After a schema bump: re-extract everything
brain extract backfill --all

# Targeted re-run for a known-bad memory
brain extract backfill --memory s2/m17/v1

# Backfill + watch the results
brain extract backfill --since $(($(date +%s%N) - 3600000000000))
brain subscribe --collect 100 | jq 'select(.event_kind == "ExtractionCompleted")'
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `InvalidArgument` | Multiple `--memory` / `--since` / `--all` flags. | Clap rejects this at parse time. Drop the extras. |
| `Internal: requires --memory, --since, or --all` | None of the three passed. | Pass exactly one. |
| `InvalidArgument` | Bad `MemoryId` form. | Re-issue with a parseable id. |
| `Conflict` | `backfill --all` already running. | Wait for the prior job to drain (visible via `subscribe`). |
| `Overloaded` | Server shedding load. | Back off. |

Full catalogue: [`../errors.md`](../errors.md).

---

## Gated features

Both subcommands today emit a `tracing` warning and then `todo!()` —
no wire request is sent. The shell surface is locked in so scripts can
be written against the final shape; the bodies light up once the wire
ops land.

| Surface | Blocked on |
|---|---|
| `extract status` | Wire op `ExtractionAuditGet` / `ExtractionAuditListByMemory` + SDK builder. The redb tables (`extractor_audit`, `extractor_audit_by_memory`) exist already per [spec §11/05](../../../../spec/11_extractors/04_audit.md). |
| `extract backfill` | Wire op `ExtractionBackfillReq` + SDK builder. The admin-only `brain-cli` may grow this verb first (per the phase 22 plan); the shell follows. |
| Streaming progress | Once `ExtractionBackfillReq` is wired, surface a `subscribe`-style streaming option so `--all` reports live progress instead of fire-and-forget. |

---

## See also

- [`entity.md`](entity.md), [`statement.md`](statement.md), [`relation.md`](relation.md), [`mention.md`](mention.md) — the four output kinds an audit row references
- [`encode.md`](encode.md) — `--wait-for-extraction` waits for this pipeline to land per-memory
- [`subscribe.md`](subscribe.md) — `ExtractionCompleted` / `ExtractionFailed` events
- [`../output-formats.md`](../output-formats.md) — table + JSON
- Spec: [`spec/11_extractors/`](../../../../spec/11_extractors/00_purpose.md), [`spec/11_extractors/04_audit.md`](../../../../spec/11_extractors/04_audit.md)
