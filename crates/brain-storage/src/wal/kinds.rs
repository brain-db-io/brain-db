//! WAL record kind discriminator.
//!
//! This module defines the `record_type` byte from `spec/05_storage_arena_wal/
//! 05_wal_records.md` §3. In sub-task 2.1 the enum is unit-only — it just
//! identifies what *kind* of record was read; the actual payload schemas
//! (per-variant rkyv-serialized structs) are added in sub-task 2.2.

/// One variant per spec'd `record_type` byte.
///
/// The discriminant matches the spec table exactly so casts to/from `u8`
/// are the on-disk encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum WalRecordKind {
    Encode = 1,
    Forget = 2,
    Link = 3,
    Unlink = 4,
    UpdateSalience = 5,
    Reclaim = 6,
    Consolidate = 7,
    UpdateKind = 8,
    UpdateContext = 9,
    CheckpointBegin = 10,
    CheckpointEnd = 11,
    TxnBegin = 12,
    TxnCommit = 13,
    TxnAbort = 14,
    MigrateEmbedding = 15,
}

impl WalRecordKind {
    /// Inverse of the `#[repr(u8)]` cast. Returns `None` for `0` (reserved
    /// per spec) and any value not in the spec's v1 table (16+).
    pub const fn from_u8(b: u8) -> Option<Self> {
        Some(match b {
            1 => Self::Encode,
            2 => Self::Forget,
            3 => Self::Link,
            4 => Self::Unlink,
            5 => Self::UpdateSalience,
            6 => Self::Reclaim,
            7 => Self::Consolidate,
            8 => Self::UpdateKind,
            9 => Self::UpdateContext,
            10 => Self::CheckpointBegin,
            11 => Self::CheckpointEnd,
            12 => Self::TxnBegin,
            13 => Self::TxnCommit,
            14 => Self::TxnAbort,
            15 => Self::MigrateEmbedding,
            _ => return None,
        })
    }

    pub const fn as_u8(self) -> u8 {
        self as u8
    }
}

/// Every spec'd v1 kind, in declaration order. Useful for exhaustive tests.
pub const ALL_KINDS: &[WalRecordKind] = &[
    WalRecordKind::Encode,
    WalRecordKind::Forget,
    WalRecordKind::Link,
    WalRecordKind::Unlink,
    WalRecordKind::UpdateSalience,
    WalRecordKind::Reclaim,
    WalRecordKind::Consolidate,
    WalRecordKind::UpdateKind,
    WalRecordKind::UpdateContext,
    WalRecordKind::CheckpointBegin,
    WalRecordKind::CheckpointEnd,
    WalRecordKind::TxnBegin,
    WalRecordKind::TxnCommit,
    WalRecordKind::TxnAbort,
    WalRecordKind::MigrateEmbedding,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discriminants_match_spec_table() {
        // Spot-check against spec/05_storage_arena_wal/05_wal_records.md §3.
        assert_eq!(WalRecordKind::Encode.as_u8(), 1);
        assert_eq!(WalRecordKind::Forget.as_u8(), 2);
        assert_eq!(WalRecordKind::Reclaim.as_u8(), 6);
        assert_eq!(WalRecordKind::CheckpointEnd.as_u8(), 11);
        assert_eq!(WalRecordKind::MigrateEmbedding.as_u8(), 15);
    }

    #[test]
    fn from_u8_round_trips_every_kind() {
        for &k in ALL_KINDS {
            assert_eq!(WalRecordKind::from_u8(k.as_u8()), Some(k));
        }
    }

    #[test]
    fn from_u8_rejects_reserved_and_unknown() {
        assert_eq!(WalRecordKind::from_u8(0), None); // reserved per spec
        assert_eq!(WalRecordKind::from_u8(16), None); // reserved for v1 minor
        assert_eq!(WalRecordKind::from_u8(128), None); // reserved for v2+
        assert_eq!(WalRecordKind::from_u8(255), None);
    }

    #[test]
    fn all_kinds_covers_every_variant() {
        // If a new variant is added without updating ALL_KINDS, this catches
        // it: round-trip through u8 must hit every value 1..=15.
        let seen: std::collections::HashSet<u8> = ALL_KINDS.iter().map(|k| k.as_u8()).collect();
        assert_eq!(seen.len(), 15);
        for v in 1..=15u8 {
            assert!(seen.contains(&v), "kind {v} missing from ALL_KINDS");
        }
    }
}
