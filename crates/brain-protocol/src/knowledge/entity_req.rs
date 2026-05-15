//! Entity-op request payloads. Spec §28/00 entity table.

use rkyv::{Archive, Deserialize, Serialize};

use crate::request::WireUuid;

/// `ENTITY_CREATE` (0x0130). Spec §28/00.
///
/// `entity_type_id` is the u32 raw form of the registry id (Person =
/// 1 in phase 16; user-declared types arrive with phase 19 schema DSL).
/// `attributes_blob` is opaque — phase 19 typed accessors. `aliases`
/// are stored verbatim, normalized inside the handler.
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityCreateRequest {
    pub entity_type_id: u32,
    pub canonical_name: String,
    pub aliases: Vec<String>,
    pub attributes_blob: Vec<u8>,
    pub request_id: WireUuid,
}

/// `ENTITY_GET` (0x0131).
#[derive(Archive, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityGetRequest {
    pub entity_id: WireUuid,
}

/// `ENTITY_UPDATE` (0x0132).
///
/// Carries the **full desired state** for the mutable fields. The
/// handler reads the current row, applies the delta, and writes back
/// inside one redb transaction (per `brain-metadata::entity_ops::entity_update`).
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityUpdateRequest {
    pub entity_id: WireUuid,
    /// New canonical_name (the handler treats unchanged-vs-rename
    /// using the same comparison as `entity_update`).
    pub canonical_name: String,
    pub aliases: Vec<String>,
    pub attributes_blob: Vec<u8>,
    pub request_id: WireUuid,
}

/// `ENTITY_RENAME` (0x0133).
///
/// When `move_to_alias = true` (default semantics per spec §18/02), the
/// old canonical_name is appended to the entity's aliases atomically
/// with the swap. The handler currently always moves to alias
/// (matching `brain-metadata::entity_ops::entity_rename`); the flag is
/// here for forward compat with a "no-trail" mode in later phases.
#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct EntityRenameRequest {
    pub entity_id: WireUuid,
    pub new_canonical_name: String,
    pub move_to_alias: bool,
    pub request_id: WireUuid,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::RequestBody;
    use crate::response::ResponseBody;
    use crate::knowledge::{
        EntityCreateResponse, EntityGetResponse, EntityRenameResponse, EntityUpdateResponse,
        EntityView,
    };
    use crate::opcode::Opcode;

    fn sample_uuid(seed: u8) -> WireUuid {
        let mut u = [0u8; 16];
        for (i, b) in u.iter_mut().enumerate() {
            *b = seed.wrapping_add(i as u8);
        }
        u
    }

    fn sample_view() -> EntityView {
        EntityView {
            entity_id: sample_uuid(7),
            entity_type_id: 1,
            canonical_name: "Alice".into(),
            normalized_name: "alice".into(),
            aliases: vec!["A.".into()],
            attributes_blob: b"x".to_vec(),
            mention_count: 3,
            created_at_unix_nanos: 1_700_000_000_000_000_000,
            updated_at_unix_nanos: 1_700_000_001_000_000_000,
            merged_into: [0; 16],
            embedding_version: 1,
            flags: 0,
        }
    }

    fn req_round_trip(body: RequestBody) {
        let bytes = body.encode();
        let decoded = RequestBody::decode(body.opcode(), &bytes).expect("decode req");
        assert_eq!(decoded, body);
    }

    fn resp_round_trip(body: ResponseBody) {
        let bytes = body.encode();
        let decoded = ResponseBody::decode(body.opcode(), &bytes).expect("decode resp");
        assert_eq!(decoded, body);
    }

    #[test]
    fn entity_create_request_roundtrip() {
        req_round_trip(RequestBody::EntityCreate(EntityCreateRequest {
            entity_type_id: 1,
            canonical_name: "Alice".into(),
            aliases: vec!["A.".into(), "Alyss".into()],
            attributes_blob: b"bio=astronaut".to_vec(),
            request_id: sample_uuid(1),
        }));
    }

    #[test]
    fn entity_get_request_roundtrip() {
        req_round_trip(RequestBody::EntityGet(EntityGetRequest {
            entity_id: sample_uuid(2),
        }));
    }

    #[test]
    fn entity_update_request_roundtrip() {
        req_round_trip(RequestBody::EntityUpdate(EntityUpdateRequest {
            entity_id: sample_uuid(3),
            canonical_name: "Alice Cooper".into(),
            aliases: vec![],
            attributes_blob: Vec::new(),
            request_id: sample_uuid(4),
        }));
    }

    #[test]
    fn entity_rename_request_roundtrip_both_flags() {
        for flag in [true, false] {
            req_round_trip(RequestBody::EntityRename(EntityRenameRequest {
                entity_id: sample_uuid(5),
                new_canonical_name: "Bob".into(),
                move_to_alias: flag,
                request_id: sample_uuid(6),
            }));
        }
    }

    #[test]
    fn entity_responses_roundtrip() {
        resp_round_trip(ResponseBody::EntityCreate(EntityCreateResponse {
            entity_id: sample_uuid(10),
        }));
        resp_round_trip(ResponseBody::EntityGet(EntityGetResponse {
            entity: sample_view(),
        }));
        resp_round_trip(ResponseBody::EntityUpdate(EntityUpdateResponse {
            entity: sample_view(),
        }));
        resp_round_trip(ResponseBody::EntityRename(EntityRenameResponse {
            entity: sample_view(),
        }));
    }

    #[test]
    fn knowledge_opcode_byte_assignments() {
        assert_eq!(Opcode::EntityCreateReq.as_u16(), 0x0130);
        assert_eq!(Opcode::EntityCreateResp.as_u16(), 0x01B0);
        assert_eq!(Opcode::EntityGetReq.as_u16(), 0x0131);
        assert_eq!(Opcode::EntityGetResp.as_u16(), 0x01B1);
        assert_eq!(Opcode::EntityUpdateReq.as_u16(), 0x0132);
        assert_eq!(Opcode::EntityUpdateResp.as_u16(), 0x01B2);
        assert_eq!(Opcode::EntityRenameReq.as_u16(), 0x0133);
        assert_eq!(Opcode::EntityRenameResp.as_u16(), 0x01B3);
        assert_eq!(Opcode::EntityCreateReq.namespace(), 0x01);
        assert!(Opcode::EntityCreateReq.is_knowledge());
        assert!(Opcode::EntityCreateReq.is_request());
        assert!(Opcode::EntityCreateResp.is_response());
    }
}

