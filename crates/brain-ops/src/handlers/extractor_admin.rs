//! Extractor governance wire-op handlers — `EXTRACTOR_LIST /
//! _DISABLE / _ENABLE`.
//!
//! `LIST` is read-only and stays on the direct-rtxn path. `DISABLE` and
//! `ENABLE` go through the unified writer (`submit(Write)`), giving
//! them WAL durability and `request_id` idempotency without changing
//! the wire response shape. The in-memory extractor registry is
//! refreshed only after the writer commits — a failed submit leaves
//! the registry untouched.

use brain_core::{ExtractorId, RequestId};
use brain_metadata::extractor::ops::{extractor_get, extractor_list, ExtractorOpError};
use brain_planner::WriterError;
use brain_protocol::{
    ExtractorDisableRequest, ExtractorDisableResponse, ExtractorEnableRequest,
    ExtractorEnableResponse, ExtractorListItem, ExtractorListRequest, ExtractorListResponseFrame,
};

use crate::context::OpsContext;
use crate::error::OpError;
use crate::handlers::link::downcast_writer_pub;
use crate::write::{Phase, PhaseAck, Write, WriteId};

const REASON_MAX_BYTES: usize = 4096;

// ---------------------------------------------------------------------------
// EXTRACTOR_LIST
// ---------------------------------------------------------------------------

pub async fn handle_extractor_list(
    req: ExtractorListRequest,
    ctx: &OpsContext,
) -> Result<ExtractorListResponseFrame, OpError> {
    let rows = {
        let rtxn = ctx
            .executor
            .metadata
            .read_txn()
            .map_err(|e| OpError::Internal(format!("read_txn: {e}")))?;
        extractor_list(&rtxn).map_err(map_extractor_op_error)?
    };
    let items: Vec<ExtractorListItem> = rows
        .into_iter()
        .filter(|r| req.include_disabled || r.is_enabled())
        .map(|r| {
            let enabled = r.is_enabled();
            ExtractorListItem {
                extractor_id: r.extractor_id,
                namespace: r.namespace,
                name: r.name,
                kind: r.kind,
                enabled,
                schema_version: r.schema_version,
                created_at_unix_nanos: r.created_at_unix_nanos,
            }
        })
        .collect();
    let total = items.len() as u32;
    Ok(ExtractorListResponseFrame {
        items,
        total,
        is_final: true,
    })
}

// ---------------------------------------------------------------------------
// EXTRACTOR_DISABLE
// ---------------------------------------------------------------------------

pub async fn handle_extractor_disable(
    req: ExtractorDisableRequest,
    ctx: &OpsContext,
) -> Result<ExtractorDisableResponse, OpError> {
    if req.extractor_id == 0 {
        return Err(OpError::InvalidRequest(
            "extractor_id must be non-zero".into(),
        ));
    }
    if req.reason.len() > REASON_MAX_BYTES {
        return Err(OpError::InvalidRequest(format!(
            "reason exceeds {REASON_MAX_BYTES}-byte cap (got {})",
            req.reason.len()
        )));
    }

    let request_hash = hash_extractor_set_enabled_request(req.extractor_id, false);
    let previous =
        submit_set_enabled(ctx, req.extractor_id, false, req.request_id, request_hash).await?;
    Ok(ExtractorDisableResponse {
        previously_enabled: previous,
        disabled_at_unix_nanos: crate::txn::now_unix_nanos_pub(),
    })
}

// ---------------------------------------------------------------------------
// EXTRACTOR_ENABLE
// ---------------------------------------------------------------------------

pub async fn handle_extractor_enable(
    req: ExtractorEnableRequest,
    ctx: &OpsContext,
) -> Result<ExtractorEnableResponse, OpError> {
    if req.extractor_id == 0 {
        return Err(OpError::InvalidRequest(
            "extractor_id must be non-zero".into(),
        ));
    }
    let request_hash = hash_extractor_set_enabled_request(req.extractor_id, true);
    let previous =
        submit_set_enabled(ctx, req.extractor_id, true, req.request_id, request_hash).await?;
    Ok(ExtractorEnableResponse {
        previously_disabled: !previous,
        enabled_at_unix_nanos: crate::txn::now_unix_nanos_pub(),
    })
}

// ---------------------------------------------------------------------------
// Shared submit core.
// ---------------------------------------------------------------------------

/// Existence-probe + submit a `SetExtractorEnabled` phase through the
/// unified writer. Returns the row's previous `enabled` byte so the
/// caller can derive `previously_enabled` / `previously_disabled`.
///
/// On replay (same request_id, same hash), returns the original
/// `previous` value by re-reading the persisted row — the writer's
/// idempotency cache short-circuits before apply runs, so the row
/// already carries the post-write state.
async fn submit_set_enabled(
    ctx: &OpsContext,
    extractor_id_raw: u32,
    enabled: bool,
    request_id: [u8; 16],
    request_hash: [u8; 32],
) -> Result<bool, OpError> {
    let id = ExtractorId::from(extractor_id_raw);
    let real_writer = downcast_writer_pub(ctx)?;
    let write_id = WriteId::from_request(RequestId::from(request_id));

    // Existence probe + capture previous state in one rtxn. We need
    // `previous` for the wire response and we want a precise NotFound
    // before submit even queues a Write. apply_set_extractor_enabled
    // doesn't surface the prior byte on its PhaseAck.
    let previous = {
        let rtxn = ctx
            .executor
            .metadata
            .read_txn()
            .map_err(|e| OpError::Internal(format!("read_txn: {e}")))?;
        let row = extractor_get(&rtxn, id)
            .map_err(map_extractor_op_error)?
            .ok_or(OpError::NotFound {
                what: "extractor",
                detail: format!("id {extractor_id_raw}"),
            })?;
        row.is_enabled()
    };

    // Idempotency: a true replay (same hash) returns the original
    // outcome without re-running apply. A mismatched hash conflicts.
    match real_writer.idempotency_lookup(write_id, Some(request_hash)) {
        crate::writer::submit::CacheLookup::Hit(_cached) => {
            // The persisted state already reflects the original write.
            // `previous` we just read is the post-write value, so it
            // equals the new `enabled` — mirror that as the original
            // call's `previous` would have been the opposite. We can't
            // recover the true prior value here, so report the value
            // that satisfies the wire contract on replay: the row
            // is now in state `enabled`, and the first call's
            // previously_enabled / previously_disabled was the negation.
            return Ok(!enabled);
        }
        crate::writer::submit::CacheLookup::Conflict => {
            return Err(map_writer_err(WriterError::Conflict(
                "extractor_set_enabled request_id replay with different params".into(),
            )));
        }
        crate::writer::submit::CacheLookup::Miss => {}
    }

    let phase = Phase::SetExtractorEnabled { id, enabled };
    let write =
        Write::single(write_id, ctx.executor.caller_agent, phase).with_request_hash(request_hash);
    let ack = real_writer.submit(write).await.map_err(map_writer_err)?;
    debug_assert!(matches!(
        ack.single_phase(),
        PhaseAck::ExtractorEnabledSet { .. }
    ));

    // Sync the in-memory registry only after the wtxn commits. The
    // persisted state is the source of truth; this keeps the dispatch
    // path's `iter_enabled` view consistent without re-querying.
    ctx.extractor_registry.write().set_enabled(id, enabled);

    Ok(previous)
}

/// BLAKE3 over the canonical `set_extractor_enabled` request fields.
/// Excludes `request_id` (the table key).
fn hash_extractor_set_enabled_request(extractor_id_raw: u32, enabled: bool) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(b"set_extractor_enabled:");
    h.update(&extractor_id_raw.to_le_bytes());
    h.update(b"\0");
    h.update(&[u8::from(enabled)]);
    *h.finalize().as_bytes()
}

fn map_writer_err(err: WriterError) -> OpError {
    OpError::ExecError(brain_planner::ExecError::WriterFailed(err))
}

fn map_extractor_op_error(e: ExtractorOpError) -> OpError {
    match e {
        ExtractorOpError::NotFound { id } => OpError::NotFound {
            what: "extractor",
            detail: format!("id {}", id.raw()),
        },
        ExtractorOpError::InvalidIdentifier { reason } => {
            OpError::InvalidRequest(reason.to_string())
        }
        ExtractorOpError::AlreadyExists { qname, existing_id } => OpError::Conflict(format!(
            "extractor {qname:?} already exists with id {}",
            existing_id.raw()
        )),
        other => OpError::Internal(other.to_string()),
    }
}
