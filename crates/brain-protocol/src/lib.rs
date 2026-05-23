//! # brain-protocol
//!
//! Brain's wire protocol: a custom binary protocol over TCP (with optional
//! TLS). Frames have a fixed 32-byte header, a magic of `b"BRN0"`, header
//! and payload CRC32C, and a 24-bit payload length cap (16 MiB).

#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]
#![forbid(unsafe_code)]

pub mod convert;
pub mod crc;
pub mod error;
pub mod frame;
pub mod handshake;
pub mod header;
pub mod opcode;
pub mod request;
pub mod requests;
pub mod response;
pub mod responses;
mod rkyv_codec;
pub mod schema;

pub use error::{ErrorCategory, ErrorCode, ProtocolError};
pub use frame::Frame;
pub use header::{Header, VERSION};
pub use opcode::Opcode;
pub use request::RequestBody;
pub use response::ResponseBody;

// Flat re-exports of the typed-graph wire payloads — formerly accessible
// via `brain_protocol::*`. Callers can pull any noun's
// request/response/event type directly from the crate root.
pub use requests::{
    EntityCreateRequest, EntityGetRequest, EntityListRequest, EntityMergeRequest,
    EntityRenameRequest, EntityResolveRequest, EntityTombstoneRequest, EntityUnmergeRequest,
    EntityUpdateRequest, ExtractorDisableRequest, ExtractorEnableRequest, ExtractorListRequest,
    FusionConfigWire, ItemIdWire, MaterializeProceduralRequest, QueryExplainRequest, QueryRequest,
    QueryTraceRequest, RecallHybridRequest, RelationCreateRequest, RelationGetRequest,
    RelationListFromRequest, RelationListToRequest, RelationSupersedeRequest,
    RelationTombstoneRequest, RelationTraverseRequest, RetrieverContributionWire,
    RetrieverOutcomeWire, RetrieverSelectionWire, RetrieverWire, SchemaGetRequest,
    SchemaListRequest, SchemaUploadRequest, SchemaValidateRequest, StatementCreateRequest,
    StatementGetRequest, StatementHistoryRequest, StatementListRequest, StatementRetractRequest,
    StatementSupersedeRequest, StatementTombstoneRequest, TimeRangeWire,
    // Wire-side primitives shared with statement.rs (EvidenceRefWire and the value/object/kind
    // helpers).
    EvidenceRefWire, StatementKindWire, StatementObjectWire, StatementValueWire,
};
pub use responses::{
    // Knowledge-event payloads.
    EntityCreatedEvent, EntityMergedEvent, EntityRenamedEvent, EntityTombstonedEvent,
    EntityUnmergedEvent, EntityUpdatedEvent, KnowledgeEventPayload, RelationCreatedEvent,
    RelationSupersededEvent, RelationTombstonedEvent, SchemaUpdatedEvent, StatementCreatedEvent,
    StatementSupersededEvent, StatementTombstonedEvent,
    // Per-noun response payloads.
    EntityCreateResponse, EntityGetResponse, EntityListItem, EntityListResponseFrame,
    EntityMergeResponse, EntityRenameResponse, EntityResolveResponse, EntityTombstoneResponse,
    EntityUnmergeResponse, EntityUpdateResponse, EntityView, ExtractorDisableResponse,
    ExtractorEnableResponse, ExtractorListItem, ExtractorListResponseFrame, MaterializeProceduralResponse,
    MemoryHit, QueryExplainResponse, QueryResponse, QueryResultItem, QueryTraceResponse,
    RecallHybridResponse, RelationCreateResponse, RelationGetResponse,
    RelationListFromResponseFrame, RelationListToResponseFrame, RelationSupersedeResponse,
    RelationTombstoneResponse, RelationTraverseResponseFrame, RelationView, RelationWireError,
    ResolutionOutcomeWire, SchemaGetResponse, SchemaListItemWire, SchemaListResponseFrame,
    SchemaUploadResponse, SchemaValidateResponse, SchemaValidationErrorWire, StatementCreateResponse,
    StatementGetResponse, StatementHistoryResponseFrame, StatementListResponseFrame,
    StatementRetractResponse, StatementSupersedeResponse, StatementTombstoneResponse,
    StatementView, TraversalPathWire, TraversalStepWire, WireToStatementError,
    // Free-function helpers in responses/statement.rs.
    evidence_ref_from_wire, evidence_ref_to_wire, relation_type_canonical, statement_kind_from_wire,
    statement_kind_to_wire, statement_object_from_wire, statement_object_to_wire,
    statement_value_from_wire, statement_value_to_wire,
};

/// Frame magic bytes. Identifies a Brain frame on the wire.
pub const MAGIC: [u8; 4] = *b"BRN0";

/// Fixed frame header size in bytes.
pub const HEADER_SIZE: usize = 32;

/// Maximum payload size (16 MiB - 1), enforced by the 24-bit length field.
pub const MAX_PAYLOAD_BYTES: usize = (1 << 24) - 1;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_is_brn0() {
        assert_eq!(&MAGIC, b"BRN0");
    }

    #[test]
    fn header_size_is_32() {
        assert_eq!(HEADER_SIZE, 32);
    }

    #[test]
    fn max_payload_fits_in_24_bits() {
        assert_eq!(MAX_PAYLOAD_BYTES, 16 * 1024 * 1024 - 1);
    }
}
