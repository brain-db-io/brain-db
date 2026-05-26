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
function of durable state. See [`08.04 §8`](../08_storage/04_recovery.md).

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

WAL replay ([`08.04 §2`](../08_storage/04_recovery.md) steps 1–5) restores the
arena + metadata first. The HNSW rebuilds (this file) are step 6, run before
the shard is marked ready ([`08.04 §8`](../08_storage/04_recovery.md)), so the
shard never serves with a half-populated semantic index. The statement HNSW is
the one exception: it repopulates asynchronously via the embed-queue worker, so
statement-scoped semantic search fills in shortly after the shard is ready.
Memory recall — the primary path — is fully available at ready.

## 5. Recovery cost

Memory HNSW: O(N·log N) graph build (see [`08.04 §8`](../08_storage/04_recovery.md)
figures). Entity HNSW: N embedding inferences + graph build. Statement HNSW:
background, amortized over worker ticks. For deployments where the
entity-rebuild embedding cost dominates startup, see §6.

## 6. (Future) write-time vector persistence

To make restart O(load) rather than O(re-embed), statement and entity embedding
vectors may be persisted at write time (redb tables `STATEMENT_VECTORS`,
`ENTITY_VECTORS`, keyed `StatementId | EntityId → [f32; D]` via bytemuck).
Startup then rebuilds the graph from stored vectors with **zero embedder
calls**. This is a write-path + schema change (a redb version bump); the
rebuild-from-text path in §2 is the baseline and remains the fallback when a
vector is absent (a row written before the feature, or a partial write).

## 7. (Future) graph snapshot persistence

Persisting the PQ-HNSW graph itself (the `SharedHnsw::save_snapshot` /
`load_snapshot` path, currently `SnapshotNotYetImplemented`) would make restart
O(load) with no rebuild at all, at the cost of snapshot CRC + checkpoint-LSN
machinery and tail-replay. Deferred until restart latency at scale is a
measured problem; it supersedes §2's rebuild for the memory index when adopted,
and requires a corresponding revision of [`08.04 §8`](../08_storage/04_recovery.md).
