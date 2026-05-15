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
