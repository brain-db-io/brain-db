//! Write-Ahead Log: per-shard, append-only, fsync-coordinated.
//!
//! See `spec/05_storage_arena_wal/04_wal_overview.md` and
//! `05_wal_records.md` for the design. This module currently exposes the
//! record-level framing only; segment writer/reader/recovery land in
//! subsequent sub-tasks (2.6–2.10).

pub mod kinds;
pub mod record;

pub use kinds::{WalRecordKind, ALL_KINDS};
pub use record::{
    DecodeOutcome, Lsn, WalRecord, WalRecordError, FOOTER_LEN, HEADER_LEN, MAX_PAYLOAD,
};
