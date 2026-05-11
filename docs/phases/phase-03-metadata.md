# Phase 3 — Metadata + Graph (redb)

## Goal

Implement the `redb`-backed metadata store: agents, contexts, memory metadata, edges, idempotency cache, and the durable LSN checkpoint. Wire it into recovery so that storage and metadata stay consistent across crashes.

## Prerequisites

- [x] Phase 2 complete (`phase-2-complete` tag).
- `MetadataSink` trait exists from Phase 2.10.

## Reading list

1. [`spec/07_metadata_graph/00_purpose.md`](../../spec/07_metadata_graph/00_purpose.md)
2. [`spec/07_metadata_graph/01_redb_choice.md`](../../spec/07_metadata_graph/01_redb_choice.md)
3. [`spec/07_metadata_graph/02_table_layout.md`](../../spec/07_metadata_graph/02_table_layout.md) — **all 13 tables.**
4. [`spec/07_metadata_graph/03_memory_table.md`](../../spec/07_metadata_graph/03_memory_table.md)
5. [`spec/07_metadata_graph/04_edge_storage.md`](../../spec/07_metadata_graph/04_edge_storage.md)
6. [`spec/07_metadata_graph/05_context_table.md`](../../spec/07_metadata_graph/05_context_table.md)
7. [`spec/07_metadata_graph/06_idempotency.md`](../../spec/07_metadata_graph/06_idempotency.md) — 24h TTL.
8. [`spec/07_metadata_graph/07_text_storage.md`](../../spec/07_metadata_graph/07_text_storage.md)
9. [`spec/07_metadata_graph/08_transactions.md`](../../spec/07_metadata_graph/08_transactions.md)

## Outputs

- `crates/brain-metadata` exports `MetadataDb`, table definitions, and an implementation of `MetadataSink` from Phase 2.
- Schema versioning header.
- Tag: `phase-3-complete`.

## Sub-tasks

### Task 3.1 — Schema versioning header ✅
**Reads:** `spec/02_data_model/09_schema_evolution.md`, `spec/07_metadata_graph/02_table_layout.md` §6.
**Writes:** `crates/brain-metadata/Cargo.toml` (real deps), `crates/brain-metadata/src/lib.rs` (real skeleton), `crates/brain-metadata/src/schema.rs` (new). Also bumped workspace `redb = "2"` → `"4"` (v4.1.0 picked up).
**What was built:**
- `CURRENT_SCHEMA_VERSION: u32 = 1`, `SCHEMA_META_TABLE: TableDefinition<&str, u32>` keyed by `"schema_version"`.
- `open_or_init_schema(&Database) -> Result<u32, SchemaError>`. Fresh DB → writes v1. Same version → returns it. Older version → returns it (placeholder for v1.1+ migration registry). Newer version → `SchemaVersionTooNew`.
- **Single global version row instead of per-table versions.** Spec §07/02 §6 reads "each table has a format version"; we use one global row covering the whole metadata file. The 13 tables co-evolve from the same crate; per-table machinery (13× the open-time checks + migration registry entries) adds bookkeeping for no benefit at v1. Documented inline in the module doc.
- Tests gated `#[cfg(all(test, not(miri)))]` for consistency with Phase 2 (redb uses mmap internally).
**Done when:** [x] `__schema_meta` records `schema_version=1` and refuses to open mismatched versions — `future_version_refuses_to_open` covers the rejection path; `fresh_db_initializes_at_v1`, `reopen_reads_existing_version`, `idempotent_reinit_returns_same_version`, `table_present_but_row_missing_initializes_to_v1` cover the rest. 5 tests.

### Task 3.2 — Memory metadata table ✅
**Reads:** `spec/07_metadata_graph/03_memory_table.md`
**Writes:** `crates/brain-metadata/src/tables/memory.rs` (and `tables/mod.rs` + `lib.rs` `pub mod tables;`).

**What was built:**
- `MemoryMetadata` — 20-field struct (~140 B/row) per spec §07/03 §1. Stores brain-core types as byte representations (`[u8; 16]` for `MemoryId`/`AgentId`, `u64` for `ContextId`, `u8` for `MemoryKind`); typed getters convert at the API boundary.
- `MEMORIES_TABLE: TableDefinition<[u8; 16], MemoryMetadata>` keyed by `MemoryId::to_be_bytes()`.
- `redb::Value` impl backed by rkyv 0.7 with `#[archive(check_bytes)]` validation. Deserialize-on-read (owned `MemoryMetadata`); zero-copy view deferred to a profiling-driven follow-up.
- `flags` module — `ACTIVE`, `HARD_FORGOTTEN`, `PINNED`, `STALE`, `RESERVED_MASK` per §07/03 §2.7.
- `MemoryKind` ↔ `u8` mapping duplicated locally from `brain_storage::wal::payload`; note to promote to brain-core if a third caller appears.
- `MemoryMetadata::new_active(...)` constructor; `is_active`/`is_pinned`/etc. flag accessors; `set_flag(mask, on)`.

**Patterns set for the rest of Phase 3:**
- Byte-array key encoding (`[u8; 16]` for ID types).
- rkyv-backed `redb::Value` via deserialize-on-read with `expect`-on-corrupt.
- `type_name` includes `::v1` for type-confused mismatch detection.
- Module per table under `tables/`.

**Done when:** [x] Insert/get/scan-by-(agent, context)/delete round-trip tests pass. Plus update, missing-key, Option round-trip, flag manipulation, brain-core type round-trip, encoding stability. **9 tests.**

### Task 3.3 — Agents and contexts tables ✅
**Reads:** `spec/07_metadata_graph/05_context_table.md`, `02_table_layout.md` §12.
**Writes:** `crates/brain-metadata/src/tables/agent.rs`, `tables/context.rs`.

**What was built (4 of the 13 tables):**
- `AGENTS_TABLE: TableDefinition<[u8; 16], AgentMetadata>` — `AgentMetadata` carries `display_name`, `created_at`, `last_active_at`, denormalized `memory_count`/`context_count`. v1 defers "configuration overrides" from spec §07/02 §12 (field-addition follow-up via spec §02/09 §2).
- `CONTEXTS_TABLE: TableDefinition<u64, ContextMetadata>` — `ContextMetadata` per spec §07/05 §2.1 (8 fields including `Vec<String> tags`).
- `CONTEXT_NAMES_TABLE: TableDefinition<(&[u8; 16], &str), u64>` — name index for agent-scoped lookup.
- `AGENT_CONTEXTS_TABLE: TableDefinition<([u8; 16], u64), ()>` — agent→[context_ids] membership, supports prefix range scan.

**Composite keys via redb v4's tuple `Key` impl.** Worked out of the box — no fallback to manual byte concatenation needed. Fixed-width agent_id prefix means range scans by agent are clean prefix scans.

**Helper constants:** `RESERVED_NAME_PREFIX = "_"` and `DEFAULT_CONTEXT_NAME = "_default"` per spec §07/05 §6. Writer-task (Phase 9) enforces the reservation against client input; storage doesn't validate.

**Done when:** [x] Both tables CRUD-tested. 10 tests covering agent insert/update/delete/typed-getter, context insert by ID, name-index lookup with hit/miss, agent-prefix range scan, cross-agent name isolation (spec §07/05 §13), and `Vec<String>` + `Option<String>` rkyv round-trip.

### Task 3.4 — Edge storage ✅
**Reads:** `spec/07_metadata_graph/04_edge_storage.md`, `spec/02_data_model/06_edges.md`.
**Writes:** `crates/brain-metadata/src/tables/edge.rs`.

**What was built (2 more tables — 7 of 13):**
- `EDGES_OUT_TABLE: TableDefinition<EdgeKey, EdgeData>` keyed by `(source, kind, target)`.
- `EDGES_IN_TABLE: TableDefinition<EdgeKey, EdgeData>` keyed by `(target, kind, source)`.
- `EdgeData` (rkyv: weight, origin, derived_by, created_at, annotation).
- `link` / `unlink` / `list_edges_from` / `list_edges_to` helpers — take pre-opened table handles to avoid redb's "table already open" error.
- **Symmetric edge handling.** `SimilarTo` and `Contradicts` write 4 rows (direct + reverse-index + mirror + mirror-reverse). Self-symmetric edges skip the mirror (would be redundant).
- Byte-mapping constant modules: `origin::{EXPLICIT, AUTO_DERIVED}` and `derived_by::{CLIENT, CONSOLIDATION_WORKER, SIMILARITY_WORKER}`.

**Done when:** [x] LINK / UNLINK / list-edges-from / list-edges-to all work; symmetric edges stored both directions. 12 tests covering EdgeData round-trip, asymmetric and symmetric link/unlink, self-symmetric (2 rows not 4), range queries with and without kind filter on both tables, list-edges-to picking up symmetric mirror, and update-via-relink.

**Mid-flight bug found and fixed across all tables.** rkyv 0.7's `from_bytes` requires 8-byte-aligned input; redb returns bytes at arbitrary alignment. `MemoryMetadata`'s 3.2 tests happened to pass by luck of alignment; `EdgeData` failed deterministically with `Underaligned { expected_align: 8, actual_align: 1 }`. Fix: copy into `rkyv::AlignedVec` before `from_bytes` in each `redb::Value` impl. Applied to `MemoryMetadata`, `AgentMetadata`, `ContextMetadata`, and `EdgeData` (preemptive).

### Task 3.5 — Idempotency table with TTL ✅
**Reads:** `spec/07_metadata_graph/06_idempotency.md`
**Writes:** `crates/brain-metadata/src/tables/idempotency.rs` (new), `crates/brain-metadata/src/tables/mod.rs` (add `pub mod idempotency;`), `docs/spec-deviations.md` (SD-3.5-1).

**What was built (1 more table — 8 of 13):**
- `IDEMPOTENCY_TABLE: TableDefinition<[u8; 16], IdempotencyEntry>` — keyed by `RequestId::to_be_bytes()` (16-byte UUIDv7).
- `IdempotencyEntry` — rkyv-derived: `response_kind: u8`, `memory_id_bytes: Option<[u8; 16]>`, `response_payload: Vec<u8>`, `request_hash: [u8; 32]`, `created_at_unix_nanos: u64`. The fifth field (`request_hash`) is **SD-3.5-1** — needed for spec §5's conflict detection in O(1) byte compare; canonical-request bytes aren't reversible from the response payload.
- `response_kind` byte module: `UNKNOWN=0, ENCODE=1, FORGET=2, LINK=3, UNLINK=4, UPDATE_KIND=5, UPDATE_CONTEXT=6, TXN_BEGIN=7, TXN_COMMIT=8` per spec §17. Same 4th-occurrence-of-u8-mapping pattern; still deferred to the brain-core promotion bundle.
- `DEFAULT_TTL_NANOS = 24h` per spec §6.
- `prune_expired(table, now_unix_nanos, ttl_nanos) -> Result<u64, StorageError>` — pure function; collects victims via `iter()`, then `remove`s. Saturating arithmetic on `created_at + ttl_nanos` so `u64::MAX` doesn't wrap.
- `IdempotencyEntry::memory_id()` typed getter at the API boundary.

**Done when:** [x] 11 tests covering CRUD, missing-key, update, `Option<MemoryId>` round-trip, 256-byte payload round-trip, `request_hash` byte compare, prune-removes-old, prune-keeps-fresh, prune-mixed (3-old + 2-fresh), prune-saturating (entry at `u64::MAX`), and `type_name` v1-marker guard. Total in brain-metadata: 47 tests.

### Task 3.6 — Text blob storage ✅
**Reads:** `spec/07_metadata_graph/07_text_storage.md`
**Writes:** `crates/brain-metadata/src/tables/text.rs` (new), `crates/brain-metadata/src/tables/mod.rs` (add `pub mod text;`).

**What was built (1 more table — 9 of 13):**
- `TEXTS_TABLE: TableDefinition<[u8; 16], &[u8]>` — keyed by `MemoryId::to_be_bytes()`, valued by redb's **built-in** `&[u8]` variable-length type. No rkyv wrapper: there's no struct to evolve, and routing reads through rkyv would add the alignment-copy workaround for zero benefit.
- Whole file is one `pub const` plus tests — every other concern lives above this layer.

**Out-of-scope, all deliberate (per spec):**
- **UTF-8 validation** (spec §5) — wire layer (Phase 4).
- **`max_text_bytes` size limit** (spec §4, §7) — wire layer.
- **Immutability enforcement** (spec §8) — application invariant; ENCODE is the only insert path.
- **Hard-forget secure-erase** (spec §9) — needs `FALLOC_FL_PUNCH_HOLE` below redb's API. Phase 8 worker territory.
- **Same-transaction coupling with `memories`** (spec §15) — `MetadataDb` (sub-task 3.10) composes both inside one `begin_write()`.
- **Compression** — the "Optional compression per spec" line in the phase doc's original `Done when` was a phase-doc artifact; spec §07/07 doesn't mention compression anywhere.

**Done when:** [x] 8 tests covering CRUD, missing-key, overwrite-replaces-bytes, empty `b""` round-trip, 1 MB round-trip (the spec's default `max_text_bytes` ceiling), multi-byte UTF-8 round-trip including the `std::str::from_utf8` re-decode sanity, and iterate-all-entries. Total in brain-metadata: 55 tests.

### Task 3.7 — `slot_versions` table ✅
**Reads:** `spec/07_metadata_graph/02_table_layout.md` §13; `spec/05_storage_arena_wal/07_write_path.md` §2.3; `spec/02_data_model/03_identifiers.md` §2.1.
**Writes:** `crates/brain-metadata/src/tables/slot_version.rs` (new), `crates/brain-metadata/src/tables/mod.rs` (add `pub mod slot_version;`).

**Realignment.** The phase doc originally framed 3.7 as a "Tombstone table" with value `(memory_id, tombstoned_at, grace_until)`. **That table is not in the spec catalog (§07/02 §1).** Tombstone *state* lives as `flags & HARD_FORGOTTEN` + `forgot_at_unix_nanos` on the existing `memories` row from 3.2; the reclaim worker scans memories for `forgot_at + grace < now` per spec §09/06 §16. The actual reclaim-related table in the spec catalog is `slot_versions`, and that's what this sub-task implements. No new SD entry — the realignment is *to* the spec, not away from it.

**What was built (1 more table — 10 of 13):**
- `SLOT_VERSIONS_TABLE: TableDefinition<u64, u32>` — keyed by `slot_id` (48-bit logical, stored as `u64` per spec catalog), valued by 32-bit version. redb's built-in scalar `Value`s — no rkyv wrapper.
- `increment(&mut Table, slot_id) -> Result<u32, SlotVersionError>` — read-modify-write inside the caller's redb transaction. Missing row → returns 1 (spec §05/07 §2.3: never-used slot starts at 1). Existing → returns N+1. `u32::MAX` → returns `SlotVersionError::Exhausted { slot_id }` and does **not** write (fail-stop; silent wrap would violate spec §02/03 §2.3's MemoryId-stability invariant).
- `SlotVersionError` — `Storage(redb::StorageError)` + `Exhausted { slot_id }`. Derives only `Debug` + `thiserror::Error` (redb::StorageError doesn't impl Clone/Copy/PartialEq — same constraint hit in 3.4).

**Done when:** [x] 8 tests covering missing→1, existing→N+1, monotonic across 10 calls, two-slot independence (3 + 5 increments), **u32::MAX overflow returns Exhausted with no wrap-to-zero** (catastrophic-failure pin), direct insert/get, range scan returns u64 keys in numerical order (50/100/200), and missing-get returns None. Total in brain-metadata: 63 tests.

### Task 3.8 — `model_fingerprints` + `next_lsn` tables ✅
**Reads:** `spec/04_embedding_layer/07_fingerprinting.md` §8 (model_fingerprints shape); `spec/07_metadata_graph/02_table_layout.md` §1 rows 10, 12 + §7 (singleton convention).
**Writes:** `crates/brain-metadata/src/tables/model_fingerprint.rs` (new), `crates/brain-metadata/src/tables/next_lsn.rs` (new), `crates/brain-metadata/src/tables/mod.rs` (add both modules).

**Realignment.** The phase doc originally titled 3.8 "Counters and statistics" with `Done when: Per-shard counters (memory count, edge count, etc.) reconcile from full scans.` That's not a stored table — it's a derivation, and the denormalized count fields it would feed (`AgentMetadata.memory_count`, `ContextMetadata.memory_count`) already exist on row types from 3.3. The spec catalog has two unaccounted-for tables in Phase 3's budget: `model_fingerprints` and `next_lsn`. 3.8 bundles both. Reconcile-from-scans logic, when needed, lands in `MetadataDb` (3.10) or Phase 8 worker — not as a storage primitive. No new SD entry (realignment is *to* the spec).

**What was built (2 more tables — 12 of 13):**
- `MODEL_FINGERPRINTS_TABLE: TableDefinition<[u8; 16], ModelInfo>` — keyed by the 16-byte fingerprint (spec §04/07 §2: BLAKE3 truncation over the model's config/tokenizer/weights/substrate-config). `ModelInfo { model_name: String, seen_at_unix_nanos: u64, memory_count_at_fingerprint: u64 }`, rkyv-derived with the established `::v1` type_name + AlignedVec workaround. `ModelInfo::new` constructor.
- `NEXT_LSN_TABLE: TableDefinition<(), u64>` — singleton per spec §07/02 §7. No helper functions; `t.insert(&(), &v)` / `t.get(&())` is what the spec prescribes.

**Done when:** [x] 10 tests across both files. model_fingerprint (6): insert/get, long `String` variable-length round-trip (exercises the AlignedVec fix), update-overwrites, missing-key, multiple-fingerprints-coexist, type_name v1 marker. next_lsn (4): singleton CRUD round-trip, update-overwrites, missing returns None, `()` key sanity (guards spec §07/02 §7's prescription). Total in brain-metadata: 73 tests.

**Mid-flight observation.** `ReadableTable` import is needed for `.get()` on `&mut Table` (write txns, e.g. inside `unit_key_round_trips`) but **not** on `ReadOnlyTable` from read txns (inherent method in redb v4). Updated next_lsn.rs's test imports accordingly; model_fingerprint.rs's tests only read via ReadOnlyTable and don't need it. Clippy caught the unused import.

### Task 3.9 — `checkpoints` table ✅
**Reads:** `spec/05_storage_arena_wal/09_checkpointing.md` §2 (full struct + table shape); `spec/07_metadata_graph/02_table_layout.md` §1 row 11.
**Writes:** `crates/brain-metadata/src/tables/checkpoint.rs` (new), `crates/brain-metadata/src/tables/mod.rs` (add `pub mod checkpoint;`).

**What was built (the 13th and final spec'd table — 13 of 13):**
- `CHECKPOINTS_TABLE: TableDefinition<u64, CheckpointMeta>` — keyed by `checkpoint_id` (monotonic per spec §2).
- `CheckpointMeta` — rkyv-derived row with the six u64 fields spec §05/09 §2 prescribes: `checkpoint_id`, `durable_lsn`, `arena_capacity_at_checkpoint`, `metadata_version_at_checkpoint`, `started_at_unix_nanos`, `completed_at_unix_nanos`. Time fields suffixed `_unix_nanos` per the established 3.x convention. `CheckpointMeta::new` constructor.
- `latest(&ReadOnlyTable) -> Result<Option<CheckpointMeta>, StorageError>` — returns the row with the highest `checkpoint_id` (the recovery target per spec §2) in O(log N) via `iter().next_back()`. Returns `None` on empty.
- Name choice: `CheckpointMeta` rather than `Checkpoint` (collides with brain-storage's WAL `Checkpoint` opcode) or the spec catalog's `CheckpointInfo` (inconsistent with this crate's row-naming pattern).

**Out of scope (composition):**
- `From<&CheckpointReport>` conversion from brain-storage's 2.12 type → 3.11 (`MetadataSink::apply(CheckpointEnd)`) where the schema-version source is in scope.
- Retention sweep (delete checkpoints older than the recovery target) — Phase 8 worker per spec §05/09 §6.
- Recovery handshake (read `latest()`, replay WAL after `durable_lsn`) — 3.11.

**Mid-flight observation.** Clippy's `doc-lazy-continuation` lint fired on a wrapped paragraph in the module docstring. Restructured the spec references as an explicit bullet list — readable in rustdoc and lint-clean.

**Done when:** [x] 9 tests: CRUD, all-fields-spot-check round-trip (catches silent field reorder), update-overwrites, missing-key, multiple-checkpoints-coexist, **`latest_returns_max_id`** with out-of-order inserts (the recovery-target + u64-ordering pin), `latest_returns_none_on_empty`, `latest_after_update`, type_name v1 marker. Total in brain-metadata: 82 tests.

**🎯 Phase 3 spec-catalog tables: 13 of 13.** Remaining sub-tasks (3.10–3.12) are pure composition.

### Task 3.10 — `MetadataDb` public type ✅
**Reads:** `spec/07_metadata_graph/08_transactions.md` (full).
**Writes:** `crates/brain-metadata/src/db.rs` (new), `crates/brain-metadata/src/lib.rs` (`pub mod db;` + re-exports).

**What was built (first composition piece over the 13 tables):**
- `MetadataDb` struct owning a `redb::Database` + cached `schema_version: u32` + `path: PathBuf`.
- `MetadataDb::open(path)` — `Database::create(path)` then `open_or_init_schema` from 3.1; refuses too-new schemas, initialises fresh DBs at `CURRENT_SCHEMA_VERSION`.
- `read_txn(&self)` and `write_txn(&mut self)` — pass-through to redb. `&mut self` on writes encodes CLAUDE.md §5 invariant 2 (single-writer-per-shard) at compile time: two writer tasks can't both hold `&mut MetadataDb`, so the borrow checker enforces the discipline rather than relying on convention. Consistent with `Wal::append(&mut self, …)` from 2.9.
- `schema_version()`, `path()` accessors. `db()` escape hatch for backup/compact/stats; documented warning not to use it to start a write txn.
- `MetadataDbError` — unifies `redb::DatabaseError` + `redb::TransactionError` + `SchemaError` for the open path. After open, callers handle txn errors natively (no wrapping cascade).

**Deliberate non-implementations:**
- No typed convenience methods (`db.get_memory(&id)`). Spec §07/08 §5 demonstrates multi-table batching inside one write txn; wrapping each row type would duplicate redb's API and break batching. Callers `use brain_metadata::tables::memory::MEMORIES_TABLE;` directly.
- No cached table handles (spec §07/08 §14). Profile-driven; v1 doesn't need it.
- No write-transaction timeout (spec §07/08 §16). Writer-task concern; `MetadataDb` doesn't auto-abort.
- `impl MetadataSink for MetadataDb` — 3.11.

**Mid-flight fixes:**
- `#[derive(Debug)]` needed for `expect_err` in the too-new-schema test.
- `ReadableDatabase` trait import for `db.begin_read()`.
- Clippy's `useless_conversion` on `Result::map_err(Into::into)` — removed since the error types already match.
- MVCC isolation test originally tried opening two `MetadataDb` on the same path; redb takes an exclusive file lock so that fails. Restructured to use one `MetadataDb`: `write_txn(&mut self)` borrows briefly (the returned `WriteTransaction` doesn't carry a lifetime tied to `db`), so calling `read_txn(&self)` afterwards is legal — and the uncommitted write is invisible to the read.

**Done when:** [x] 9 tests: open-fresh, reopen, **too-new-schema refuses**, write-read round trip end-to-end through the wrapper, **MVCC isolation pin** (uncommitted writes are invisible), post-commit visibility, concurrent read txns coexist, schema_version accessor, path accessor. Total in brain-metadata: 91 tests.

### Task 3.11 — `MetadataSink` impl for recovery
**Reads:** `spec/05_storage_arena_wal/08_recovery.md`
**Writes:** `crates/brain-metadata/src/sink.rs`
**Done when:** `impl MetadataSink for MetadataDb` consumes WAL records and updates tables idempotently. End-to-end recovery test (storage + metadata) passes.

### Task 3.12 — Cross-crate integration test
**Reads:** all of phases 2–3.
**Writes:** `crates/brain-metadata/tests/recovery_integration.rs`
**Done when:** Test that drives `Wal::append → MetadataDb` then crashes and recovers. Final state matches expected.

## Phase exit checklist

- [ ] All sub-tasks complete.
- [ ] `just verify` green.
- [ ] Recovery integration test passes 100 random-seed iterations.
- [ ] All 13 spec'd tables present (count `tables/*.rs`).
- [ ] Tag `phase-3-complete`.
