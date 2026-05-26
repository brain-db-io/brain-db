# PROPOSED SPEC — derived-index recovery (review, then apply to `spec/`)

**This is a planning artifact, not the spec.** The `spec/` tree is read-only and
changes go through you. Below are two proposed edits for your review:

1. New content for the empty `spec/09_indexing/06_persistence.md`.
2. An amendment to `spec/08_storage/04_recovery.md §8`.

Both document the recovery model the implementation now follows (memory HNSW
arena-rebuild, entity/statement HNSW startup rebuild) and set up Task 2
(write-time vector persistence). Apply verbatim or adjust to taste.

---

## EDIT 1 — replace `spec/09_indexing/06_persistence.md` (currently empty) with:

````markdown
# 09.06 Index Persistence & Recovery

The three per-shard HNSW indexes (memory, statement, entity) are **in-RAM
only**. They are *derived* state: every vector they hold is reconstructable
from durable source-of-truth (the arena, the redb metadata store). Brain does
**not** persist the HNSW graphs independently in v1; it rebuilds them at shard
startup. This file specifies how each index is rebuilt and the durability
contract that makes the rebuild lossless.

## 1. Why rebuild rather than persist

`hnsw_rs` builds an in-memory graph with no cheap incremental on-disk insert —
durability would require rewriting the whole graph dump on a cadence. Brain
keeps the source vectors durable instead (arena for memories; redb text for
statements/entities) and rebuilds the graphs on startup. This keeps the write
path cheap (one WAL append + one arena/redb write) and makes recovery a pure
function of durable state. See `08/04 §8`.

## 2. The three indexes

| Index | Source of truth | Vector at rebuild | Rebuild trigger |
|---|---|---|---|
| Memory HNSW | arena (persisted vectors) | read verbatim from the arena | startup, synchronous, before serving |
| Entity HNSW | redb `ENTITIES_TABLE` (`canonical_name`) | re-embed `canonical_name` | startup, synchronous, before serving |
| Statement HNSW | redb `STATEMENTS_TABLE` (text) | re-embed via `StatementEmbedWorker` | startup seeds the embed queue; worker repopulates in background |

The resolver inserts `embed(canonical_name)` at entity-create, so re-embedding
canonical names on restart reproduces the stored vectors exactly. The statement
embedder builds `subject + predicate + object` text; seeding the embed queue
with every live statement lets the existing worker reproduce those vectors
without duplicating its text-assembly logic.

## 3. Liveness & idempotency

- Tombstoned entities (`flags::TOMBSTONED`) and tombstoned statements are
  skipped — the rebuild reflects only live rows.
- Re-seeding the statement embed queue is idempotent: the worker skips any
  statement already present in the HNSW, and re-enqueuing upserts the same row.
- The memory HNSW rebuild folds the (empty-at-startup) pending buffer, so a
  stray insert during boot can't be lost.

## 4. Startup ordering

WAL replay (`08/04 §2` steps 1–5) restores the arena + metadata first. The
HNSW rebuilds (this file) are step 6, run before the shard is marked ready
(`08/04 §8`), so the shard never serves with a half-populated semantic index.
The statement HNSW is the one exception: it repopulates asynchronously via the
embed-queue worker, so statement-scoped semantic search fills in shortly after
the shard is ready. Memory recall — the primary path — is fully available at
ready.

## 5. Recovery cost

Memory HNSW: O(N·log N) graph build (`08/04 §8` figures). Entity HNSW: N
embedding inferences + graph build. Statement HNSW: background, amortized over
worker ticks. For deployments where the entity-rebuild embedding cost dominates
startup, see §6.

## 6. (Future) write-time vector persistence — Task 2

To make restart O(load) rather than O(re-embed), statement and entity embedding
vectors may be persisted at write time (new redb tables `STATEMENT_VECTORS`,
`ENTITY_VECTORS`, `EntityId|StatementId → [f32; D]` via bytemuck). Startup then
rebuilds the graph from stored vectors with **zero embedder calls**. This is a
write-path + schema change (a redb version bump) and is deferred to a dedicated
sub-task; the rebuild-from-text path above is the v1 baseline and remains the
fallback when a vector is absent.

## 7. (Future) graph snapshot persistence — deferred

Persisting the PQ-HNSW graph itself (the `SharedHnsw::save_snapshot` /
`load_snapshot` path, currently `SnapshotNotYetImplemented`) would make restart
O(load) with no rebuild at all, at the cost of snapshot CRC + checkpoint-LSN
machinery and tail-replay. Deferred until restart latency at scale is a
measured problem; it supersedes §2's rebuild for the memory index when adopted.
````

---

## EDIT 2 — amend `spec/08_storage/04_recovery.md §8`

Replace the §8 body ("The HNSW index is not persisted independently; ... For a
1M-memory shard ...") with:

````markdown
## 8. Rebuilding the HNSW indexes

The three HNSW indexes (memory, statement, entity) are not persisted
independently; they are rebuilt on startup from durable source-of-truth. See
[09.06 Index Persistence & Recovery](../09_indexing/06_persistence.md) for the
per-index rebuild model. Summary:

- **Memory HNSW** — iterate occupied, non-tombstoned arena slots and insert
  each `(MemoryId, vector)`. (Equivalent to iterating live memories in the
  metadata store and reading the arena; Brain iterates the arena directly,
  since each slot carries its occupancy and tombstone flags.)
- **Entity HNSW** — iterate live entities in the metadata store and insert
  `(EntityId, embed(canonical_name))`.
- **Statement HNSW** — seed the embed queue with every live statement; the
  per-shard `StatementEmbedWorker` repopulates the index in the background.

Memory and entity rebuilds complete before the shard is marked ready (step 8);
the statement rebuild proceeds asynchronously after ready.

For a 1M-memory shard, the memory rebuild takes ~30 seconds single-threaded or
~5 seconds parallel; entity rebuild adds its embedding cost. The rebuild can be
parallelized across cores; each shard's indexes are owned by that shard's
executor, so shards rebuild concurrently.
````

---

## Apply checklist

- [ ] Paste EDIT 1 into `spec/09_indexing/06_persistence.md`.
- [ ] Apply EDIT 2 to `spec/08_storage/04_recovery.md §8`.
- [ ] (Optional) add a one-line pointer from `spec/09_indexing/00_purpose.md` to §06.

Once applied, Task 2 (write-time vector persistence) is unblocked.
