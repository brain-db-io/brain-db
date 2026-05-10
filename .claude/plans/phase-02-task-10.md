# Phase 2 — Task 2.10: Recovery driver

**Classification:** heavy. Bridges three storage primitives (arena, WAL, metadata sink), introduces the first user-facing trait (`MetadataSink`), and is the keystone for the random-kill test in 2.11.

**Spec:** `spec/05_storage_arena_wal/08_recovery.md` (replay algorithm), `09_checkpointing.md` §2 + §3 + §11 (durable_lsn semantics), `spec/15_failure_recovery/02_crash_recovery.md` §§4–6 (recovery procedure). `12_open_questions.md`: OQ-ST-3 (gap tolerance — we stay strict).

## 1. Scope

In:

- `MetadataSink` trait — boundary between the storage crate and the future redb-backed metadata store (Phase 3). Two methods: `durable_lsn(&self) -> u64`, `apply(&mut self, lsn, payload) -> Result<()>`. Idempotency is the sink's responsibility (recovery may re-run on the same WAL).
- `InMemoryMetadataSink` — in-process test implementation that records every applied `(lsn, payload)` pair and dedupes by LSN.
- `RecoveryReport` — what the caller learns after recovery.
- `recover(arena, allocator, wal_dir, shard_uuid, metadata_sink) -> Result<RecoveryReport>` — orchestrates the replay loop:
  1. Open `WalReader` over `wal_dir`.
  2. Iterate records in strict LSN order.
  3. Skip records with `lsn <= sink.durable_lsn()`.
  4. Apply to the arena (vector + slot metadata).
  5. Apply to the sink.
  6. Track stats; rebuild the slot allocator at the end.
- TXN buffer state machine per spec §05/08 §6: records inside `TXN_BEGIN`/`TXN_COMMIT` are queued; on `COMMIT` we apply them as a group; on `ABORT` or end-of-WAL with no commit, we discard.

Out:

- **HNSW rebuild.** Spec §05/08 §8 / §15/02 §10 covers rebuilding from arena+metadata. That belongs to `brain-index` (Phase 4); the recovery driver doesn't touch it.
- **`brain-metadata` redb integration.** Phase 3. The `MetadataSink` trait defines the boundary; the redb impl plugs in later.
- **Real-process kill test.** 2.11 — that's a separate sub-task with subprocess spawning.
- **Auto-create arena/WAL on fresh start.** Recovery operates on already-opened `ArenaFile` + already-built `SlotAllocator`. Caller chooses fresh-create vs reopen.
- **Reopen-and-continue.** Recovery is read-only on the WAL (no new records); a follow-up sub-task (or Phase 9 wire-up) will add `Wal::resume(recovered_state)`. Not in 2.10 scope.

## 2. Spec quotes that bind the design

> **§05/08 §2 (high-level procedure):**
> > 1. Open the metadata store (redb). 2. Read the most recent checkpoint marker. 3. Open WAL segments from the checkpoint's durable_lsn forward. 4. Replay records in LSN order: a. validate CRC, b. if CRC fails, this is the truncation point — stop, c. if CRC succeeds, apply.
>
> **§05/08 §5 (apply):** "Each apply is idempotent — if recovery is re-run on the same WAL, the result is the same."
>
> **§05/08 §6 (transactions):** TXN_BEGIN buffers records; TXN_COMMIT applies them; TXN_ABORT or partial transaction at end-of-WAL discards.
>
> **§05/09 §2 (checkpoint marker):** `Checkpoint { durable_lsn, ... }`. "The metadata store has a singleton table holding the most recent checkpoint."
>
> **§15/02 §4 step 5:** "a. Verify CRC. b. Apply to metadata if not already applied (idempotent via LSN). c. Apply to arena if not already applied."
>
> **§15/02 §13 (WAL gap detection):** "If a record is missing (gap in LSN sequence): the substrate detects gaps via LSN continuity check. Refuses to come up." (We get this for free from `WalReader`'s strict LSN check.)

## 3. Design decisions

### 3.1 `MetadataSink` trait shape — one `apply` method, not one per kind

```rust
pub trait MetadataSink {
    /// The LSN through which the sink's state is durable. Recovery skips
    /// records with `lsn <= durable_lsn()`. Returns 0 for a fresh sink.
    fn durable_lsn(&self) -> u64;

    /// Apply one record's effect. Must be idempotent on `lsn` —
    /// recovery may call `apply` with the same `(lsn, payload)` again on
    /// a subsequent recover() invocation.
    fn apply(&mut self, lsn: u64, payload: &WalPayload) -> Result<(), MetadataSinkError>;
}
```

Alternative considered: one method per `WalRecordKind` variant. Rejected — the trait would have 15 methods and force every impl to pattern-match anyway. One `apply` keeps the trait small and lets each impl dispatch internally.

### 3.2 Arena writes happen inside `recover`, not through the sink

Spec separates "apply to metadata" from "apply to arena". Recovery has direct mutable access to the `ArenaFile` and writes the slot bytes (vector + metadata) for ENCODE/CONSOLIDATE/RECLAIM/etc. The sink handles only metadata-store concerns (memory rows, edges, idempotency table, checkpoint marker).

This split matches:
- The arena is mmap'd; writes are pointer arithmetic (in-process). Cheap.
- The metadata is redb-backed; writes are transactional (real I/O). Owned by Phase 3's sink impl.

### 3.3 SlotAllocator rebuilt at the end, not maintained incrementally

After all records are applied, call `SlotAllocator::rebuild_from_arena(&arena)` (sub-task 2.5). The allocator's state is derivable from the arena bytes; rebuilding is O(capacity) and deterministic.

Alternative: maintain the allocator incrementally during replay (`alloc` for ENCODE, `free` for RECLAIM). Rejected:
- ENCODE's slot index is *recorded* (from `record.memory_id`); we don't allocate fresh. `SlotAllocator::alloc` would assign a different idx — breaking the WAL's record.
- The full-scan rebuild is O(capacity) once; doing it incrementally and then verifying is more code for no benefit.

### 3.4 TXN buffer state machine — included but lightly tested

Spec §05/08 §6 mandates transactional buffering. Include the state machine even though the current writer doesn't emit TXN records (callers in Phase 9+ will). The basic machine is ~30 lines.

Tests for the TXN path: build a synthetic WAL with hand-crafted TXN markers and verify partial-transaction discard + complete-transaction apply. Two dedicated tests.

### 3.5 Module placement: `crates/brain-storage/src/recovery.rs`

A top-level module (not inside `wal/` or `arena/`) since recovery spans both. The trait + driver live together.

### 3.6 Error type

```rust
#[derive(thiserror::Error, Debug)]
pub enum RecoveryError {
    #[error("WAL read error: {0}")]
    WalRead(#[from] WalReadError),

    #[error("arena out of capacity: tried to write slot {idx}, capacity is {capacity}")]
    ArenaOutOfCapacity { idx: u64, capacity: u64 },

    #[error("metadata sink rejected record at LSN {lsn}: {source}")]
    SinkError {
        lsn: u64,
        #[source]
        source: MetadataSinkError,
    },

    #[error("vector dimension mismatch at LSN {lsn}: expected {expected}, got {found}")]
    VectorDimMismatch { lsn: u64, expected: usize, found: usize },

    #[error("MemoryId shard {memory_shard} doesn't match arena shard (expected {arena_shard})")]
    ShardMismatch { memory_shard: u16, arena_shard: u16 },

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(thiserror::Error, Debug, Clone)]
pub enum MetadataSinkError {
    #[error("transient: {0}")]
    Transient(String),
    #[error("corruption: {0}")]
    Corruption(String),
}
```

ShardMismatch is a corruption signal — a record's memory_id encodes a shard runtime ID that should match the shard we're recovering. For 2.10's API, the caller passes the shard's runtime ID separately; we cross-check on every record.

Actually — let me defer the shard cross-check to a follow-up. The caller passes the storage UUID (which the WAL reader already checks against the segment header), and we don't have the runtime shard ID handy. Drop `ShardMismatch` for 2.10.

### 3.7 Arena application table

| Record kind | Arena effect |
|---|---|
| `Encode` | slot[memory_id.slot()]: vector = payload.vector (if non-empty), slot_version = memory_id.version(), flags = OCCUPIED, fp = payload.embedding_model_fp_short prefix, created_at/last_modified = record.timestamp, refresh_crc. |
| `Forget` | slot[memory_id.slot()]: flags |= TOMBSTONED, last_modified = record.timestamp, refresh_crc. (If `mode = Hard`: also zero the vector + set HARD_FORGOTTEN — defer; see §6.) |
| `Reclaim` | slot[slot_id]: slot_version = payload.new_version, flags = 0, refresh_crc. |
| `Consolidate` | slot[new_memory_id.slot()]: same as Encode. |
| `UpdateSalience` | None (metadata only). |
| `UpdateKind`, `UpdateContext`, `Link`, `Unlink`, `MigrateEmbedding` | Vector unchanged unless `MigrateEmbedding` (write new_vector); metadata via sink. |
| `CheckpointBegin`, `CheckpointEnd`, `TxnBegin`, `TxnCommit`, `TxnAbort` | None on arena; sink handles checkpoint state and TXN markers via apply() if needed. |

For 2.10 I'll implement Encode/Forget/Reclaim/Consolidate/MigrateEmbedding arena-side; the rest pass through to the sink unchanged. Hard-forget vector-zeroing is a follow-up (not blocking the done-when).

### 3.8 Vector dim guard

The WalPayload allows variable-length vectors (with `vector_dims` length prefix per SD documented in 2.2). The arena's `VECTOR_DIM = 384`. Recovery rejects records whose vector dimension doesn't match the arena's expectation. Returns `VectorDimMismatch`.

Empty vectors (the spec's "exclude vector" path) are accepted — the slot's vector field is left unchanged.

## 4. Architecture

```rust
// crates/brain-storage/src/recovery.rs

pub trait MetadataSink { /* ... */ }
pub enum MetadataSinkError { /* ... */ }
pub enum RecoveryError { /* ... */ }
pub struct RecoveryReport {
    pub records_replayed: u64,
    pub records_skipped: u64,   // skipped because lsn <= durable_lsn
    pub records_discarded: u64, // discarded by TXN_ABORT or partial txn
    pub starting_lsn: u64,
    pub ending_lsn: u64,
    pub last_segment_seq: Option<u64>,
}

pub fn recover(
    arena: &mut ArenaFile,
    wal_dir: &Path,
    shard_uuid: [u8; 16],
    sink: &mut dyn MetadataSink,
) -> Result<(RecoveryReport, SlotAllocator), RecoveryError>;

/// In-memory test sink — records every (lsn, payload) pair, deduping by LSN.
pub struct InMemoryMetadataSink {
    by_lsn: BTreeMap<u64, WalPayload>,
    durable_lsn: u64,
}
impl InMemoryMetadataSink {
    pub fn new() -> Self;
    pub fn with_durable_lsn(lsn: u64) -> Self;
    pub fn applied(&self) -> &BTreeMap<u64, WalPayload>;
    pub fn set_durable_lsn(&mut self, lsn: u64);
}
impl MetadataSink for InMemoryMetadataSink { /* ... */ }
```

### 4.1 The replay loop

```rust
pub fn recover(...) -> Result<(RecoveryReport, SlotAllocator), RecoveryError> {
    let durable_lsn = sink.durable_lsn();
    let reader = WalReader::open(wal_dir, shard_uuid)?;
    let segments = reader.segments();
    let starting_lsn = segments.first().map_or(0, |s| s.starting_lsn);
    let last_seg = segments.last().map(|s| s.segment_seq);

    let mut replayed = 0u64;
    let mut skipped = 0u64;
    let mut discarded = 0u64;
    let mut last_lsn = 0u64;
    let mut txn_state: TxnState = TxnState::Normal;

    for item in reader {
        let record = item?;
        let lsn = record.lsn.raw();
        last_lsn = lsn;
        if lsn <= durable_lsn {
            skipped += 1;
            continue;
        }
        let payload = record.typed_payload().map_err(|e| /* wrap */)?;
        match (&mut txn_state, &payload) {
            (TxnState::Normal, WalPayload::TxnBegin(p)) => {
                txn_state = TxnState::InTxn { txn_id: p.txn_id, buffer: vec![(lsn, payload)] };
            }
            (TxnState::InTxn { txn_id, buffer }, WalPayload::TxnCommit(p)) if p.txn_id == *txn_id => {
                buffer.push((lsn, payload));
                for (b_lsn, b_payload) in buffer.drain(..) {
                    apply(arena, sink, b_lsn, &b_payload)?;
                    replayed += 1;
                }
                txn_state = TxnState::Normal;
            }
            (TxnState::InTxn { .. }, WalPayload::TxnAbort(_)) => {
                let TxnState::InTxn { buffer, .. } = std::mem::replace(&mut txn_state, TxnState::Normal) else { unreachable!() };
                discarded += buffer.len() as u64;
            }
            (TxnState::InTxn { buffer, .. }, _) => {
                buffer.push((lsn, payload));
            }
            (TxnState::Normal, _) => {
                apply(arena, sink, lsn, &payload)?;
                replayed += 1;
            }
        }
    }

    // Partial txn at end of WAL: discard.
    if let TxnState::InTxn { buffer, .. } = txn_state {
        discarded += buffer.len() as u64;
    }

    let allocator = SlotAllocator::rebuild_from_arena(arena);
    Ok((
        RecoveryReport { records_replayed: replayed, records_skipped: skipped,
                          records_discarded: discarded,
                          starting_lsn, ending_lsn: last_lsn, last_segment_seq: last_seg },
        allocator,
    ))
}

fn apply(
    arena: &mut ArenaFile,
    sink: &mut dyn MetadataSink,
    lsn: u64,
    payload: &WalPayload,
) -> Result<(), RecoveryError> {
    apply_to_arena(arena, lsn, payload)?;
    sink.apply(lsn, payload).map_err(|e| RecoveryError::SinkError { lsn, source: e })?;
    Ok(())
}

fn apply_to_arena(arena: &mut ArenaFile, lsn: u64, payload: &WalPayload) -> Result<(), RecoveryError> {
    match payload {
        WalPayload::Encode(p) => write_encoded_slot(arena, lsn, p)?,
        WalPayload::Forget(p) => mark_slot_tombstoned(arena, lsn, p)?,
        WalPayload::Reclaim(p) => reclaim_slot(arena, lsn, p)?,
        WalPayload::Consolidate(p) => write_consolidated_slot(arena, lsn, p)?,
        WalPayload::MigrateEmbedding(p) => migrate_slot_vector(arena, lsn, p)?,
        // The rest are metadata-only.
        _ => {}
    }
    Ok(())
}
```

### 4.2 Arena helpers

Each helper takes `&mut ArenaFile` + the typed payload, computes slot index from `memory_id.slot()`, bounds-checks against `arena.capacity_slots()`, writes the slot fields, calls `refresh_crc()`. Vector dim check inline.

Example for ENCODE:

```rust
fn write_encoded_slot(arena: &mut ArenaFile, lsn: u64, p: &EncodePayload) -> Result<(), RecoveryError> {
    let slot_idx = p.memory_id.slot();
    if slot_idx >= arena.capacity_slots() {
        return Err(RecoveryError::ArenaOutOfCapacity { idx: slot_idx, capacity: arena.capacity_slots() });
    }
    if !p.vector.is_empty() && p.vector.len() != VECTOR_DIM {
        return Err(RecoveryError::VectorDimMismatch { lsn, expected: VECTOR_DIM, found: p.vector.len() });
    }
    let slot = arena.slot_mut(slot_idx);
    if !p.vector.is_empty() {
        slot.vector.copy_from_slice(&p.vector);
    }
    slot.metadata.slot_version = p.memory_id.version();
    slot.metadata.flags = flags::OCCUPIED;
    slot.metadata.embedding_model_fp_short = p.embedding_model_fp;
    // Use the WAL record's timestamp (we don't have it inside the typed payload;
    // could plumb it down, or use 0 — see §6 risks).
    slot.metadata.created_at_unix_nanos = 0;       // placeholder; see below
    slot.metadata.last_modified_at_unix_nanos = 0;
    slot.refresh_crc();
    Ok(())
}
```

**Issue:** the typed `EncodePayload` doesn't carry the WAL record's `timestamp_ns` — the WAL record header does (separately, via `WalRecord { ..., timestamp_ns, ... }`). For arena timestamps we want the WAL record's timestamp. Either:

(a) Thread the `WalRecord` (not just the typed payload) through the apply chain.
(b) Use 0 for now; the metadata sink owns the authoritative timestamp.

Choice: **(a)**. The arena's `created_at_unix_nanos` is part of the slot's CRC-covered region; setting it to a non-deterministic value would make recovery non-idempotent (two recovers would produce different CRCs). Threading the WAL record through gives us a deterministic timestamp.

Updated signature:

```rust
fn apply(arena, sink, record: &WalRecord, payload: &WalPayload) -> Result<...>;
fn apply_to_arena(arena, record: &WalRecord, payload: &WalPayload) -> Result<...>;
```

The replay loop already has `WalRecord` available (we decoded it). Pass the record into apply.

## 5. Files touched

- `crates/brain-storage/src/recovery.rs` (new, ~520 lines incl. tests).
- `crates/brain-storage/src/lib.rs` (add `pub mod recovery;`).
- `docs/phases/phase-02-storage.md` (mark 2.10 done).

No new deps.

## 6. Trade-offs and risks

| Question | Choice | Why |
|---|---|---|
| One `apply` method or one per kind on `MetadataSink` | One `apply(lsn, &WalPayload)` | Small trait surface; impl dispatches internally. |
| Arena writes inside `recover` vs another trait (`ArenaSink`) | Inside recover | The arena is local; no need for a second seam. |
| Allocator: incremental vs rebuild | Rebuild at end | Simpler; deterministic; O(capacity) is fine. |
| TXN handling now or later | Now | Spec mandates it; ~30 lines. |
| Hard-forget vector-zeroing in 2.10 | Defer | Not in any done-when criterion; isolated follow-up. |
| Return signature `(report, allocator)` | Yes | Caller needs the allocator to instantiate a Wal afterwards. |

Risks:

- **Arena timestamps need the WAL record timestamp.** Plumb the `WalRecord` (not just the typed payload) through `apply` so the slot's `created_at` / `last_modified_at` are deterministic across recoveries. (§4.2 discussion.)
- **`p.embedding_model_fp` is 16 bytes; slot's `embedding_model_fp_short` is 16 bytes.** Same shape; direct copy.
- **`MemoryId::slot()` returns `u48` packed in a `u64`.** Bounds-check against `arena.capacity_slots()` (u64).
- **Test idempotency requires the sink to dedupe.** `InMemoryMetadataSink.by_lsn` is a `BTreeMap<u64, WalPayload>`; second `apply(same_lsn, ...)` overwrites the entry with itself — idempotent.
- **MemoryKind / EdgeKind enums lack `Hash`.** Not used in the recovery path; `WalPayload`'s `Clone` is enough.

## 7. Test plan

All tests use `tempfile::TempDir` + 2.4's `ArenaFile` + 2.5's `SlotAllocator` + 2.9's `Wal` to produce a WAL.

### Empty cases (2)

1. **Empty WAL recovery is a no-op.** Create empty WAL dir + fresh arena. `recover` returns 0 records replayed.
2. **WAL with no records past `durable_lsn`.** Sink reports `durable_lsn = 100`; WAL has records 1..=100. All 100 skipped; 0 replayed.

### End-to-end (3 — phase doc done-when)

3. **Recovery after writes (done-when #2).** Write 20 ENCODE records via `Wal::append`. Run `recover`. Sink contains all 20 records; arena slots reflect the writes; allocator's next_fresh matches.
4. **Recovery is idempotent (done-when #3).** Run recover twice; report and sink state are identical after the second call.
5. **`durable_lsn` partial replay.** Sink reports `durable_lsn = 5`; WAL has 10 records; recover replays 6..=10.

### Torn-write tail (1)

6. **Torn tail.** Write 10 records, `set_len` the file to truncate mid-record-10. Recover replays 9 records cleanly; no error.

### Arena application (3)

7. **ENCODE writes the vector + metadata.** After recovery, `arena.slot(slot_idx).vector` matches the WAL record's vector; `slot.is_valid()` is true; `slot.is_occupied()` is true.
8. **FORGET sets TOMBSTONED on the slot.** Write ENCODE then FORGET in the WAL; after recovery, slot's TOMBSTONED bit is set, OCCUPIED still set (spec §05/02 §3.2 "active but tombstoned").
9. **RECLAIM bumps version + clears flags.** Write ENCODE then FORGET then RECLAIM; after recovery, slot's flags = 0 and slot_version matches RECLAIM's `new_version`.

### TXN (2)

10. **Complete transaction applies all records.** Synthesize TXN_BEGIN + 3 records + TXN_COMMIT in a manually-crafted WAL. Recovery applies all 3.
11. **Partial transaction at EOL discards records.** Synthesize TXN_BEGIN + 3 records + (no commit). Recovery discards the 3; sink doesn't see them.

### Error paths (2)

12. **Vector dimension mismatch.** Synthesize ENCODE with `vector.len() = 100` (not 384). Recovery returns `VectorDimMismatch`.
13. **Arena capacity overflow.** Synthesize ENCODE with `memory_id.slot() = 9999` against a 16-slot arena. Recovery returns `ArenaOutOfCapacity`.

**Total: 13 tests.**

## 8. Estimated commit shape

One commit on `feature/brain-storage`:

> `feat(brain-storage): recovery driver (sub-task 2.10)`

Body covers:
- `MetadataSink` trait + `InMemoryMetadataSink`.
- `recover()` orchestration.
- TXN state machine.
- Arena application table.
- WAL record timestamp threading for determinism.
- Test count.

Files: as in §5. No new deps. Verify gate: `cargo fmt --all -- --check && ./scripts/check-skills.sh && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p brain-storage --all-targets` inside the dev container.

---

PLAN READY: see `.claude/plans/phase-02-task-10.md` — confirm to proceed.
