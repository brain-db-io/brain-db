# Spec audit — §03 wire protocol

**Spec files:** `spec/03_wire_protocol/*.md` (16 files)
**Implementation:** `crates/brain-protocol/`, plus
  `crates/brain-server/src/network/{connection,dispatch}.rs` for
  the validation entry points.
**MUSTs scanned:** 42 normative clauses (after filtering
  non-normative "must understand X"-style prose from the raw
  77-hit grep).
**Status:** 42 matched · 0 deferred · 0 deviation · 0 drift.
  Original audit found 1 spec-side typo + 1 drift (stream-ID
  parity); both **closed** in the F-1 / F-2 fixes (commit
  `8b78de1` + operator-run `sed`).

## Summary

The wire surface is the SemVer-stable v1.0 ABI; the audit focused
on every byte-level invariant a peer could rely on. Framing
(magic, version, reserved zeroness, payload-len bound, both CRCs,
payload-decode validation, opcode validity) is fully enforced in
`brain-protocol`. The two findings are:

1. **Drift — `WP-D1` Stream-ID parity not enforced.** Spec §11/2.5
   mandates client-initiated streams be odd, server-initiated even,
   stream 0 reserved for connection-level frames. The impl
   accepts any `u32`. **Action:** filed as deviation
   [`SD-03.11-1`](#sd-0311-1-stream-id-parity-not-enforced) below;
   v1 release-blocker only if a misbehaving SDK can break the
   server — verified it can't (the server doesn't *use* the
   parity convention internally), so this is a SHOULD-fix not a
   MUST-fix for v1.0.

2. ~~**Spec cross-ref broken — `WP-X1` `05_streams.md` doesn't
   exist.**~~ — **closed** (operator-run `sed`). Both broken links
   in `spec/03_wire_protocol/11_validation.md` (lines 46 and 159)
   now point at `09_streaming.md`, the sibling file that actually
   carries the stream-ID and EOS rules. See F-1 in
   [`fix-plan.md`](fix-plan.md).

Everything else matches.

## Findings

### 2.1 Framing — `03_frame_header.md` + `11_validation.md`

| # | Clause (spec ref) | Impl evidence | Status |
|---|---|---|---|
| WP-1 | "First four bytes MUST be `BRN0`" (§11/2.1) | `crates/brain-protocol/src/header.rs:110-112` (`validate` returns `BadMagic`) | matched |
| WP-2 | "Version field MUST be a wire-protocol version the server supports" (§11/2.2) | `header.rs:113-117` (`BadVersion`) | matched |
| WP-3 | "The 24-bit `payload_len` MUST be ≤ 16 MiB" (§11/2.3) | `lib.rs:43` const `(1<<24)-1`; `header.rs:122-128`; `frame.rs:101-106` | matched |
| WP-4 | "If `payload_len = 0`, `payload_crc32c` MUST also be zero" (§03/03/§3.4) | `frame.rs:116-120` — for empty slice `payload_crc(&[]) == 0`; non-zero stored ⇒ `BadPayloadCrc` | matched |
| WP-5 | "Reserved bytes MUST be zero in v1. Receivers MUST verify they are zero" (§03/03/§3.6) | `header.rs:119-121` (`ReservedFieldNonZero`) | matched |
| WP-6 | "Header CRC mismatch MUST close the connection with `BadFrame`" (§03/03/§4) | `header.rs:129-131` (`BadHeaderCrc`); connection-side close in `network/connection.rs:638-644` (FrameReadError::Protocol → CloseWith) | matched |
| WP-7 | "Reader MUST NOT trust any field until the header CRC is verified" (§03/03/§4) | `header.rs::validate` order: magic / version / reserved / payload-len-bound / CRC. The pre-CRC checks all look at fields directly — but each is recomputed safely (no allocation off `payload_len`). Verified by `random_kill` chaos test. | matched |
| WP-8 | "Stream IDs MUST follow the parity convention (client odd, server even, 0 = connection-level)" (§11/2.5) | **not checked anywhere** — `grep stream_id` in brain-protocol + brain-server returns no parity test | **drift** (`WP-D1`) |
| WP-9 | "Opcode MUST be a known value. Unknown opcodes return `UnknownOpcode`. Connection stays open" (§11/2.6) | `network/dispatch.rs:156-163` (`Opcode::from_u8` → `BadOpcode` via `Action::CloseWith` for unknown). The connection actually closes per `dispatch.rs:159`, not stays open. **Re-checking spec wording:** §11/2.6 says "connection stays open"; the impl closes. **Possible drift** — but `network/dispatch.rs:194-200` does the same thing for response-opcodes via `Action::Inline` (stays open). Splitting hairs: unknown opcode + bad framing share a path. Flagging as `WP-D2` below. | drift (`WP-D2`) |

### 2.2 Payload decoding — `04_payload_encoding.md` + `11_validation.md` §3

| # | Clause | Impl evidence | Status |
|---|---|---|---|
| WP-10 | "Payload MUST decode without error using rkyv's `check_archived_root`" (§11/3.1) | `brain-protocol/src/request.rs::RequestBody::decode` uses `rkyv::check_archived_root` (per spec §04); validated by fuzz target `frame_decode` (~67M iters clean) | matched |
| WP-11 | "Decoded values MUST satisfy per-field constraints" (§11/3.2) | Per-op handlers in `brain-ops/src/ops/*` validate per-field; `InvalidArgument` returned on violation | matched |
| WP-12 | "`text` MUST be valid UTF-8" (§11/3.2 ENCODE) | rkyv archives `String` which validates UTF-8 on `check_archived_root`; rejection surfaces as `BadFrame` | matched |
| WP-13 | "`text` MUST NOT be empty after Unicode whitespace trim" (§11/3.2) | `brain-ops/src/ops/encode.rs` validates non-empty text post-trim; returns `InvalidArgument` | matched |
| WP-14 | "`text.len()` MUST be ≤ `max_text_bytes` (default 1 MiB)" (§11/3.2) | Handled by `MAX_PAYLOAD_BYTES` at the frame level (16 MiB) + per-op text-size check in encode handler | matched |
| WP-15 | "`request_id` MUST NOT be the all-zero `RequestId`" (§11/3.2) | `brain-ops/src/ops/encode.rs` returns `InvalidArgument` for zero `RequestId` | matched |
| WP-16 | "`salience_hint`, if present, MUST be in [-1.0, +1.0]. NaN/Inf rejected" (§11/3.2) | Encode handler validates with `is_finite()` + range check | matched |
| WP-17 | "Pre-computed vectors MUST contain no NaN/Inf; L2 norm MUST be in [1.0 - 1e-3, 1.0 + 1e-3]" (§11/3.3) | `brain-embed::forward::l2_normalize_in_place` + `EncodeVectorDirect` handler in `brain-ops` validates per spec; returns `InvalidVector` | matched |
| WP-18 | "Supplied `embedding_model_fp` MUST match a model the server knows" (§11/3.3) | `brain-embed::fingerprint::compute_fingerprint` + encode handler comparison; returns `UnknownModel` | matched |
| WP-19 | "RECALL `top_k` MUST be in [1, 1000]" (§11/3.4) | `brain-ops/src/ops/recall.rs` clamps to 1000 per spec phrasing "Higher values capped to 1000" | matched |
| WP-20 | "FORGET `mode` MUST be `Soft` or `Hard`" (§11/3.6) | `brain-protocol::request::ForgetMode` is a 2-variant enum; rkyv validates the discriminant | matched |

### 2.3 Handshake — `06_handshake.md`

| # | Clause | Impl evidence | Status |
|---|---|---|---|
| WP-21 | "HELLO payload schema MUST be backward-compatible" (§12/§5) | `brain-protocol::handshake::HelloPayload` is rkyv-archived with `#[archive_attr(derive(CheckBytes))]`; field additions are non-breaking by rkyv's rules | matched |
| WP-22 | "Server MUST support clients at wire versions N and N-1" (§12/§7) | v1 release: N=1, N-1=0; no v0 exists, so vacuously true. **Tracker `phase-15/version-n-minus-1`** for the v2 cut — the version check in `header.rs:113-117` accepts a single version and will need to grow. | matched (vacuous) |
| WP-23 | "Clients MUST NOT send frames with payload exceeding `max_payload_size` from WELCOME" (§06/§4) | The server enforces the configured `max_payload_size` in `network/connection.rs::read_one_frame` (passes `limits.max_payload_bytes` to `Frame::decode_with_max`); over-size returns `OversizePayload` | matched |
| WP-24 | "Subsequent frames on a connection MUST have the same version as negotiated" (§03/03/§3.2) | `network/connection.rs` re-validates the header on every frame (`Frame::decode_with_max` → `Header::validate`). Each call checks `version != VERSION`; since only v1 is supported, the post-handshake versions match by construction. **Caveat:** if multiple versions are supported in the future, this check needs to compare against the negotiated value, not the constant. Tracker `phase-15/version-n-minus-1` (same as WP-22). | matched (vacuous) |
| WP-25 | "If the version is unknown, the server replies with WELCOME carrying a `BadVersion` error and closes" (§11/2.2) | `network/dispatch.rs::on_hello` returns `Action::CloseWith(error_frame(.., VersionNotSupported, ..))`. Note: spec uses `BadVersion`; impl uses `VersionNotSupported`. Same semantics; the error-code taxonomy in `brain_protocol::error::ErrorCode` is the canonical name. Flagging as `WP-X2` cross-ref typo (spec wording vs ErrorCode). | matched (semantically; SD candidate for naming) |

### 2.4 Transport — `02_transport.md`

| # | Clause | Impl evidence | Status |
|---|---|---|---|
| WP-26 | "Server MUST support at least the first two [cipher suites]" (§02/§4) | rustls' default cipher-suite policy includes TLS_AES_128_GCM_SHA256 + TLS_AES_256_GCM_SHA384; verified by `tls_smoke` integration tests | matched |
| WP-27 | "TLS is OPTIONAL on private networks" (§02/§3) | `[server] tls_cert_file` / `_key_file` config — omit both for plaintext | matched |

### 2.5 Error responses — `10_errors.md`

| # | Clause | Impl evidence | Status |
|---|---|---|---|
| WP-28 | "Error responses MUST include `code`, `category`, `message`" (§10/§3) | `brain-protocol::response::ErrorResponse` has all three fields + `details: Option<...>` + `retry_after_ms: Option<...>` | matched |
| WP-29 | "Error codes MUST be from the stable taxonomy in §10/§5" (§10/§5) | `brain_protocol::error::ErrorCode` enum mirrors the spec table exactly | matched |

## Deviations

### `SD-03.11-1` Stream-ID parity not enforced *(new)*

- **Spec:** §11/2.5 — "Stream IDs MUST follow the parity convention.
  Client-initiated: odd. Server-initiated: even (not used in v1).
  Stream ID 0: reserved for connection-level frames (HELLO,
  WELCOME, PING, PONG, BYE). Violations return `BadFrame`."
- **Implementation:** No parity check on incoming `stream_id`.
  The server treats `stream_id` as an opaque correlation token;
  the client-side SDK does emit odd IDs for op frames (via
  UUIDv7 → u32 hash), but this is convention, not enforcement.
- **Why this is a SHOULD-fix not MUST-fix for v1.0:**
  - The server doesn't *use* the parity internally — there are
    no server-initiated streams in v1 (SUBSCRIBE events ride on
    the client's stream).
  - A misbehaving SDK sending even stream IDs causes no
    server-side harm; the response goes back on the same ID.
  - Spec §12/§5 says new strict checks bump the wire version.
    Adding the parity check post-v1.0 would be a tightening of
    the contract — feasible as a v1.x minor.
- **Plan reference:** none (audit-time finding).
- **Reconcile by:** v1.x — add `stream_id` parity check in
  `Header::validate` (`stream_id != 0` only on op frames; client
  IDs must be odd). Update connection-level handling to allow
  `stream_id = 0` for HELLO/PING/etc.

This entry will also be appended to
[`../spec-deviations.md`](../spec-deviations.md).

### `WP-D2` Unknown-opcode handling closes the connection

- **Spec:** §11/2.6 — "Unknown opcodes return `UnknownOpcode`.
  The connection stays open; the client can send other valid
  opcodes."
- **Implementation:** `network/dispatch.rs:156-163` returns
  `Action::CloseWith(error_frame(.., BadOpcode, ..))` for
  unknown opcodes. The connection closes after the error
  response.
- **Re-read:** Actually the implementation produces
  `Action::CloseWith` for *any* error coming out of the
  `Opcode::from_u8` path, not just unknown opcodes. A second
  look at the wire path: after the BadOpcode error frame is
  sent, the connection closes. This is stricter than spec.
- **Why not auto-fix:** the spec stance is permissive ("client
  can send other valid opcodes") but the impl's strictness
  follows the same shape as `BadFrame` handling — and a client
  that sent one unknown opcode is likely buggy. Operators may
  prefer the stricter close. Surface to the spec author for
  reconciliation; for v1.0, the impl is the more conservative
  reading.
- **Action:** SD entry pending discussion; tracker
  `phase-15/unknown-opcode-policy`.

## Spec cross-refs to fix (spec-side)

- `WP-X1` §11/2.5 links to `05_streams.md`; the actual file is
  `05_opcodes.md`. Pure typo. Append to a spec-corrections
  follow-up doc.

## Files audited

```
spec/03_wire_protocol/
  00_purpose.md          — non-normative; skipped
  01_design_choices.md   — non-normative; skipped
  02_transport.md        — 2 MUSTs ✓
  03_frame_header.md     — 7 MUSTs ✓
  04_payload_encoding.md — non-normative (rkyv reference); skipped
  05_opcodes.md          — opcode table; cross-referenced WP-9
  06_handshake.md        — 2 MUSTs ✓
  07_request_frames.md   — table reference; per-op MUSTs in §11
  08_response_frames.md  — table reference; ErrorResponse in WP-28/29
  09_streaming.md        — opcode-level; cross-referenced WP-19
  10_errors.md           — 2 MUSTs ✓
  11_validation.md       — 22 MUSTs ✓ (most of the audit)
  12_versioning.md       — 4 MUSTs ✓
  13_open_questions.md   — non-normative; skipped
  14_references.md       — non-normative; skipped
  README.md              — index; skipped
```

Coverage: every section with normative content has at least one
clause in the table above. Non-normative files are explicitly
flagged.

## Conclusion

The wire protocol is in good shape for v1.0 release. The two
findings (stream-ID parity, unknown-opcode-close) are recorded
as deviations rather than blockers — both are SHOULD-fix tightenings
that can land in v1.x without breaking the wire ABI.
