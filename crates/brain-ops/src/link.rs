//! LINK / UNLINK handlers (sub-task 7.8).
//!
//! Spec §09/07. Direct passthrough to the writer's `submit_link` /
//! `submit_unlink` — no planning needed (the writer validates
//! endpoints + range-checks weight in v1).

use brain_core::{EdgeKind, MemoryId, RequestId};
use brain_planner::{LinkOp, UnlinkOp, WriterError};
use brain_protocol::request::{EdgeKindWire, LinkRequest, UnlinkRequest};
use brain_protocol::response::{LinkResponse, UnlinkResponse};

use crate::context::OpsContext;
use crate::error::OpError;

pub async fn handle_link(req: LinkRequest, ctx: &OpsContext) -> Result<LinkResponse, OpError> {
    validate_weight(req.weight, EdgeKind::from(req.kind))?;

    let op = LinkOp {
        request_id: RequestId::from(req.request_id),
        source: MemoryId::from(req.source),
        target: MemoryId::from(req.target),
        kind: req.kind.into(),
        weight: req.weight,
    };

    let ack = ctx
        .executor
        .writer
        .submit_link(op)
        .await
        .map_err(map_writer_err_for_link)?;

    Ok(LinkResponse {
        source: ack.source.into(),
        target: ack.target.into(),
        kind: EdgeKindWire::from(ack.kind),
        weight: ack.weight,
        created_at_unix_nanos: ack.created_at_unix_nanos,
        already_existed: ack.already_existed,
    })
}

pub async fn handle_unlink(
    req: UnlinkRequest,
    ctx: &OpsContext,
) -> Result<UnlinkResponse, OpError> {
    let op = UnlinkOp {
        request_id: RequestId::from(req.request_id),
        source: MemoryId::from(req.source),
        target: MemoryId::from(req.target),
        kind: req.kind.into(),
    };

    let ack = ctx
        .executor
        .writer
        .submit_unlink(op)
        .await
        .map_err(map_writer_err_for_unlink)?;

    Ok(UnlinkResponse {
        source: ack.source.into(),
        target: ack.target.into(),
        kind: EdgeKindWire::from(ack.kind),
        removed: ack.removed,
    })
}

fn validate_weight(weight: f32, kind: EdgeKind) -> Result<(), OpError> {
    // Spec §09/07 §2: `[0, 1]` for most kinds; `[-1, 1]` for
    // `Contradicts` (negative makes sense — "strongly contradicts").
    let (lo, hi) = if matches!(kind, EdgeKind::Contradicts) {
        (-1.0_f32, 1.0_f32)
    } else {
        (0.0_f32, 1.0_f32)
    };
    if !(lo..=hi).contains(&weight) || weight.is_nan() {
        return Err(OpError::InvalidRequest(format!(
            "LINK weight {weight} out of range [{lo}, {hi}] for kind {kind:?}"
        )));
    }
    Ok(())
}

/// LINK-specific writer-error mapping. MemoryNotFound on either
/// endpoint surfaces as `OpError::NotFound`; everything else goes
/// through the default `OpError::ExecError` path.
fn map_writer_err_for_link(err: WriterError) -> OpError {
    match err {
        WriterError::Internal(msg) if msg.contains("not found") => OpError::NotFound {
            what: "memory",
            detail: msg,
        },
        other => OpError::ExecError(brain_planner::ExecError::WriterFailed(other)),
    }
}

fn map_writer_err_for_unlink(err: WriterError) -> OpError {
    OpError::ExecError(brain_planner::ExecError::WriterFailed(err))
}
