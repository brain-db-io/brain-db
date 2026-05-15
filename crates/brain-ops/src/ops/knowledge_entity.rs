//! Entity wire-op handlers — `ENTITY_CREATE` / `_GET` / `_UPDATE` /
//! `_RENAME` (spec §28/00, phase 16.6c).
//!
//! Each handler:
//!
//! 1. Validates the request.
//! 2. Acquires the per-shard `MetadataDb` lock.
//! 3. Opens a redb write (or read) transaction.
//! 4. Calls into `brain_metadata::entity_ops::*`.
//! 5. Commits the transaction.
//! 6. Maps `EntityOpError` to `OpError` per spec §28's error codes
//!    (mapped through the substrate ErrorCode taxonomy until §28 error
//!    codes land as first-class — see [`map_entity_op_error`]).
//!
//! Phase 16.6c handlers do **not** touch the entity HNSW (16.3) or
//! emit subscription events. Both wire in later sub-tasks.

use brain_core::{Entity, EntityAttributes, EntityId, EntityTypeId};
use brain_metadata::entity_ops::{
    entity_get, entity_put, entity_rename, entity_update, EntityOpError,
};
use brain_protocol::knowledge::{
    EntityCreateRequest, EntityCreateResponse, EntityGetRequest, EntityGetResponse,
    EntityRenameRequest, EntityRenameResponse, EntityUpdateRequest, EntityUpdateResponse,
    EntityView,
};

use crate::context::OpsContext;
use crate::error::OpError;

// ---------------------------------------------------------------------------
// ENTITY_CREATE
// ---------------------------------------------------------------------------

pub async fn handle_entity_create(
    req: EntityCreateRequest,
    ctx: &OpsContext,
) -> Result<EntityCreateResponse, OpError> {
    if req.canonical_name.trim().is_empty() {
        return Err(OpError::InvalidRequest(
            "canonical_name must be non-empty".into(),
        ));
    }

    let entity_type = EntityTypeId(req.entity_type_id);
    let now = crate::txn::now_unix_nanos_pub();
    let id = EntityId::new();
    let mut entity = Entity::new_active(
        id,
        entity_type,
        req.canonical_name.clone(),
        normalize_name(&req.canonical_name),
        now,
    );
    entity.aliases = req.aliases;
    entity.attributes = EntityAttributes::from(req.attributes_blob);

    {
        let mut db_guard = ctx.executor.metadata.lock();
        let wtxn = db_guard
            .write_txn()
            .map_err(|e| OpError::Internal(format!("write_txn: {e}")))?;
        entity_put(&wtxn, &entity).map_err(map_entity_op_error)?;
        wtxn.commit()
            .map_err(|e| OpError::Internal(format!("commit: {e}")))?;
    }

    Ok(EntityCreateResponse {
        entity_id: id.to_bytes(),
    })
}

// ---------------------------------------------------------------------------
// ENTITY_GET
// ---------------------------------------------------------------------------

pub async fn handle_entity_get(
    req: EntityGetRequest,
    ctx: &OpsContext,
) -> Result<EntityGetResponse, OpError> {
    let id = EntityId::from(req.entity_id);
    let entity = {
        let db_guard = ctx.executor.metadata.lock();
        let rtxn = db_guard
            .read_txn()
            .map_err(|e| OpError::Internal(format!("read_txn: {e}")))?;
        entity_get(&rtxn, id).map_err(map_entity_op_error)?
    };
    let entity = entity.ok_or_else(|| OpError::NotFound { what: "entity", detail: format!("{id:?}") })?;
    Ok(EntityGetResponse {
        entity: entity_to_view(&entity),
    })
}

// ---------------------------------------------------------------------------
// ENTITY_UPDATE
// ---------------------------------------------------------------------------

pub async fn handle_entity_update(
    req: EntityUpdateRequest,
    ctx: &OpsContext,
) -> Result<EntityUpdateResponse, OpError> {
    if req.canonical_name.trim().is_empty() {
        return Err(OpError::InvalidRequest(
            "canonical_name must be non-empty".into(),
        ));
    }
    let id = EntityId::from(req.entity_id);
    let now = crate::txn::now_unix_nanos_pub();
    let after = {
        let mut db_guard = ctx.executor.metadata.lock();
        // Load current entity to preserve immutable / unchanged fields.
        let current = {
            let rtxn = db_guard
                .read_txn()
                .map_err(|e| OpError::Internal(format!("read_txn: {e}")))?;
            entity_get(&rtxn, id)
                .map_err(map_entity_op_error)?
                .ok_or_else(|| OpError::NotFound { what: "entity", detail: format!("{id:?}") })?
        };
        let mut next = current.clone();
        next.canonical_name = req.canonical_name.clone();
        next.normalized_name = normalize_name(&req.canonical_name);
        next.aliases = req.aliases;
        next.attributes = EntityAttributes::from(req.attributes_blob);

        let wtxn = db_guard
            .write_txn()
            .map_err(|e| OpError::Internal(format!("write_txn: {e}")))?;
        entity_update(&wtxn, &next, now).map_err(map_entity_op_error)?;
        wtxn.commit()
            .map_err(|e| OpError::Internal(format!("commit: {e}")))?;

        // Re-read post-commit so we return the persisted view (including
        // entity_update's derived fields like embedding_version).
        let rtxn = db_guard
            .read_txn()
            .map_err(|e| OpError::Internal(format!("read_txn: {e}")))?;
        entity_get(&rtxn, id)
            .map_err(map_entity_op_error)?
            .ok_or_else(|| OpError::Internal(format!("entity {id:?} missing post-update")))?
    };
    Ok(EntityUpdateResponse {
        entity: entity_to_view(&after),
    })
}

// ---------------------------------------------------------------------------
// ENTITY_RENAME
// ---------------------------------------------------------------------------

pub async fn handle_entity_rename(
    req: EntityRenameRequest,
    ctx: &OpsContext,
) -> Result<EntityRenameResponse, OpError> {
    if req.new_canonical_name.trim().is_empty() {
        return Err(OpError::InvalidRequest(
            "new_canonical_name must be non-empty".into(),
        ));
    }
    // `move_to_alias = false` would mean "discard old name, no alias
    // trail". `brain_metadata::entity_ops::entity_rename` always
    // appends to aliases (spec §18/02 default). Reject the no-trail
    // mode until a later phase implements it explicitly.
    if !req.move_to_alias {
        return Err(OpError::InvalidRequest(
            "rename with move_to_alias=false is not yet supported".into(),
        ));
    }
    let id = EntityId::from(req.entity_id);
    let now = crate::txn::now_unix_nanos_pub();
    let after = {
        let mut db_guard = ctx.executor.metadata.lock();
        let wtxn = db_guard
            .write_txn()
            .map_err(|e| OpError::Internal(format!("write_txn: {e}")))?;
        entity_rename(&wtxn, id, req.new_canonical_name.clone(), now)
            .map_err(map_entity_op_error)?;
        wtxn.commit()
            .map_err(|e| OpError::Internal(format!("commit: {e}")))?;

        let rtxn = db_guard
            .read_txn()
            .map_err(|e| OpError::Internal(format!("read_txn: {e}")))?;
        entity_get(&rtxn, id)
            .map_err(map_entity_op_error)?
            .ok_or_else(|| OpError::Internal(format!("entity {id:?} missing post-rename")))?
    };
    Ok(EntityRenameResponse {
        entity: entity_to_view(&after),
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Normalize per `brain-metadata::entity_ops::normalize_name`. Inlined
/// here to avoid threading the metadata crate into the wire types.
fn normalize_name(s: &str) -> String {
    brain_metadata::entity_ops::normalize_name(s)
}

fn entity_to_view(e: &Entity) -> EntityView {
    let merged_into = e.merged_into.map(|id| id.to_bytes()).unwrap_or([0u8; 16]);
    EntityView {
        entity_id: e.id.to_bytes(),
        entity_type_id: e.entity_type.raw(),
        canonical_name: e.canonical_name.clone(),
        normalized_name: e.normalized_name.clone(),
        aliases: e.aliases.clone(),
        attributes_blob: e.attributes.as_bytes().to_vec(),
        mention_count: e.mention_count,
        created_at_unix_nanos: e.created_at_unix_nanos,
        updated_at_unix_nanos: e.updated_at_unix_nanos,
        merged_into,
        embedding_version: e.embedding_version,
        flags: e.flags,
    }
}

/// Map `EntityOpError` to `OpError`. Until §28 error codes get a
/// first-class slot in the substrate's `ErrorCode`, we surface them as
/// the closest substrate categories — NotFound for missing rows,
/// Conflict for duplicates, InvalidArgument for type-registry misses,
/// Internal for redb failures.
fn map_entity_op_error(err: EntityOpError) -> OpError {
    match err {
        EntityOpError::NotFound(id) => OpError::NotFound { what: "entity", detail: format!("{id:?}") },
        EntityOpError::UnknownEntityType(t) => {
            OpError::InvalidRequest(format!("unknown entity_type {t:?}"))
        }
        EntityOpError::DuplicateCanonicalName {
            type_id,
            name,
            existing,
        } => OpError::Conflict(format!(
            "canonical_name {name:?} already exists for type {type_id:?}: {existing:?}"
        )),
        EntityOpError::Storage(e) => OpError::Internal(format!("redb storage: {e}")),
        EntityOpError::Table(e) => OpError::Internal(format!("redb table: {e}")),
        EntityOpError::TrigramOp(e) => OpError::Internal(format!("trigram op: {e}")),
    }
}
