# Phase 2 — Task 2.1: `Lsn` newtype and `WalRecord` framing

**Classification:** trivial (pure data + CRC, no syscalls).

**Spec:** `spec/05_storage_arena_wal/05_wal_records.md` (authoritative).

## Layout (LE)

```
record header (32 bytes):
   0..8   lsn               u64 LE
   8      record_type       u8
   9      flags             u8
  10..12  reserved          [u8; 2]  (zero)
  12..16  payload_length    u32 LE
  16..24  timestamp_ns      u64 LE
  24..32  agent_id_lo64     u64 LE

payload (variable, exactly payload_length bytes)

record footer (8 bytes):
   0..4   payload_crc32c    u32 LE   (CRC32C over header + payload)
   4..8   reserved          [u8; 4]  (zero)

total = 32 + payload_length + 8
```

## In-memory shape

```rust
pub struct Lsn(u64);                            // next/Display/Ord

pub struct WalRecord {
    pub lsn: Lsn,
    pub kind: WalRecordKind,                    // unit enum (stub for 2.1)
    pub flags: u8,
    pub timestamp_ns: u64,
    pub agent_id_lo64: u64,
    pub payload: Vec<u8>,
}

impl WalRecord {
    pub fn encoded_len(&self) -> usize;         // 32 + payload.len() + 8
    pub fn encode_into(&self, out: &mut Vec<u8>);
    pub fn decode_one(buf: &[u8]) -> Result<DecodeOutcome, WalRecordError>;
}

pub enum DecodeOutcome {
    Record { record: WalRecord, consumed: usize },
    Truncated,                                  // not enough bytes — normal at tail
}

pub enum WalRecordError {
    CrcMismatch { expected: u32, actual: u32 },
    UnknownRecordType(u8),
    NonZeroReserved,
    PayloadTooLarge(u32),                       // > MAX_PAYLOAD (16 MiB per spec §05/05 §19)
}
```

## Rules

- **Truncated vs error**: `< 32`, or `< 32 + payload_length + 8`, → `Truncated`. CRC mismatch on a fully-present record → `Err(CrcMismatch)`. Per spec §05/05 §18 + §05/08, recovery treats CRC failure as truncate-here, but at this layer we surface them distinctly so the WalReader can decide.
- **Reserved bytes** must be zero on read (rejected) and zero on write.
- **Max payload** capped at 16 MiB per spec §05/05 §19.
- **CRC** covers header + payload, not the footer.

## Tests

- Round-trip per kind (one record per `WalRecordKind` variant: 15 kinds from spec §05/05 §3).
- Truncation: encode → truncate to 0..encoded_len()-1 → every prefix shorter than full returns `Truncated`.
- CRC corruption: encode, flip a payload byte, expect `CrcMismatch`.
- Unknown record_type byte: expect `UnknownRecordType`.
- Non-zero reserved bytes: expect `NonZeroReserved`.

## Crate gating

Add at the top of `crates/brain-storage/src/lib.rs`:

```rust
#[cfg(not(target_os = "linux"))]
compile_error!(
    "brain-storage requires Linux (mmap/mremap, O_DIRECT, pwritev2(RWF_DSYNC), io_uring). \
     Use the dev container — see README.md 'Development environment'."
);
```

Don't gate the whole crate — that would silently produce an empty crate on non-Linux, which is worse than a clear compile error.

## Files

- `crates/brain-storage/src/lib.rs` (modify: add cfg guard, declare `pub mod wal`)
- `crates/brain-storage/src/wal/mod.rs` (new: re-exports)
- `crates/brain-storage/src/wal/record.rs` (new: Lsn, WalRecord, encode/decode)
- `crates/brain-storage/src/wal/kinds.rs` (new: minimal `WalRecordKind` unit enum, expanded in 2.2)

## Verify gate

`just verify` inside the dev container (host is macOS — compile_error guards prevent host build).
