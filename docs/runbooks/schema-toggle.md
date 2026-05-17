# Runbook — Schema toggle (declare / migrate / revert)

When to use this runbook:

- You're running Brain in **substrate-only mode** (no schema
  declared) and want to enable the knowledge layer.
- You want to **migrate an existing schema** to a new version.
- You want to **revert** a schema-declared deployment to
  substrate-only behaviour.

Phase 24 ships the workers and wire surface this runbook drives.
For background see [`spec/28_knowledge_wire_protocol/08_schema_optional_mode.md`](../../spec/28_knowledge_wire_protocol/08_schema_optional_mode.md).

---

## Prerequisites

- `brain-server` reachable.
- Schema file (`my-schema.brain`) reviewed against
  [`spec/21_schema_dsl/00_purpose.md`](../../spec/21_schema_dsl/00_purpose.md).
- LLM API keys configured in `brain-server`'s env if any
  declared extractors use the LLM tier (phase 21).
- A maintenance window — the backfill consumes shard time.

## Pre-flight checklist

- [ ] Backup the data directory (`tar -czf …`).
- [ ] `brain-cli admin stats` reports healthy.
- [ ] LLM cost budget reviewed.

## Step 1 — Validate the schema (dry-run)

```bash
brain-cli schema validate --file my-schema.brain
```

Expected: 0 errors. Address every reported error before
proceeding. Common ones: unresolved type references, conflicting
predicate kinds, missing extractor definitions.

## Step 2 — Declare the schema

```bash
brain-cli schema upload --file my-schema.brain
```

Expected response:

```text
namespace: acme
schema_version: 1
validation_errors: []
migration_summary: { total_items: 0 }
```

What happens server-side (spec §28/08 §1):

- The per-shard `SchemaGate` flips from `false` to `true`.
- Substrate RECALL now routes through the hybrid pipeline
  (semantic + lexical + graph; spec §28/08 §5).
- Knowledge-layer opcodes (entity / statement / relation /
  query) start accepting requests.

## Step 3 — Backfill existing memories

```bash
brain-cli admin backfill \
    --extractors all \
    --memory-range all
```

The backfill walks every memory × every enabled extractor,
checkpointing each pair under the shared `worker_checkpoints`
table.

Useful flags:

- `--dry-run` — print the plan + count without invoking
  extractors. Always start here.
- `--extractor pattern,classifier` — limit to specific
  tiers.
- `--memory-range start..end` — partial backfill by id.
- `--priority background|low` — override the default
  Background lane.

> **v1 limitation:** memory text is not persisted beyond the
> WAL. Live backfill marks each item `Failed` with reason
> `"memory text not persisted (v1 limitation)"`. Dry-run is
> fully functional for plan preview. Operators re-ingest from
> their own source-of-truth in v1; full content-aware
> backfill lands in a post-v1 enhancement.

If a backfill is interrupted (Ctrl-C / server restart), rerun
the same command. The worker re-attaches to the
`worker_checkpoints` table and resumes from the first
non-`Completed` row.

## Step 4 — Verify

```bash
# Hybrid query — contributing_retrievers populated.
brain-cli query "test cue"

# Statements created by the backfill.
brain-cli statement list --subject <entity-id>
```

`contributing_retrievers` is empty on substrate-only
deployments and populated on schema-declared ones. Spec
§28/08 §5.

## Step 5 — Migrating an existing schema

```bash
brain-cli schema upload --file my-schema-v2.brain
```

Server-side, the SCHEMA_UPLOAD handler computes a
`MigrationPlan` listing affected `(memory, extractor)` pairs
under the new version. The response includes
`migration_summary.total_items` so you can see the size of
the migration.

If `--dry-run` is set, the plan is returned but the migration
worker is **not** enqueued. Drop the flag to commit.

> **Same v1 limitation as backfill** applies to live
> migration: items marked `Failed` with the
> memory-text-not-persisted reason. Operators re-ingest.

## Step 6 — Reverting (substrate-only fallback)

There is **no `SCHEMA_DROP` opcode in v1** (spec §28/08 §1).
To return to substrate-only behaviour:

### Option A — stop using knowledge-layer ops

- Substrate primitives (ENCODE / RECALL / etc.) continue
  unchanged.
- Knowledge-layer data stays on disk, unused.
- `client.query()` keeps working; it's a service offering
  that callers can simply ignore.

### Option B — empty the active-schema pointer

Destructive of the schema **pointer** (not the data):

1. Stop the server.
2. Open the metadata redb file with `redb-cli` (or write a
   small tool against `brain-metadata`).
3. Remove the namespace row from
   `schema_active_versions`.
4. Restart the server. The per-shard `SchemaGate` re-seeds
   from metadata at startup and reads empty → `false`. The
   hybrid path is disabled; substrate RECALL is plain
   semantic.

A redb-cli wrapper for this is tracked as a post-v1
operator-tooling task.

---

## Pitfalls

- A backfill against millions of memories that include LLM
  extractors can cost real LLM API money. **Always
  `--dry-run` first.**
- The first RECALL after a gate flip incurs a one-time
  retriever initialisation (HNSW cache warmup, etc.).
  Subsequent calls are fast.
- Migration runs cooperatively under the Background lane
  (20 % shard time per §27/00). Heavy migrations are slow
  but don't starve RECALL.

## Recovery

- **Backfill stuck**:
  `brain-cli admin backfill cancel --request-id <id>`; rerun.
- **Migration stuck**:
  `brain-cli admin schema migration cancel --request-id <id>`.
- **SCHEMA_UPLOAD failed mid-flight**: redb is transactional.
  Either the upload landed or it didn't. Check
  `brain-cli schema list` to see the active version.
- **Suspicious failure rate**: a backfill that exceeds
  50 % failure across its first 100 items aborts
  automatically. Inspect the operator metric
  `backfill_failure_rate_high_total` and the worker logs.

---

## See also

- [`spec/28_knowledge_wire_protocol/08_schema_optional_mode.md`](../../spec/28_knowledge_wire_protocol/08_schema_optional_mode.md) — wire-level state machine.
- [`spec/27_knowledge_workers/04_state_carrying_workers.md`](../../spec/27_knowledge_workers/04_state_carrying_workers.md) — backfill + migration worker mechanics.
- [`spec/25_provenance_versioning/00_purpose.md`](../../spec/25_provenance_versioning/00_purpose.md) — re-extraction semantics.
