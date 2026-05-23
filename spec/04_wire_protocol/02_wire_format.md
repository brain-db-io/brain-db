# 04.02 Wire Format

The wire format covers two layers: the 32-byte frame header that prefixes every frame, and the payload encoding (rkyv for structured data, bytemuck for raw vector bytes).

> The opcode is a big-endian `u16` (bytes 5–6) and `flags` is a single byte (byte 7). The namespace byte (high byte of opcode) is `0x00` for substrate ops, `0x01` for typed-graph ops — see [`03_opcodes.md`](03_opcodes.md).

## Frame Header

### 1. The 32-byte header

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|              magic = "BRN0" (4 bytes)                         |
+---------------+-------------------------------+---------------+
|   version (8) |          opcode (16)          |   flags (8)   |
+---------------+-------------------------------+---------------+
|                  header_crc32c (32)                           |
+---------------------------------------------------------------+
|                     stream_id (32)                            |
+---------------------------------------------------------------+
|   payload_len (24, big-endian)                |   reserved(8) |
+-----------------------------------------------+---------------+
|                  payload_crc32c (32)                          |
+---------------------------------------------------------------+
|                       reserved (32)                           |
+---------------------------------------------------------------+
|                       reserved (32)                           |
+---------------------------------------------------------------+
```

Total: 32 bytes.

Field-by-field:

| Bytes | Field | Type | Purpose |
|---|---|---|---|
| 0–3 | `magic` | 4 ASCII chars | Identifies a Brain frame: `"BRN0"` (0x42 0x52 0x4E 0x30) |
| 4 | `version` | `u8` | Protocol version. Initially 1; bumps on incompatible changes. |
| 5–6 | `opcode` | `u16` (big-endian) | The operation type. High byte = namespace (0x00 substrate, 0x01 typed graph); low byte = op index. See [`03_opcodes.md`](03_opcodes.md). |
| 7 | `flags` | `u8` | Frame-level flags (see §2). |
| 8–11 | `header_crc32c` | `u32` | CRC32C of bytes 0–7 plus bytes 12–31 (i.e., the rest of the header excluding this field, treated as zero during computation). |
| 12–15 | `stream_id` | `u32` | The stream this frame belongs to (see [`06_streaming.md`](06_streaming.md)). |
| 16–18 | `payload_len` | `u24` (24-bit big-endian) | Payload length in bytes; max 16,777,215 (16 MiB - 1). |
| 19 | reserved | `u8` | Must be zero; reserved for future expansion. |
| 20–23 | `payload_crc32c` | `u32` | CRC32C of the payload (zero if `payload_len = 0`). |
| 24–31 | reserved | 8 bytes | Reserved for future expansion. Must be zero. |

All multi-byte integers are **big-endian**.

### 2. Flags

The 8-bit `flags` field encodes per-frame metadata:

```
bit 7   6   5   4   3   2   1   0
   +---+---+---+---+---+---+---+---+
   |EOS|MPL|CMP|       reserved    |
   +---+---+---+---+---+---+---+---+
```

| Bit | Name | Meaning |
|---|---|---|
| 7 | `EOS` | End of stream — last frame of this stream. |
| 6 | `MPL` | Multi-payload — payload spans multiple frames; concatenate to reconstruct. |
| 5 | `CMP` | Compressed — payload is zstd-compressed. (Reserved; not currently used.) |
| 4-0 | reserved | Must be zero. |

The flags `EOS` and `MPL` are mutually compatible: a multi-payload final frame has both. A single-frame final response has only `EOS`.

### 3. Field details

#### 3.1 magic

The 4-byte sequence `0x42 0x52 0x4E 0x30` (ASCII `"BRN0"`).

The `0` in the trailing position is a generation marker. If a fundamentally incompatible new framing is ever needed, a marker like `"BRN1"` would be used. Within `"BRN0"`-framed protocols, the `version` field handles compatible evolution.

A reader that sees a different magic on the first frame of a connection MUST close the connection — this isn't a Brain frame.

#### 3.2 version

The protocol version. Currently **1**.

The version is checked at handshake time (the `HELLO` frame's negotiation). Once negotiated, all subsequent frames on the connection MUST have the same version. A frame with a different version is a protocol error and the connection is closed.

#### 3.3 opcode

The operation type. See [`03_opcodes.md`](03_opcodes.md) for the full opcode table (substrate primitives at `0x00xx` and typed-graph operations at `0x01xx`).

The opcode is a big-endian `u16` split into two bytes:

- **High byte — namespace.**
  - `0x00` — substrate (cognitive primitives, connection management, admin).
  - `0x01` — typed graph (schema, entities, statements, relations, queries, extractors).
  - `0x02`–`0xFF` — reserved for future namespaces.
- **Low byte — operation index within the namespace.** Low byte `< 0x80` is server-bound (C → S, request); low byte `≥ 0x80` is client-bound (S → C, response). The direction rule applies independently within each namespace.

Examples: `0x0020` is substrate `ENCODE_REQ`; `0x00A0` is substrate `ENCODE_RESP`; `0x0130` is knowledge `ENTITY_CREATE` (request); `0x01B0` is knowledge `ENTITY_CREATE_RESP`.

#### 3.4 payload_len

The length of the payload in bytes, as a 24-bit big-endian unsigned integer. Maximum: 16,777,215 (just under 16 MiB).

A frame with `payload_len = 0` has no payload bytes after the header. Both `EOS`-only frames and pure ACK frames typically have empty payloads.

For payloads larger than 16 MiB, use multi-payload framing: split the payload across multiple frames, all but the last having `MPL` set, and concatenate at the receiver.

#### 3.5 stream_id

The stream this frame belongs to. See [`06_streaming.md`](06_streaming.md) for the streaming model.

`stream_id = 0` is reserved for connection-level frames (PING, PONG, BYE, HELLO, WELCOME, AUTH, AUTH_OK, error frames not associated with a stream).

`stream_id` 1, 3, 5, ... (odd) are client-allocated. `stream_id` 2, 4, 6, ... (even) are reserved for server-initiated streams in the future; not currently used.

#### 3.6 header_crc32c

CRC32C of the header. Computed over bytes 0–7 followed by bytes 12–31 — i.e., the entire header minus the `header_crc32c` field itself. During computation, the `header_crc32c` field is treated as if zero (or omitted).

The polynomial is the [Castagnoli polynomial 0x1EDC6F41](https://en.wikipedia.org/wiki/Cyclic_redundancy_check). Hardware acceleration is available on x86 (SSE 4.2) and ARM (CRC32 extension); see [01.05 Hardware](../01_architecture/05_hardware_and_targets.md) §2.1.

A frame with mismatched `header_crc32c` is treated as corruption: the receiver MUST close the connection with a `BadFrame` error.

#### 3.7 payload_crc32c

CRC32C of the payload. Computed over the payload bytes (after the 32-byte header).

If `payload_len = 0`, `payload_crc32c` MUST also be zero.

A frame with mismatched `payload_crc32c` is corruption: connection close with `BadPayload` error.

#### 3.8 Reserved fields

The 8 bytes at positions 24–31 and the single byte at position 19 are reserved for future use. They MUST be zero in the current wire version. Receivers MUST verify they are zero; non-zero values are protocol errors.

The reserved space provides room for future additions:

- More flags.
- Per-frame priority indicators.
- Tracing IDs (alternatives to OpenTelemetry's W3C trace context).

The exact use is intentionally open; the framing keeps flexibility while remaining stable.

### 4. Frame parsing

#### 4.1 The reader's algorithm

```
loop:
    read 32 bytes (header)
    verify magic == "BRN0"
    verify version matches negotiated version
    verify reserved fields are zero
    verify header_crc32c
    if payload_len > 0:
        read payload_len bytes
        verify payload_crc32c
    dispatch by opcode and stream_id
```

The reader MUST NOT trust any field until the header CRC is verified. Out-of-bounds payload_len, garbage opcodes, etc., are all caught by the header CRC check (assuming the CRC was set correctly by the sender; if the CRC matches but the field is invalid, it's a sender bug, not corruption).

#### 4.2 Why two CRCs

Two checksums seem redundant but serve different purposes:

- **`header_crc32c`** validates the header, so the reader can trust `payload_len` and `payload_crc32c` enough to read the payload.
- **`payload_crc32c`** validates the payload, catching corruption that occurred after the header was written.

A single CRC over both header and payload would require buffering the entire payload before the reader could trust it. Two CRCs let the reader stream-process: parse header, allocate buffer, read payload, validate.

#### 4.3 Why CRC32C, again

Already justified in [`01_design.md`](01_design.md) §7. CRC32C is fast, hardware-accelerated, adequate for transmission-error detection. TLS handles adversarial concerns; CRCs handle accidental corruption.

### 5. Frame size

Minimum frame size: 32 bytes (header only, no payload). Maximum frame size: 32 + 16,777,215 = 16,777,247 bytes.

The 16 MiB limit on payload prevents pathological frames that would block the connection while being read. For larger transfers, multi-payload framing (the `MPL` flag) is used.

### 6. Multi-payload frames

When a logical message exceeds 16 MiB, the sender splits it into multiple frames:

- All frames have the same `stream_id` and `opcode`.
- All but the last have `MPL = 1`.
- The last frame has `MPL = 0` (and may have `EOS = 1` if it's the end of the stream).
- The receiver concatenates the payloads in receive order.

This is rarely needed in practice. Most operations produce small frames; multi-payload kicks in only for very large `RECALL` results (10000+ memories) or large bulk transfers.

### 7. Frame examples

#### 7.1 PING frame

```
Field            Value
magic            "BRN0"
version          1
opcode           0x0010 (PING)
flags            0x00
header_crc32c    <computed>
stream_id        0
payload_len      0
reserved         0
payload_crc32c   0
reserved         0..0
```

32 bytes total. No payload.

#### 7.2 RECALL request frame

```
Field            Value
magic            "BRN0"
version          1
opcode           0x0021 (RECALL_REQ)
flags            0x00
header_crc32c    <computed>
stream_id        7 (client-allocated, odd)
payload_len      <size of rkyv-encoded RecallRequest>
reserved         0
payload_crc32c   <computed>
reserved         0..0
```

Plus the rkyv-encoded RecallRequest payload. See [`05_frame_layouts.md`](05_frame_layouts.md) for layout.

#### 7.3 RECALL response frame (intermediate, with one result)

```
Field            Value
magic            "BRN0"
version          1
opcode           0x00A1 (RECALL_RESP)
flags            0x00  (not EOS yet)
header_crc32c    <computed>
stream_id        7
payload_len      <size of one MemoryResult>
reserved         0
payload_crc32c   <computed>
reserved         0..0
```

#### 7.4 RECALL response frame (final, EOS)

```
Field            Value
magic            "BRN0"
version          1
opcode           0x00A1 (RECALL_RESP)
flags            0x80  (EOS)
header_crc32c    <computed>
stream_id        7
payload_len      0  (or final batch of results)
reserved         0
payload_crc32c   <computed>
reserved         0..0
```

### 8. Endianness summary

| Field | Endianness |
|---|---|
| `magic` | byte order (literal ASCII) |
| `version` | single byte |
| `opcode` | big-endian `u16` |
| `flags` | single byte |
| `header_crc32c` | big-endian `u32` |
| `stream_id` | big-endian `u32` |
| `payload_len` | big-endian `u24` |
| reserved bytes | byte order; must be zero |
| `payload_crc32c` | big-endian `u32` |

Within payloads, encodings (rkyv structures and bytemuck-cast vectors) have their own conventions; see the payload encoding section below.

## Payload Encoding

### 9. Two encodings, one payload

A single payload may carry both:

- **Structured data** (memory IDs, scores, salience, metadata) — encoded with [rkyv](https://github.com/rkyv/rkyv).
- **Raw vector bytes** (`f32` arrays representing embeddings) — appended after the rkyv data, accessed via [bytemuck](https://github.com/Lokathor/bytemuck).

The split achieves zero-copy reads on both components. rkyv's structured access works on the rkyv portion; bytemuck's `cast_slice<u8, f32>` on the trailing bytes gives direct access to the vector data without decoding.

### 10. The payload layout

A typical payload:

```
+----------------------------------+
| rkyv-encoded structure (N bytes) |
+----------------------------------+
| raw f32 vectors (M bytes)        |
+----------------------------------+
```

The rkyv structure includes a field giving the offset (relative to the start of the payload) and length of the raw vector data. This lets the reader find the vector portion without scanning.

```rust
#[derive(rkyv::Archive)]
struct RecallRequestPayload {
    cue_text: String,
    cue_vector_offset: u32,    // 0 if cue is text-only; non-zero if vector pre-supplied
    cue_vector_dim: u16,        // 0 if no vector; 384 if present
    top_k: u32,
    confidence_threshold: f32,
    context_filter: Option<ContextFilter>,
    // ... other fields
}
```

After the rkyv data, the raw vector bytes (if any) follow.

For payloads with no vector data (e.g., metadata-only requests), `cue_vector_offset = 0` and the raw section is empty.

### 11. rkyv

[rkyv](https://github.com/rkyv/rkyv) is a Rust serialization framework providing zero-copy deserialization. Self-described as "a zero-copy deserialization framework for Rust."

#### 11.1 What rkyv provides

rkyv lets the receiver access deserialized data without copying it. The bytes on the wire are the same bytes the program reads as a struct (modulo a small header). For a 1 KiB structured payload with many fields, this saves the copy and the per-field decode that Protobuf would perform.

The trade-off is that the on-wire format is rkyv-specific — third-party readers (without an rkyv library) can't easily decode it. The protocol accepts this; SDKs include rkyv.

#### 11.2 rkyv version pinning

The protocol pins to a specific rkyv version (initially 0.7.x). Within a major rkyv version, format compatibility is preserved. Bumping rkyv to a new major version requires bumping the wire-protocol version.

#### 11.3 What rkyv-encoded data looks like

rkyv writes the data structure followed by a small trailer (the "root pointer") that lets the reader find the start of the encoded structure within the buffer. The reader decodes by:

1. Reading the buffer.
2. Asking rkyv for the archived view at the buffer's end.
3. Accessing fields via the archived view; no allocation, no copy.

The buffer is the rkyv portion of the payload. The reader knows the rkyv portion's size from the rkyv root pointer (and from the difference between `payload_len` and the trailing vector bytes).

#### 11.4 Validation

rkyv supports validation: checking that an archive is well-formed before accessing it. Brain uses rkyv's validation to catch malformed data (which would be a protocol error, not just garbage data).

The validation cost is small (~100 ns for typical payloads) and worth paying for safety.

### 12. bytemuck

[bytemuck](https://github.com/Lokathor/bytemuck) provides safe bit-cast operations between types of compatible memory layout.

#### 12.1 What bytemuck is used for

The raw vector portion of a payload is a sequence of `f32` values, each 4 bytes. bytemuck's `cast_slice<u8, f32>` reinterprets the byte slice as an `f32` slice without copying:

```rust
let vector_bytes: &[u8] = &payload[vector_offset..vector_offset + vector_byte_len];
let vector: &[f32] = bytemuck::cast_slice(vector_bytes);
```

The reader gets a `&[f32]` directly into the network buffer. No allocation, no decode loop, no per-element overhead.

#### 12.2 Endianness for vectors

Vectors use **little-endian** `f32`, matching the byte layout of common CPUs (x86 and ARM). Big-endian would force a byte swap on the hot path, which the protocol avoids.

The endianness mismatch with the frame header (big-endian) is internally consistent within the protocol's frame: header in big-endian, structured payload data in rkyv's native (little-endian on most platforms), vector bytes in little-endian. The reader handles each section appropriately.

#### 12.3 Alignment

`f32` alignment is 4 bytes on all target architectures. The rkyv portion of the payload may end at any byte boundary; the rkyv portion is padded to a multiple of 4 bytes so the vector portion starts aligned.

The padding bytes are zero. The padding length (0 to 3 bytes) is determined by the rkyv portion's size; the reader skips the padding to reach the vector portion.

#### 12.4 Multiple vectors per payload

For payloads with multiple vectors (e.g., `RECALL` results carrying multiple memory vectors), the vectors are concatenated in the raw section. The structured section indexes into the raw section via offset and length per vector.

```
+-----------------------+
| rkyv portion          |
|   results: [          |
|     { vec_offset: 0,  |
|       vec_len: 1536 } |
|     { vec_offset: 1536|
|       vec_len: 1536 } |
|     ...               |
|   ]                   |
+-----------------------+
| padding (0-3 bytes)   |
+-----------------------+
| f32 vector 0 (1536 b) |
+-----------------------+
| f32 vector 1 (1536 b) |
+-----------------------+
| ...                   |
+-----------------------+
```

Vectors are typically same-size (384 dims = 1536 bytes). Mixed-size vectors are supported via the explicit length field but unusual.

### 13. The full payload format

A frame's payload has the structure:

```
[rkyv-encoded header]  [rkyv root pointer]  [padding]  [raw vector bytes]
```

The rkyv root pointer is at the end of the rkyv portion (rkyv's convention). The reader:

1. Reads the entire payload (`payload_len` bytes).
2. Validates `payload_crc32c`.
3. Locates the rkyv root pointer at offset N (where N is set by rkyv's encoding).
4. Accesses the archived structure, including the `vector_offset` and `vector_len` fields.
5. Casts the raw section to `&[f32]` via bytemuck.

### 14. Why not just rkyv for everything

The structured part uses rkyv. Why not put vectors in the rkyv structure too?

A vector field in rkyv would be encoded element-by-element. For 384-dim `f32` vectors, that's 384 fields per vector with rkyv overhead per field. Even with rkyv's efficient encoding, the overhead is non-trivial.

Putting vectors in the raw section bypasses this. The vectors are just bytes; the reader gets a slice without overhead.

The architectural symmetry: structured data goes through rkyv (whose strength is structured access), bulk data goes through bytemuck (whose strength is bulk byte access). Each tool for its own job.

### 15. Encoding the cue vector for ENCODE_VECTOR_DIRECT

`ENCODE_VECTOR_DIRECT` is the power-user opcode that lets clients send pre-computed vectors. Its payload:

```rust
struct EncodeVectorDirectPayload {
    text: String,
    vector_offset: u32,         // offset to the vector in the raw section
    vector_dim: u16,            // expected: 384
    model_fingerprint: [u8; 16],
    context_id: ContextId,
    salience_hint: f32,
    request_id: RequestId,
}
```

Followed by the raw vector. The model fingerprint identifies which embedding model produced the vector; the server validates that fingerprint matches a known model.

### 16. Encoding RECALL request

A request without a pre-supplied vector:

```rust
struct RecallRequestPayload {
    cue_text: String,
    cue_vector_offset: u32,     // 0 — no pre-supplied vector
    cue_vector_dim: u16,         // 0
    top_k: u32,
    confidence_threshold: f32,
    context_filter: Option<Vec<ContextId>>,
    age_bound_unix_nanos: Option<u64>,
    kind_filter: Option<Vec<MemoryKind>>,
    request_id: Option<RequestId>,
}
```

Followed by no raw vector data.

A request with pre-supplied cue vector:

```rust
struct RecallRequestPayload { ... cue_vector_offset: <set> ... cue_vector_dim: 384 ... }
```

Followed by 1536 bytes of vector data.

### 17. Encoding RECALL response

Each response frame carries one result (or a small batch):

```rust
struct RecallResponsePayload {
    is_final: bool,             // matches the EOS flag, redundantly
    results: Vec<MemoryResult>,
}

struct MemoryResult {
    memory_id: MemoryId,
    text: String,
    similarity_score: f32,
    confidence: f32,
    salience: f32,
    kind: MemoryKind,
    context_id: ContextId,
    created_at: u64,
    vector_offset: u32,         // offset into raw section, if vector included
    vector_dim: u16,             // 384 if included, 0 otherwise
}
```

Followed by 0 or more vectors in the raw section.

By default, vectors are NOT included in `RECALL` responses. Clients receive `vector_offset = 0` and have no vectors. The `include_vectors` flag in the request enables vector return; when set, each result's vector is included in the raw section.

### 18. Compression

The frame header reserves a `CMP` flag for compression. Not currently used.

If a future major version adds compression (probably zstd over the entire payload, or just the rkyv portion), the flag is set and the receiver decompresses before parsing.

For the current wire version, all payloads are uncompressed.

### 19. Payload size estimation

A typical encode payload:

- rkyv structure: ~150–200 bytes (text varies)
- vector data: 0 (text-only encode) or 1536 bytes (vector pre-supplied)
- total: ~150–2000 bytes

A typical recall request:

- rkyv structure: ~80–150 bytes (cue text varies)
- vector data: 0 (typical)
- total: ~80–150 bytes

A typical recall response (10 memories, no vectors):

- rkyv structure: ~2 KiB (10 × ~200 bytes per result)
- vector data: 0
- total: ~2 KiB

A recall response with vectors (10 memories, vectors included):

- rkyv structure: ~2 KiB
- vector data: 10 × 1536 = 15,360 bytes
- total: ~17 KiB

These are well within the 16 MiB single-frame limit. Multi-payload framing is reserved for unusual cases.

### 20. Typed-graph payload conventions

The `0x01xx` (typed-graph) opcodes reuse the payload encoding rules: rkyv 0.7 with `check_bytes`, big-endian multi-byte integers, CRC32C over the body, 16 MiB − 1 hard cap, `MPL` for multi-frame logical payloads. This section documents the typed-graph-specific conventions for what goes *inside* an rkyv body.

#### 20.1 Opaque blob fields

Several typed-graph structs carry `Vec<u8>` fields that are not interpreted by the wire layer:

| Field | Carrier | Schema-aware decode by |
|---|---|---|
| `EntityCreateRequest.attributes_blob` | entity ops | the schema validator |
| `EntityView.attributes_blob` | entity reads | the SDK typed accessor |
| `RelationCreateRequest.properties_blob` | relation ops | the schema validator |
| `RelationView.properties_blob` | relation reads | the SDK typed accessor |
| `StatementValueWire::Blob(_)` (inner) | statement values | application-level |
| `SchemaUploadRequest.schema_document` (String) | schema upload | parser |

##### 20.1.1 Inner encoding

`attributes_blob` and `properties_blob` are themselves rkyv-encoded maps:

```rust
// Logical shape (decoded after schema validation):
pub type AttributesMap = BTreeMap<String, StatementValueWire>;
```

The wire layer treats them as bytes. The schema validator runs:

```rust
let map: AttributesMap = rkyv::check_archived_root::<AttributesMap>(blob)?;
let validated = schema.validate_attributes(entity_type, &map)?;
```

before the redb commit. Validation failures surface as `EntityTypeMismatch` (schema-aware).

##### 20.1.2 Why nested rkyv

Letting the inner shape be rkyv means SDK code can deserialize attributes once and pattern-match against typed accessors. The alternative (e.g. JSON or protobuf inside the rkyv outer struct) adds a second codec to the client and server. One codec, one validation pass.

##### 20.1.3 Size cap

Each opaque blob ≤ 64 KiB (see [`07_error_handling.md`](07_error_handling.md) §16.9.1). Above that, callers split the payload into auxiliary records (statements with `EvidenceRef::Overflow`, etc.) or use a future "BLOB_PUT" pathway.

#### 20.2 Evidence encoding

Statements carry an `EvidenceRefWire`:

```rust
pub enum EvidenceRefWire {
    Inline(Vec<u128>),     // ≤ 8 MemoryIds
    Overflow(WireUuid),    // EvidenceOverflowId
}
```

**Inline path.** `Inline` carries up to 8 packed `MemoryId`s as `u128` values (the same packing used in substrate ops). Cheap and zero-decode. Reject on the server if `len > 8`.

**Overflow path.** For larger evidence sets, the client pre-creates an `evidence_overflow` row out-of-band (via a future `EVIDENCE_PUT` opcode; today only the worker pipeline writes these rows) and references its UUIDv7 in `EvidenceRef::Overflow`. Reads dereference transparently.

**No middle ground.** There's deliberately no "13 inline" or "32 inline" tier. Either ≤ 8 (the common case for hand-authored statements) or overflow. Avoids tier-boundary edge cases in the storage layer.

#### 20.3 Predicate strings

`predicate` fields are wire-carried as `String` in their canonical `"namespace:name"` form. The server interns them into a `predicates` redb table on first encounter (per [`../02_data_model/00_purpose.md`](../02_data_model/00_purpose.md)). Subsequent reads emit the same canonical form back to the wire.

**Why strings, not interned `u32`?** Convenience for SDKs. A client constructing a `StatementCreateRequest` shouldn't have to look up a `PredicateId` first. The intern step happens server-side on the create path.

**Trade-off.** ~20-40 bytes per statement frame for the predicate string vs ~4 bytes for an interned id. Acceptable for current scale; revisit if `STATEMENT_LIST` streaming becomes bandwidth-bound at high QPS.

#### 20.4 Time fields

All time fields are **unix nanoseconds**, `u64`.

**Sentinel zero for "absent".** `valid_from_unix_nanos = 0`, `valid_to_unix_nanos = 0`, `event_at_unix_nanos = 0` mean "absent / not applicable" — not "January 1, 1970 00:00:00 UTC". Anyone encoding the unix epoch literally should encode as `1` ns instead (or accept the loss of one ns precision).

**Why not `Option<u64>`?** Same reasoning as [§01/§12](01_design.md) — `Option<u64>` archived directly via rkyv is awkward. Sentinel zero is simpler. Documented per-field where it matters.

#### 20.5 Pagination cursors

`ENTITY_LIST`, `STATEMENT_LIST`, `RELATION_LIST_*`, `SCHEMA_LIST`, `EXTRACTOR_LIST` all carry an opaque `Vec<u8>` cursor field for continuation. The shape is server-defined:

```rust
// Currently : opaque rkyv blob containing the last seen key in the
// query's primary index. Concretely for ENTITY_LIST it's the last EntityId scanned,
// for STATEMENT_LIST it's a (subject, predicate, statement_id) triple, etc.
```

**Why opaque?** Clients shouldn't depend on the cursor's internal shape — it's free to change between phases as indexes evolve. The wire shape is just "give me back what the server gave you, and you'll get the next page".

**Cap.** ≤ 1 KiB. Malformed cursors (e.g. an `ENTITY_LIST` cursor fed to `STATEMENT_LIST`) error out with `InvalidArgument`.

**Stability across schema changes.** A cursor issued under schema version N is not guaranteed valid under schema version N+1. Clients that span a `SCHEMA_UPDATED` event should restart their list scan from cursor `Vec::new()`.

#### 20.6 Sentinel and reserved fields summary

For implementers, the complete list of "sentinel zero means absent" fields in typed-graph bodies:

| Field | Carrier | Sentinel |
|---|---|---|
| `EntityView.merged_into` | entity reads | `[0; 16]` |
| `EntityView.flags & TOMBSTONED` | entity reads | bit clear |
| `EntityResolveResponse.audit_id` | resolver | `[0; 16]` |
| `StatementView.subject_pending_audit_id` | statement reads | `[0; 16]` |
| `StatementView.superseded_by` | statement reads | `[0; 16]` |
| `StatementView.supersedes` | statement reads | `[0; 16]` |
| `StatementView.event_at_unix_nanos` (Fact/Pref) | statements | `0` |
| `StatementView.valid_from_unix_nanos`, `valid_to_unix_nanos` | statements | `0` |
| `RelationView.superseded_by` | relations | `[0; 16]` |
| `RelationView.valid_from_unix_nanos`, `valid_to_unix_nanos` | relations | `0` |

Reserved `u8` / `u32` byte / word fields that MUST be zero: none in typed-graph bodies (the frame header carries the only reserved bytes; see §1 above).

#### 20.7 Large-blob policy

Typed-graph bodies carry blobs ≤ 64 KiB inline. Blobs ≥ 64 KiB use:

- **For evidence:** `EvidenceRef::Overflow(EvidenceOverflowId)` referencing a separate `evidence_overflow` row.
- **For other large payloads** (e.g. an embedding blob): currently no typed-graph opcode carries one directly; future ops may use a "BLOB_PUT then reference by id" pattern.

The 64 KiB cap is per-blob, not per-frame. A frame may carry multiple ≤ 64 KiB blobs as long as the total fits in the 16 MiB - 1 frame budget. Multi-payload framing (the `MPL` flag) extends beyond a single frame's budget if needed.

---

*Continue to [`03_opcodes.md`](03_opcodes.md) for the full opcode table.*
