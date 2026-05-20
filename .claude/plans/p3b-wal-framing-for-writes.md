# P3b — WAL framing for the unified `submit(Write)` path

## Context

P1-P3c shipped the unified write path: `Write { phases }` flows through
`RealWriterHandle::submit` → idempotency check → one redb wtxn →
`apply::dispatch` per phase → commit → event publish → cache stamp.

**The gap:** `submit()` writes to redb but never appends to the WAL.
A crash between commit and the next request loses no data (redb is
durable), but a subscriber reconnecting with `start_lsn=N` cannot
replay events for writes that took the unified path — those writes
have no WAL records to replay from. And there's no story for replaying
the unified path on a fresh shard at startup.

The legacy path (`submit_encode` / `submit_forget` / `submit_link` /
`submit_unlink`) does WAL-append → fsync → redb-commit → publish in
order, and recovery on shard open replays the WAL onto a fresh shard.
P3b brings the unified path to feature parity.

## The core design choice

There are three reasonable shapes for "how does a Write become WAL
records." Each has its own tradeoffs.

### Option A — `WalPayload::Write(serialized_bytes)` (envelope-style)

One new variant carries the whole Write (its phases vector) as a
single rkyv-encoded blob. One WAL record per Write, regardless of
phase count.

- **Pros:** Trivially atomic (record either lands fully or not at
  all). One fsync per write. Recovery sees one record per write.
- **Cons:** Multi-phase writes can be huge (a TXN with K entity
  creates + M statements). WAL record-size limits become a concern.
  Serialization format is its own design (rkyv vs bincode vs custom).
  Existing typed payloads (`Encode`, `Forget`, `Link`, ...) become
  redundant for the unified path but stay around for the legacy path
  until P4 deletes it — leaving two parallel encodings during the
  migration window.

### Option B — `BeginWrite` + N × `Phase` + `EndWrite` framing

Three new record kinds. Recovery groups records between `BeginWrite`
and `EndWrite` markers; a torn write (begin without end) is discarded.

- **Pros:** Each WAL record is bounded in size. Granular crash
  semantics: torn writes are obvious. Per-phase records mean each
  phase can choose its own serialization.
- **Cons:** Recovery has to maintain a buffered state machine
  (already exists for TxnBegin/Commit but would need to grow). 1+N+1
  LSNs per write instead of 1. New `WalRecordKind` discriminants
  reserved.

### Option C — Reuse existing typed payloads + wrap multi-phase writes in TxnBegin/TxnCommit

Substrate phases map to existing typed payloads:
`UpsertMemory → WalPayload::Encode`,
`Tombstone(Memory) → WalPayload::Forget`,
`Link → WalPayload::Link`,
`Unlink → WalPayload::Unlink`,
`UpdateSalience/Kind/Context → existing variants`,
`MigrateEmbedding → existing variant`,
`Reclaim → existing variant`.

Knowledge phases (UpsertEntity / UpsertStatement / UpsertRelation /
Tombstone(Entity/Statement/Relation) / Supersede / UpsertSchema /
SetExtractorEnabled / Resolve / MergeEntities / UpdateAttribute /
StampAudit / ReclaimSlots) map to existing `WalPayload::Knowledge` and
the typed `RelationLink` / `RelationSupersede` / `RelationTombstone`
variants that already exist.

Multi-phase writes (TXN_COMMIT or worker batches like the extractor's
post-encode bundle) wrap their phase records in `WalPayload::TxnBegin`
+ `WalPayload::TxnCommit`. **The TXN state machine in `recovery.rs`
already handles this** — see `crates/brain-storage/src/recovery.rs`
lines 217-256.

- **Pros:** Reuses every existing WAL machinery: payloads, encoders,
  decoders, recovery state machine, subscribe-replay. No new
  `WalRecordKind` discriminants. Multi-phase atomicity is free.
- **Cons:** Per-phase mapping logic is non-trivial. Some phases
  (UpsertSchema, MergeEntities) have no existing typed payload and
  fall back to `WalPayload::Knowledge` with an opaque body, which
  needs its own encoding scheme for the apply::dispatch replay path.

## Recommendation: **Option C**

Reasons:
1. **Minimal new surface.** No new `WalRecordKind` variants needed
   beyond what exists. No new payload encoders. No new recovery code.
2. **Reuses durable invariants.** The Glommio shard's group-commit,
   the WAL reader's segment scanning, the TXN state machine —
   everything keeps working.
3. **Backwards-compatible at the WAL byte level.** A WAL written by
   P3b is identical to one a legacy `submit_encode` would have
   written, modulo the TxnBegin/Commit envelope for multi-phase writes.
   Existing recovery code reads it without modification.
4. **The `WalPayload::Knowledge` variant was designed for exactly
   this purpose.** It has decoders in `subscribe.rs` already. P3b
   wires the encoders.
5. **No serialization-format design needed.** Each phase encodes via
   its matching existing typed payload's `encode_*` function.

The cost is the per-phase mapping logic in `submit()`. That's
mechanical — one match arm per Phase variant — and lives in one place.

## What lands in this slice

### 1. `Phase → WalPayload` mapping

New module: `crates/brain-ops/src/ops/writer/wal_map.rs`. One function:

```rust
pub fn phase_to_wal_payload(phase: &Phase, write: &Write) -> Option<WalPayload>;
```

`None` for phases that don't need a WAL record (e.g. `ReclaimSlots`
is re-derivable from MEMORIES_TABLE on next maintenance cycle; auto-
derived edges have `origin=AUTO_DERIVED` and are also re-derivable;
the subscribe path handles those without WAL).

Mapping by phase:

| Phase variant | WAL payload |
|---|---|
| `UpsertMemory` | `WalPayload::Encode(EncodePayload {...})` |
| `Tombstone(Memory)` | `WalPayload::Forget(ForgetPayload {...})` |
| `Link` | `WalPayload::Link(LinkPayload {...})` |
| `Unlink` | `WalPayload::Unlink(UnlinkPayload {...})` |
| `UpdateSalience` | `WalPayload::UpdateSalience(UpdateSaliencePayload {...})` |
| `UpdateKind` | `WalPayload::UpdateKind(UpdateKindPayload {...})` |
| `UpdateContext` | `WalPayload::UpdateContext(UpdateContextPayload {...})` |
| `UpdateEmbedding` | `WalPayload::MigrateEmbedding(MigrateEmbeddingPayload {...})` |
| `ReclaimSlots` | `WalPayload::Reclaim(ReclaimPayload {...})` |
| `UpsertEntity` | `WalPayload::Knowledge(KnowledgeRecord { kind: EntityCreate, body: rkyv-of-Entity })` |
| `Tombstone(Entity)` | `WalPayload::Knowledge(KnowledgeRecord { kind: EntityTombstone, body: rkyv-of-(id, at) })` |
| `UpsertStatement` | `WalPayload::Knowledge(KnowledgeRecord { kind: StatementCreate, body: rkyv-of-Statement })` |
| `Supersede(Statement)` | `WalPayload::Knowledge(KnowledgeRecord { kind: StatementSupersede, body: rkyv-of-(old, new_Statement, at) })` |
| `Tombstone(Statement)` | `WalPayload::Knowledge(KnowledgeRecord { kind: StatementTombstone, body: rkyv-of-(id, reason, at) })` |
| `UpsertRelation` | `WalPayload::RelationLink(RelationLinkPayload {...})` |
| `Supersede(Relation)` | `WalPayload::RelationSupersede(RelationSupersedePayload {...})` |
| `Tombstone(Relation)` | `WalPayload::RelationTombstone(RelationTombstonePayload {...})` |
| `UpsertSchema` | `WalPayload::Knowledge(KnowledgeRecord { kind: SchemaUpdate, body: rkyv-of-(namespace, version, blob) })` |
| `SetExtractorEnabled` | `WalPayload::Knowledge(KnowledgeRecord { kind: ExtractorToggle, body: rkyv-of-(id, enabled) })` |
| `Resolve` | None — derived state; the entity it creates lands as a separate `UpsertEntity` phase if needed |
| `MergeEntities` | `WalPayload::Knowledge(KnowledgeRecord { kind: EntityMerge, body: rkyv-of-(source, target, at) })` |
| `UpdateAttribute` | `WalPayload::Knowledge(KnowledgeRecord { kind: AttributeUpdate, body: rkyv-of-(target, key, value) })` |
| `StampAudit` | None — derivable from the things being audited |

The `WalPayload::Knowledge` body bytes are rkyv-encoded structs;
introduce small typed wrappers under
`crates/brain-storage/src/wal/knowledge_bodies.rs` for each kind so
the encoders / decoders share a definition.

### 2. Submit-pipeline integration

Update `RealWriterHandle::submit` to insert WAL append between the
idempotency-cache-miss and the wtxn-open step:

```rust
pub async fn submit(&self, write: Write) -> Result<WriteAck, WriterError> {
    if let Some(cached) = cache.lookup(write.write_id) { return Ok(cached); }

    // P3b — WAL append. For single-phase writes: one payload.
    // For multi-phase: TxnBegin + N × phase payloads + TxnCommit.
    let (lsn_first, lsn_last) = self.wal_append_for(&write).await?;

    // Open wtxn, apply each phase, commit (unchanged from P3).
    let phase_acks = apply_all_phases(&self.metadata, &write)?;

    // Publish events using the WAL-assigned LSNs (replaces bus-minted).
    publish_events_with_lsns(self, &write, lsn_first, &phase_acks);

    let ack = WriteAck { write_id, committed_at, lsn_first, lsn_last, phase_acks };
    cache.stamp(write_id, ack.clone());
    Ok(ack)
}
```

The legacy submit methods stay untouched — they continue to use their
existing typed-payload append paths. P4 deletes them after handler
migration.

### 3. Recovery path — verify, don't change

Recovery for the single-phase case is already correct (typed payloads
replay through `apply_to_redb` which writes the same redb rows the
apply functions write). For the multi-phase case, the TXN state
machine groups records between TxnBegin and TxnCommit and replays
them as a batch. **What needs verification:** the per-record replay
inside the buffered loop currently calls `apply_to_redb` (the legacy
substrate replay function) — it needs an additional dispatch path for
`WalPayload::Knowledge` records to call into `apply::dispatch` against
the decoded Phase.

This is the only new code in recovery: a helper that takes a
`(WalPayload::Knowledge, WriteTransaction)` and reconstructs the
corresponding `Phase`, then calls `apply::dispatch`.

### 4. Idempotency cache becomes durable

The in-memory `WriteIdempotencyCache` from P3 graduates to a redb-
backed table: extend the existing `IDEMPOTENCY_TABLE` schema to key by
`WriteId` (16 bytes) instead of (or in addition to) the legacy
`RequestId` key. Replays after a crash see the cached `WriteAck` from
redb.

This is technically optional for P3b (the in-memory cache is correct
within a single process lifetime) but is the natural pair: once WAL
is durable, idempotency should be too. Decide whether to bundle here
or split into P3c-prime.

## Files touched

```
crates/brain-storage/src/wal/
├── payload.rs              # Add knowledge_body encoders for the
│                           # KnowledgeRecord kinds that don't have them
│                           # yet (EntityCreate / EntityTombstone /
│                           # StatementCreate / StatementSupersede /
│                           # StatementTombstone / SchemaUpdate /
│                           # ExtractorToggle / EntityMerge /
│                           # AttributeUpdate).
└── knowledge_bodies.rs     # NEW. rkyv-derived typed wrappers for each
                            # KnowledgeRecord kind's body.

crates/brain-ops/src/ops/writer/
├── wal_map.rs              # NEW. phase_to_wal_payload + helpers
│                           # that build typed payloads from phases.
└── submit.rs               # Insert WAL-append step + LSN-stamping
                            # into the submit pipeline.

crates/brain-ops/src/apply/
└── mod.rs                  # New entry point `dispatch_from_wal_payload`
                            # for recovery — takes a WalPayload and a
                            # wtxn, reconstructs the Phase, calls
                            # apply::dispatch.

crates/brain-storage/src/recovery.rs
                            # Hook the new dispatch_from_wal_payload
                            # into the TXN replay loop and the
                            # single-record replay path for
                            # WalPayload::Knowledge variants.

crates/brain-metadata/src/tables/idempotency.rs (if P3c-prime bundles)
                            # Add WriteId-keyed column.

crates/brain-ops/src/ops/writer/submit.rs
                            # Idempotency cache: extend to use the
                            # durable table.
```

## Acceptance tests

1. **Single-phase WAL round-trip.** Submit `Write { phases: [Link] }`.
   Inspect the WAL — exactly one `WalPayload::Link` record with the
   right fields. Recover from the WAL onto a fresh metadata DB. The
   edge row materialises.

2. **Multi-phase WAL round-trip.** Submit `Write { phases: [UpsertMemory, Link] }`.
   WAL has `TxnBegin, Encode, Link, TxnCommit`. Recovery replays the
   whole batch atomically — both rows land or neither.

3. **Torn write.** Submit a 3-phase write but truncate the WAL after
   the first phase record (before TxnCommit). Recovery discards the
   incomplete batch (matches existing TXN state machine behaviour).

4. **Knowledge phase WAL round-trip.** Submit `Write { phases: [UpsertEntity] }`.
   WAL has one `WalPayload::Knowledge(EntityCreate)` record with a
   rkyv-encoded Entity body. Recovery decodes, calls
   `apply_upsert_entity` against a fresh wtxn, entity row lands.

5. **LSN-stamped events.** Submit a Write. The published
   `EventEnvelope` has `lsn` set to the WAL-assigned value, not 0 (as
   in P3c). A subscriber with `start_lsn=N+1` after a server restart
   sees the event via subscribe-replay.

6. **Idempotency across restart** (if P3c-prime is bundled). Submit
   `Write { write_id: X, phases: [...] }`. Restart the server. Submit
   the same write_id with different phases — returns the cached ack
   from the first submission, doesn't re-apply.

7. **Recovery determinism property test.** With `proptest`: generate
   random `Write` sequences, submit them all, snapshot the resulting
   redb state, then replay the same WAL onto a fresh metadata DB
   from scratch. The post-replay redb state matches the original byte-
   for-byte (modulo timestamps / non-deterministic fields, which apply
   functions never read).

## Edge cases worth nailing in tests

1. **Empty Write (`phases: []`).** Reject at submit time with
   `WriterError::Internal("empty write")` — an empty write would
   write a TxnBegin/TxnCommit pair to the WAL with no enclosed work,
   which recovery would handle correctly but is wasteful and almost
   certainly a caller bug.

2. **Very large Write.** A multi-phase write whose total encoded
   size exceeds the WAL segment size limit. Either the segment writer
   handles segment-spanning records (check brain-storage) or we
   reject at submit time. Document either way.

3. **Phase with no WAL mapping inside a multi-phase write.** A Write
   like `[UpsertMemory, ReclaimSlots]` where ReclaimSlots returns
   `None` from `phase_to_wal_payload`. The WAL records the UpsertMemory
   inside a TxnBegin/Commit envelope; ReclaimSlots is purely a redb
   commit. Recovery sees the envelope, replays UpsertMemory, doesn't
   need to know about ReclaimSlots because the reclaim is derivable
   from MEMORIES_TABLE state. Confirm test covers this case.

4. **Write submitted, WAL append succeeds, redb commit fails.**
   Replay on next start would re-apply the WAL records. The retry
   succeeds (apply is idempotent at the redb level — same Phase
   produces same row) or fails on the same constraint that failed
   live. Either way, no torn state. Property test should cover this.

5. **Schema-version drift.** A WAL record written by today's Brain
   replayed by tomorrow's. Spec is read-only and the wire is
   non-versioned per the user's preferences (no DB/wire versioning).
   So: same-revision-or-fail. The recovery code already fails fast
   on payload-decode error; this stays the same.

## What this slice does NOT do

- **HNSW + arena writes for UpsertMemory.** That's P3d. Recovery for
  Encode-shaped WAL records currently replays into `apply_to_redb` →
  metadata only. The HNSW vector and arena bytes get rebuilt by
  HnswMaintenanceWorker scanning the metadata table. P3b doesn't
  change this — it ports the existing pattern to the unified path.

- **Handler migration.** That's P4. Substrate handlers
  (`handle_encode` / `handle_forget` / `handle_link` / `handle_unlink`)
  still use the legacy `submit_encode` etc. methods, which still do
  their own WAL append via the legacy path. Both paths produce the
  same WAL records — recovery doesn't care which path wrote them.

- **EdgeWorker consolidation, ExtractorWorker reshape, TXN unification.**
  P6 / P7 / P8. After P3b, those slices can route their writes through
  `submit(Write)` and gain WAL durability the same way encode does.

## Sequencing inside P3b

Roughly in order — each step compiles and tests independently:

1. **Define `knowledge_bodies.rs`.** rkyv-derived structs for each
   `KnowledgeRecord` kind. Round-trip serde tests.

2. **Wire encoders for the new KnowledgeRecord kinds in `payload.rs`.**
   Round-trip tests against `record::WalRecord::from_typed` /
   `decode_one`.

3. **Build `wal_map.rs`.** One match arm per Phase variant.
   Property test: every Phase variant either returns a WalPayload
   whose decode round-trips back to the same Phase, or returns None.

4. **Plumb WAL append into `submit()`.** Single-phase write: one
   append. Multi-phase write: TxnBegin + N × payloads + TxnCommit.
   Unit tests: WAL byte snapshot per write shape.

5. **Add `apply::dispatch_from_wal_payload`.** Maps WalPayload back
   to Phase, calls `apply::dispatch`. Unit tests against tempfile
   MetadataDb.

6. **Hook `dispatch_from_wal_payload` into recovery.** The recovery
   replay loop already calls `apply_to_redb` for substrate payloads;
   add a parallel path for knowledge payloads.

7. **Acceptance suite** (the seven tests in §"Acceptance tests"
   above). The proptest is the most demanding — needs a
   proptest-driven `Write` generator that produces well-formed
   phases.

8. **Decide on idempotency-durability bundling.** If yes: extend
   IDEMPOTENCY_TABLE, write the cache through. If no: split as
   P3c-prime follow-up.

Approximate sizing: ~500 LoC if idempotency stays in-memory,
~700 LoC if it goes durable.

## Open questions for the user to decide before code

1. **Knowledge body serialization format.** rkyv is the project
   default. Confirm? Alternative: bincode for these specific bodies
   if rkyv's archived-bytes guarantee isn't needed for KnowledgeRecord
   replays (since they're consumed once at recovery, not held as
   archived views).

2. **`WalPayload::Knowledge` kind discriminants.** The existing
   `KnowledgeRecord.kind` byte set covers EntityCreate /
   EntityTombstone / StatementCreate / StatementSupersede /
   StatementTombstone / SchemaUpdate. We need to add:
   ExtractorToggle, EntityMerge, AttributeUpdate. Each gets a new
   byte value. Confirm we're free to allocate them (no backwards-
   compatibility constraint from the user's no-versioning stance).

3. **Idempotency durability bundling** (see step 8 above). My
   recommendation: bundle — once WAL is durable, idempotency should
   match, and the work is small.

4. **Empty Write rejection vs. silent no-op.** I lean reject with
   an error so caller bugs surface immediately.

5. **Whether to log WAL byte size per write as a metric.** Useful for
   capacity planning; adds a histogram bucket.

## When this slice is done

- `submit(Write)` is functionally equivalent to `submit_encode` /
  `submit_forget` / `submit_link` / `submit_unlink` for WAL durability
  and subscribe-replay. The only remaining feature gap is HNSW + arena
  for UpsertMemory (P3d).
- Multi-phase writes commit atomically with respect to crash recovery.
- A 30-second `kill -9` test against a running server with sustained
  writes shows no data loss on restart (modulo the HNSW gap — vectors
  get re-derived by maintenance).
- The path is ready for P4 to migrate the first wire handler
  (`handle_link` is a good candidate — no HNSW dependency).
