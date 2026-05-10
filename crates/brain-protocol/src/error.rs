//! Protocol-level errors.
//!
//! Minimal taxonomy required by frame-header validation (Task 1.1).
//! The full spec §10 error set is built up across later sub-tasks
//! (`spec/03_wire_protocol/10_errors.md`).

use thiserror::Error;

/// Errors raised while parsing or validating a wire frame.
#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum ProtocolError {
    /// Frame's magic bytes aren't `b"BRN0"`.
    #[error("bad magic: expected b\"BRN0\"")]
    BadMagic,
    /// Frame's version doesn't match the negotiated version.
    #[error("bad version: got {got}, expected {expected}")]
    BadVersion { got: u8, expected: u8 },
    /// Stored header CRC32C doesn't match the recomputed value.
    #[error("bad header crc32c")]
    BadHeaderCrc,
    /// `payload_len` exceeds the 24-bit max.
    #[error("oversize payload: {len} > {max}")]
    OversizePayload { len: u32, max: u32 },
    /// A reserved header field was non-zero.
    #[error("reserved field non-zero")]
    ReservedFieldNonZero,
    /// An opcode byte didn't match any known opcode.
    #[error("unknown opcode: 0x{0:02X}")]
    UnknownOpcode(u8),
    /// Stored payload CRC32C doesn't match the recomputed value.
    #[error("bad payload crc32c")]
    BadPayloadCrc,
    /// Input ran out before a full frame could be decoded
    /// (need more bytes for the header or for the declared payload).
    #[error("truncated frame: have {have} bytes, need {need}")]
    Truncated { have: usize, need: usize },
}
