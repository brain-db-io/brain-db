# Plan: Restart data availability — derived-index recovery

**Status:** approved — Task 1 implemented (statement + entity HNSW startup rebuild), uncommitted; full restart→recall integration harness still pending; Task 2 not started

## Implementation note (Task 1)

- `brain-metadata`: added `entity::ops::entity_iter_all_live` (scan live entities → `(EntityId, canonical_name)`) and `statement::statement_embed_queue_seed_all_live` (re-enqueue every live statement). Both unit-tested (`iter_all_live_returns_live_skips_tombstoned`, `embed_queue_seed_all_live_reenqueues_live_statements`) — pass in container.
- `brain-server` `spawn_shard`: after the memory-HNSW reseed, (1) rebuilds the entity HNSW synchronously by re-embedding canonical names, (2) seeds the statement embed queue so `StatementEmbedWorker` repopulates the statement HNSW in the background.
- Verified: `cargo check` + `cargo clippy --no-deps -D warnings` clean on `brain-metadata` and `brain-server` in the Linux container.
- **Deferred:** the real-embedder restart→recall integration harness (the e2e harness uses `NopDispatcher`; building a real-embedder restart test is a separate piece). Unit tests cover the rebuild *sources*; the startup wiring is compile-verified but not yet exercised end-to-end.
- **Spec still owed:** `09/06_persistence.md` (empty) + `08/04 §8` amendment to document statement/entity rebuild — drafted-for-user-apply, not yet written (spec is read-only).
**Date:** 2026-05-26
**Author:** Claude (autonomous)
**Estimated commits:** 3–5 across 3 sequenced tasks (+ 1 spec change, user-applied)

---

## 1. Scope

Make all recall-relevant state available after a server/db restart, and move toward the user's stated architecture — *do the heavy work at write time, persist to the filesystem, pick up cheaply on restart*.

Three derived (RAM-only) indexes are the entire problem; the source-of-truth layer (WAL + arena + redb + tantivy) is already durable.

- **Memory HNSW** — already fixed (commit pending): rebuilt from the arena at startup, which is exactly what spec `08/04 §8` mandates.
- **Statement HNSW** — currently lossy on restart (only the pending embed-*queue* replays; already-embedded statements are dropped).
- **Entity HNSW** — currently lossy on restart (reseeds only as surfaces get re-extracted).

**Does NOT cover** (explicitly deferred): full PQ-HNSW graph snapshot persistence (the `save_snapshot`/`load_snapshot` stubs) is scoped as an optional Task 3, gated on a measured restart-latency problem and a spec change — not required for correctness.

This plan is **spec-gated**: `spec/09_indexing/06_persistence.md` is an empty stub and `08/04 §8` constrains the approach. The spec work is step 0.

## 2. Spec references

- **`spec/08_storage/04_recovery.md §8`** (binding): verbatim —
  > "The HNSW index is **not persisted independently**; it's **rebuilt on startup from the arena and metadata.** After WAL replay, Brain: 1. Iterates over all active (non-tombstoned) memories in the metadata store. 2. ...reads the vector from the arena. 3. Calls `hnsw_index.insert(memory_id, vector)`."
  - This is the authoritative model for the **memory** HNSW. The landed fix conforms (it iterates the arena directly via `ArenaRebuildSource`; §8 iterates metadata then reads the arena — equivalent, since arena slots carry occupancy/tombstone flags). Reconciliation note belongs in the spec amendment.
  - §8 says nothing about the **statement** or **entity** HNSW (they post-date this section). Their restart behavior is **spec-silent** → must be written.
- **`spec/08_storage/04_recovery.md §1`**: recovery goal — "All WAL records durably written before the crash are reflected in ... the HNSW index." The statement/entity indexes currently violate this.
- **`spec/08_storage/05_checkpointing.md §2`**: checkpoint marks WAL≤LSN reflected in **arena + metadata** — *not* the indexes. Index rebuild is unbounded by checkpoints today.
- **`spec/09_indexing/06_persistence.md`**: **empty stub.** The snapshot→load→tail-replay recovery procedure was never specified. Must be authored before Task 3.
- **CLAUDE.md invariant 1 (WAL-before-ack)** and **§5**: source of truth is durable; derived indexes are reconstructable. Honored.

Code evidence:
- `crates/brain-server/src/shard/mod.rs:1424` — memory HNSW created empty; reseed-from-arena landed at `~1885`.
- `:1431–1434` — statement HNSW: "on restart the queue replays the still-pending rows; rows already embedded fall through" → **lossy**.
- `:1444–1446` — entity HNSW: "reseeds via the resolver's tier-4 path the first time each surface form is re-extracted" → **lossy**.
- `crates/brain-index/src/shared.rs:74,85` — `save_snapshot`/`load_snapshot` are **stubs** returning `HnswError::SnapshotNotYetImplemented` (PQ persistence never wired).
- `crates/brain-metadata/src/tables/statement.rs:85` — `STATEMENT_EMBED_QUEUE_TABLE: [u8;16] → u64` is a *queue of statement IDs needing embedding*, **not** a vector store. Statement vectors are not persisted.
- Entity vectors are likewise RAM-only (resolver embeds canonical names into the entity HNSW; no persisted vector found).

## 3. External validation

- **Task 1 + Task 2** — internal wiring + redb schema only (rebuild via the existing embed worker / `rebuild_impl`; new redb vector tables). **Not applicable — internal.**
- **Task 3 (deferred)** — would depend on `hnsw_rs` whole-graph `file_dump`/reload (`HnswIo`) round-tripping under our PQ wrapper. **Not yet validated** — to be web-checked against `hnsw_rs 0.3` docs *if and when* Task 3 is approved. Flagged as a Task-3 prerequisite, not done now.

## 4. Architecture sketch

Spectrum of approaches (the plan picks Level 1 now, Level 2 as the user's target, Level 3 deferred):

```
Level 0 (today):  stmt/entity vectors nowhere on disk  → lossy restart
Level 1 (Task 1): re-embed from persisted *text* at startup → correct, embed-cost
Level 2 (Task 2): persist stmt/entity *vectors* at write time → rebuild graph, no re-embed
Level 3 (Task 3): persist the *graph* (snapshot) → load directly, no rebuild  [deferred]
```

### Task 1 — startup rebuild of statement + entity HNSW (correctness; spec-aligned)

Mirror the memory-HNSW pattern (`08/04 §8`) for the other two indexes, run synchronously in `spawn_shard` before the shard serves:

```
// statement HNSW: re-enqueue every live statement into the existing
// embed queue so StatementEmbedWorker repopulates the index in the
// background (non-blocking on the serve path), OR a direct synchronous
// rebuild for determinism. Source text: STATEMENTS_TABLE.
seed_statement_embed_queue_from_all_live(&metadata)      // background catch-up
// entity HNSW: re-embed each live entity's canonical_name and insert.
rebuild_entity_hnsw_from_metadata(&metadata, &embedder, &entity_hnsw)
```

Memory HNSW: already landed (arena rebuild). No change beyond the §8 reconciliation note.

### Task 2 — persist derived-index vectors at write time (the user's architecture)

Add the embedding vector to the write path so restart rebuilds the HNSW *graph* from stored vectors without re-embedding:

```
new redb tables (or per-index arenas):
  STATEMENT_VECTORS:  StatementId([u8;16]) → [f32; 384] (bytemuck)
  ENTITY_VECTORS:     EntityId([u8;16])    → [f32; 384]
write path: on STATEMENT_CREATE / entity create, store the vector alongside.
startup: rebuild_impl from the stored vectors (no embedder calls).
```

This is "heavy work at write time": embed once, persist, never re-embed on restart.

### Task 3 — full PQ-HNSW graph snapshot (deferred, spec-change, scale-only)

Implement `save_snapshot`/`load_snapshot` (codebook + graph + idmap + tombstones + CRC), record the snapshot LSN in the checkpoint, load-on-restart + replay WAL tail, arena/vector-rebuild as fallback. Turns restart into O(load). Big; only if Level 2 restart cost is a measured problem.

## 5. Trade-offs considered

| Approach | Pros | Cons | Verdict |
|---|---|---|---|
| **L1: re-embed from text at startup** (Task 1) | Correct; spec-aligned (§8 rebuild principle); zero schema/write-path change; ships now | Startup embed cost (entity names sync; statements via background worker → semantic-stmt search lags briefly) | ✓ **now** — the actual data-availability fix |
| **L2: persist vectors at write time** (Task 2) | The user's model; no re-embed on restart; deterministic, bounded restart; modest write/storage cost | New redb tables (schema bump); write-path change; still rebuilds the graph | ✓ **next** — primary architectural thrust |
| **L3: persist the graph** (Task 3) | O(load) restart at scale | Implement stubbed PQ persistence; **contradicts §8**; biggest blast radius | deferred — scale-gated |
| Do nothing / lazy reseed (status quo) | — | Silent semantic data loss on every restart | rejected |
| Block serving until full sync rebuild of all 3 | Simple correctness | Slow startup; statements need embed inference | partial — use background catch-up for statements |

## 6. Risks / open questions

- **Spec contradiction (Task 3).** §8 says "not persisted independently." Task 3 reverses that → requires a §8 amendment + 09/06 authored. Spec is read-only; I draft, user applies. **Task 3 stays gated.**
- **Startup embedding cost (Task 1, entity).** Re-embedding all entity canonical names at startup is synchronous CPU. Mitigation: it's bounded by entity count (typically << memory count); if large, route through the off-core embed path. Open question: acceptable startup budget?
- **Statement semantic search lag (Task 1).** Re-enqueueing statements means the statement HNSW repopulates in the background, not instantly. Mitigation: acceptable (statement-scoped semantic search is a minority path; memory recall — the main path — is unaffected). Alternative: synchronous rebuild if determinism is required.
- **redb schema bump (Task 2).** New vector tables → per CLAUDE.md, pre-release in-place change (no migration shim). Confirm with `brain-redb-schema` discipline.
- **Embedding determinism on reload (Task 1).** Re-embedding must produce the same vectors as the originals (same model + fingerprint). The embedding fingerprint gate (`spec/07_embedding/05`) already enforces model identity — verify it's consulted so a model change doesn't silently mix vector spaces.
- **§8 literal vs landed memory fix.** §8 iterates metadata then reads arena; the landed fix iterates the arena directly. Equivalent, but the spec amendment should record the chosen source to avoid future "spec drift" confusion.

## 7. Test plan

Each maps to recovery goal `08/04 §1` ("reflected in ... the HNSW index"):

- `[ ] memory recall survives restart` ← `shard_restart_then_semantic_recall_returns_memory` (real embedder: encode → drop shard → respawn → recall returns the memory with non-zero `sem`). Proves the landed fix + guards regression.
- `[ ] statement semantic search survives restart` ← `shard_restart_then_statement_scope_recall_returns_statement` (after background catch-up drains).
- `[ ] entity resolution survives restart` ← `entity_hnsw_repopulated_after_restart` (resolve a surface that requires the tier-3 embedding match post-restart).
- `[ ] empty shard restart is a no-op` ← extend `arena_uuid_persists_across_restarts` to assert empty indexes, no error.
- **Task 2 adds:** `[ ] stored vectors round-trip` (write → read redb → bit-equal); `[ ] restart rebuilds from stored vectors without invoking the embedder` (embedder call-count == 0 on restart path).
- **Chaos (Task 2/3):** kill-during-checkpoint → restart → all three indexes consistent with WAL (use `brain-chaos-test`).

Harness note: the existing restart test (`tests/shard.rs::arena_uuid_persists_across_restarts`) uses a zero-vector stub and the e2e harness uses `NopDispatcher` — neither catches semantic regressions. Task 1 must add a **real-embedder restart→recall** harness (extract `spawn → encode → drop → respawn → recall` into a reusable helper). This is itself part of the deliverable.

## 8. Commit shape

- **Spec 0 (user-applied):** author `spec/09_indexing/06_persistence.md` (recovery model for all three indexes) + amend `08/04 §8` (statement/entity rebuild; arena-iteration reconciliation; Task-2 vector persistence; Task-3 snapshot as future). Surfaced as a proposed diff; the user applies it (spec is read-only).
- **Commit A (Task 1a):** statement HNSW restart — seed embed queue from all live statements at startup + the real-embedder restart→recall test harness.
- **Commit B (Task 1b):** entity HNSW restart — rebuild from metadata canonical names at startup + entity-resolution-after-restart test.
- **Commit C (memory):** the already-landed arena reseed (commit `<pending>`) — referenced, not re-done; spec note reconciled.
- **Commit D (Task 2, separate approval):** `STATEMENT_VECTORS` / `ENTITY_VECTORS` redb tables + write-path persistence + startup rebuild-from-stored-vectors + round-trip tests.
- **Task 3:** its own plan when/if approved.

## 9. Confirmation

Recommended sequence: **Spec 0 → Task 1 (ships the correctness fix) → Task 2 (your write-time-persist architecture) → Task 3 only if restart latency at scale is measured.**

Awaiting confirmation on: (a) the spec-first gate, (b) whether to ship Task 1 first as the correctness fix, and (c) whether Task 2 is the agreed target architecture before I touch code.
