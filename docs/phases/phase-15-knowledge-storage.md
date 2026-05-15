# Phase 15: Storage Extensions

## Goal

Add the knowledge-layer redb table definitions, new on-disk artifacts (tantivy directories, statement HNSW, entity HNSW, LLM cache), and the knowledge-layer WAL frame types. After this phase, the binary boots against an existing substrate data directory, exposes empty knowledge-layer tables, and substrate primitives behave as before.

## Prerequisites

- Substrate phases 1 through 14 complete (substrate at v0.x release-candidate).

## Reading list

- `spec/26_knowledge_storage/00_purpose.md` — the full storage layout.
- `spec/17_knowledge_model/00_purpose.md` — three-layer model + schema-optional semantics.
- `AUTONOMY.md` — knowledge-layer rules (schema-optional regression is binding).
- `spec/05_storage_arena_wal/` and `spec/07_metadata_graph/` — substrate storage primitives.

## Outputs

- New redb table schemas in `brain-metadata`.
- New WAL frame types in `brain-storage`.
- Directory layout for new artifacts (tantivy, statement.hnsw, entity.hnsw).
- A schema-declared flag (set by SCHEMA_UPLOAD; default off).
- The binary boots, knowledge tables initialize empty, substrate traffic unaffected.

## Sub-tasks

### 15.1 Define new redb tables in `brain-metadata`

**Reads:** `26_knowledge_storage/00_purpose.md` (the table list).
**Writes:** `crates/brain-metadata/src/tables/knowledge.rs`.
**Done when:** all 25 knowledge-layer redb tables compile with correct key/value type signatures.
**Pitfalls:** Don't import any knowledge-layer *behavior* yet — only types. Keep this module isolated so substrate code is unaffected.

### 15.2 Add knowledge-layer WAL frame type discriminator

**Reads:** `26_knowledge_storage/00_purpose.md` (WAL extensions section); `spec/05_storage_arena_wal/`.
**Writes:** `crates/brain-storage/src/wal/frame.rs`.
**Done when:** WAL writer accepts new frame types (placeholders, write-noop), reader recognizes them, substrate frame parsing remains intact.
**Pitfalls:** Don't increment WAL version number; new frame types are additive. CRC computation must include the new type byte.

### 15.3 New on-disk artifact paths

**Reads:** `26_knowledge_storage/00_purpose.md` (shard layout).
**Writes:** `crates/brain-storage/src/layout.rs`.
**Done when:** `Shard::open()` creates new directories (`statements.tantivy/`, etc.) if missing; doesn't disturb existing substrate files.
**Pitfalls:** mkdir-p semantics; existing substrate shards must still open.

### 15.4 LLM cache redb file

**Reads:** `26_knowledge_storage/00_purpose.md` (LLM cache section).
**Writes:** `crates/brain-metadata/src/llm_cache.rs`.
**Done when:** separate redb file per shard, opened on `Shard::open()`, two tables initialized.
**Pitfalls:** Keep this file separate from `metadata.redb` to avoid bloating the hot metadata file with LLM blobs.

### 15.5 knowledge-mode server config flag

**Reads:** `spec/17_knowledge_model/00_purpose.md` (schema-optional behavior); `AUTONOMY.md` (substrate-only regression is binding).
**Writes:** `crates/brain-server/src/config.rs`.
**Done when:** `BRAIN_KNOWLEDGE_ENABLED=true` env var (default false); when false, knowledge tables exist but no knowledge-layer workers spawn; when true, knowledge-layer workers spawn (idle until schema declared).
**Pitfalls:** Dont gate knowledge-layer wire opcodes on this flag; gate worker activation only.

### 15.6 substrate-only mode regression test

**Reads:** `spec/16_benchmarks_acceptance/`.
**Writes:** `tests/knowledge_compat.rs`.
**Done when:** all substrate acceptance tests pass when run with knowledge mode disabled. P50/P99 ENCODE and RECALL latencies within 110% of substrate-only baseline.
**Pitfalls:** Run on substrate reference data; check tail latencies, not just averages.

## Done-when (phase)

- the binary boots against a data directories from substrate-only deployments.
- All substrate acceptance tests pass.
- knowledge-layer redb tables are empty.
- knowledge-layer workers are not running (no schema).
- WAL contains no knowledge-layer frames (no knowledge-layer writes happen).
- New disk artifact directories exist but are empty.

## Pitfalls

- Don't add behavior in this phase. Only structure. Each subsequent phase adds behavior.
- Test substrate-only mode *at the end* of the phase, not after each sub-task. Some sub-tasks (especially the WAL frame extension) only show compatibility at integration time.
- Avoid pulling in tantivy or new HNSW crates in this phase. Defer to phases 22 (tantivy) and 16 (entity HNSW).
