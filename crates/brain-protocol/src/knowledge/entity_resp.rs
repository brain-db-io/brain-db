//! Entity-op response payloads. Spec §28/00 entity table.

use rkyv::{Archive, Deserialize, Serialize};

use crate::request::WireUuid;

/// Read-side view of an entity. Mirrors `brain_core::Entity` but uses
/// wire-domain primitives (`[u8; 16]` for the entity id, `u32` for the
/// type id) so the rkyv derive fires without coupling `brain-core`
/// value types to rkyv.
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityView {
    pub entity_id: WireUuid,
    pub entity_type_id: u32,
    pub canonical_name: String,
    pub normalized_name: String,
    pub aliases: Vec<String>,
    pub attributes_blob: Vec<u8>,
    pub mention_count: u32,
    pub created_at_unix_nanos: u64,
    pub updated_at_unix_nanos: u64,
    /// `[0; 16]` when not merged (rkyv archive derives don't play well
    /// with `Option<[u8; 16]>` here; consumers treat all-zero as None).
    pub merged_into: WireUuid,
    pub embedding_version: u32,
    pub flags: u32,
}

/// Reply to `ENTITY_CREATE`.
#[derive(Archive, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityCreateResponse {
    pub entity_id: WireUuid,
}

/// Reply to `ENTITY_GET`.
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityGetResponse {
    pub entity: EntityView,
}

/// Reply to `ENTITY_UPDATE`. Carries the post-update view for the
/// client's convenience (avoids a follow-up `ENTITY_GET`).
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityUpdateResponse {
    pub entity: EntityView,
}

/// Reply to `ENTITY_RENAME`. Carries the post-rename view.
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityRenameResponse {
    pub entity: EntityView,
}

/// Reply to `ENTITY_MERGE`. Spec §28/01 §7.2.
#[derive(Archive, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityMergeResponse {
    /// MergeId (the audit row id), not an EntityId.
    pub audit_id: WireUuid,
    pub grace_period_seconds: u64,
}

/// Reply to `ENTITY_UNMERGE`. Spec §28/01 §8.2.
#[derive(Archive, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityUnmergeResponse {
    pub restored_entity_id: WireUuid,
}

/// `ResolutionOutcome` wire enum — mirrors `brain_core::knowledge::ResolutionOutcome`
/// but flattened to a u8 for rkyv-archive simplicity.
///
/// Spec §28/01 §9.2.
#[derive(Archive, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
#[repr(u8)]
pub enum ResolutionOutcomeWire {
    Resolved = 1,
    Created = 2,
    Ambiguous = 3,
    NotFound = 4,
}

/// Reply to `ENTITY_RESOLVE`. Spec §28/01 §9.2.
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityResolveResponse {
    pub outcome: ResolutionOutcomeWire,
    /// Which tier resolved (1..=5; 0 if unresolved).
    pub tier: u8,
    pub confidence: f32,
    /// Populated when outcome == Resolved or Created (single id).
    /// `[0; 16]` for Ambiguous / NotFound.
    pub resolved_entity: WireUuid,
    /// Populated when outcome == Ambiguous; ranked by score.
    pub candidate_ids: Vec<WireUuid>,
    /// `[0; 16]` unless an ambiguity audit was written.
    pub audit_id: WireUuid,
}

/// Per-item frame body for the streaming `ENTITY_LIST` response.
/// Spec §28/01 §10.2.
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityListItem {
    pub entity: EntityView,
}

/// Tail frame body for the streaming `ENTITY_LIST` response. EOS-flagged.
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityListResponseTail {
    pub next_cursor: Vec<u8>,
    pub total_returned: u32,
}

/// Combined response body for `ENTITY_LIST`. The streaming dispatcher
/// emits either an `Item` frame or a `Tail` frame; the wire opcode is
/// the same (`0x01B7`) and the body discriminates.
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub enum EntityListResponseFrame {
    Item(EntityListItem),
    Tail(EntityListResponseTail),
}

impl EntityListResponseFrame {
    /// True for the final tail frame; false for per-item intermediate
    /// frames. Mirrors the substrate's `is_final` body-side signal.
    #[must_use]
    pub fn is_final(&self) -> bool {
        matches!(self, Self::Tail(_))
    }
}

/// Reply to `ENTITY_TOMBSTONE`. Spec §28/01 §11.2.
#[derive(Archive, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityTombstoneResponse {
    pub tombstoned_at_unix_nanos: u64,
}
