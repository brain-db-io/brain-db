# Phase 2 — Task 2.2: Typed `WalPayload` per spec §05/05

**Classification:** moderate (15 variants × byte-level encoding/decoding per spec, plus integration with the 2.1 framing layer).

**Spec:** `spec/05_storage_arena_wal/05_wal_records.md` §§5–16 (authoritative byte layouts).

## Architectural decision

The phase doc 2.2 says "WalRecordKind … each carries the spec's payload schema". Rather than retrofit data variants onto the discriminator enum (which would invalidate 2.1's framing tests and conflate "what byte goes in the type field" with "what is the typed meaning"), keep the two layers separate:

- `wal/kinds.rs` (from 2.1): `WalRecordKind` is a unit enum mirroring the wire `record_type` byte. Untouched.
- `wal/record.rs` (from 2.1): `WalRecord` is the byte-framing layer (`payload: Vec<u8>`). Untouched.
- `wal/payload.rs` (NEW, this task): `WalPayload` is the typed enum, one variant per spec'd kind, each carrying a per-variant payload struct. Has `encode_to_bytes` / `decode(kind, bytes)` for the spec-defined layouts.
- `WalPayload::kind()` returns the matching `WalRecordKind`. A bridge `WalRecord::from_typed(...)` / `WalRecord::typed_payload(&self)` joins the two layers.

This way: the framing layer continues to work for any opaque payload (we'll need this anyway when SUBSCRIBE forwards records or the audit tool inspects unknown future-version kinds). The typed layer interprets the bytes for callers who want them.

## Per-variant byte layouts (LE everywhere unless noted; MemoryId is BE per spec §02/03 §2.2)

Mirroring spec §05/05 §§5–16 exactly. For spec ambiguities, defaults documented inline.

```text
Encode (record_type=1):
  memory_id              16 bytes BE        (spec §02/03 §2.2)
  request_id             16 bytes           (UUID raw)
  agent_id               16 bytes           (UUID raw)
  context_id              8 bytes  u64 LE
  kind                    1 byte            (0=Episodic, 1=Semantic, 2=Consolidated)
  salience_initial        4 bytes  f32 LE
  embedding_model_fp     16 bytes
  text_length             4 bytes  u32 LE
  text                   text_length bytes  UTF-8
  vector_dims             2 bytes  u16 LE   (number of f32s; default 384 for BGE-small)
  vector                 vector_dims*4 bytes f32 LE
  edge_count              2 bytes  u16 LE
  edges                  edge_count * EdgeRecord
    EdgeRecord: source(16) target(16) edge_kind(u8) weight(f32 LE) origin(u8)  = 38 bytes

Spec §05/05 §5 mentions FLAG_INCLUDE_VECTOR/FLAG_INCLUDE_EDGES but §4 doesn't
allocate flag bits for them. We always emit both vector and edges (count=0 if
no edges, vector_dims=0 if no vector). vector_dims is a length prefix so this
is forward-compatible with larger embedding models. Spec says "Default:
include the vector"; we honor that.

Forget (record_type=2):
  memory_id              16 bytes BE
  request_id             16 bytes
  mode                    1 byte    (0=soft, 1=hard)
  reason                  1 byte    (0=client, 1=eviction, ...)
  total: 34 bytes

Link (record_type=3):
  source                 16 bytes BE
  target                 16 bytes BE
  edge_kind               1 byte
  weight                  4 bytes f32 LE
  origin                  1 byte    (0=Explicit, 1=AutoDerived)
  total: 38 bytes

Unlink (record_type=4):
  source                 16 bytes BE
  target                 16 bytes BE
  edge_kind               1 byte
  edge_seq                4 bytes u32 LE
  total: 37 bytes

UpdateSalience (record_type=5):
  count                   4 bytes u32 LE
  for i in 0..count:
    memory_id            16 bytes BE
    new_salience          4 bytes f32 LE
    reason                1 byte
  total: 4 + 21*count

  Spec §05/05 §9 says "the payload then carries multiple tuples" when
  coalesced. We always encode as count + tuples (count=1 for a single
  update). Uniform encoding > spec ambiguity.

Reclaim (record_type=6):
  slot_id                 8 bytes u64 LE
  old_version             4 bytes u32 LE
  new_version             4 bytes u32 LE
  total: 16 bytes

Consolidate (record_type=7):
  new_memory_id          16 bytes BE
  source_count            4 bytes u32 LE
  source_memory_ids      source_count * 16 bytes BE
  text_length             4 bytes u32 LE
  text                   text_length bytes
  vector_dims             2 bytes u16 LE
  vector                 vector_dims*4 bytes f32 LE
  embedding_model_fp     16 bytes

UpdateKind (record_type=8):
  memory_id              16 bytes BE
  new_kind                1 byte
  total: 17 bytes

UpdateContext (record_type=9):
  memory_id              16 bytes BE
  new_context_id          8 bytes u64 LE
  total: 24 bytes

CheckpointBegin (record_type=10):
  checkpoint_id           8 bytes u64 LE
  started_at              8 bytes u64 LE
  total: 16 bytes

CheckpointEnd (record_type=11):
  checkpoint_id           8 bytes u64 LE
  durable_lsn             8 bytes u64 LE
  arena_capacity          8 bytes u64 LE
  total: 24 bytes

TxnBegin (record_type=12):
  txn_id                 16 bytes
  expected_record_count   4 bytes u32 LE
  total: 20 bytes

TxnCommit (record_type=13):
  txn_id                 16 bytes
  total: 16 bytes

TxnAbort (record_type=14):
  txn_id                 16 bytes
  reason_code             4 bytes u32 LE
  total: 20 bytes

MigrateEmbedding (record_type=15):
  memory_id              16 bytes BE
  old_fingerprint        16 bytes
  new_fingerprint        16 bytes
  vector_dims             2 bytes u16 LE
  new_vector             vector_dims*4 bytes f32 LE
```

## Errors

```rust
pub enum WalPayloadError {
    /// Bytes ran out mid-field.
    Underrun { needed: usize, had: usize },
    /// Trailing bytes after the structured fields.
    TrailingBytes(usize),
    /// `kind` byte from the header didn't match a known variant. (Surfaces from
    /// `WalRecordKind::from_u8` already; included so callers can use one error
    /// type.)
    UnknownKind(u8),
    /// `MemoryKind` byte is not 0/1/2.
    BadMemoryKind(u8),
    /// `EdgeKind` byte is not 0..=7.
    BadEdgeKind(u8),
    /// `EdgeOrigin` byte is not 0/1.
    BadEdgeOrigin(u8),
    /// `text_length` claims more bytes than remain in the payload.
    BadTextLength(u32),
    /// Text didn't decode as UTF-8.
    BadUtf8,
    /// `vector_dims * 4` overflows the remaining payload, or `vector_dims`
    /// is implausibly large (> 4096 for safety).
    BadVectorDims(u16),
}
```

## Tests (per the spec done-when criteria)

For every variant:
1. **Round-trip**: build the payload, encode, decode, compare. Equality across all fields including floats (use bit-equality, not `==`, to avoid NaN games).
2. **Truncation rejection**: encoding produces N bytes; decoding any prefix of length 0..N-1 returns `Underrun`. (Mirrors the framing-layer test from 2.1 but for the payload layer.)
3. **Trailing-bytes rejection**: append a stray byte; expect `TrailingBytes(1)`.
4. Discriminant-specific failures where the format admits them (`BadMemoryKind`, `BadEdgeKind`, etc.).

## Files

- `crates/brain-storage/src/wal/payload.rs` (new): `WalPayload` enum + per-variant payload structs + encode/decode + tests.
- `crates/brain-storage/src/wal/mod.rs` (modify): re-export `WalPayload`, `WalPayloadError`, payload structs.
- `crates/brain-storage/src/wal/record.rs` (modify): add `WalRecord::from_typed(lsn, flags, timestamp_ns, agent_id_lo64, payload: WalPayload) -> Self` and `WalRecord::typed_payload(&self) -> Result<WalPayload, WalPayloadError>` bridges.
- `crates/brain-storage/Cargo.toml`: no new deps (we use `crc32c`, `thiserror`, `bytemuck` already; `uuid` is transitively available via `brain-core`).

## Verify gate

`cargo test -p brain-storage --all-targets` and `cargo clippy --workspace --all-targets -- -D warnings` inside the dev container.
