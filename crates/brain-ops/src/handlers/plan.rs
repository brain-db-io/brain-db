//! PLAN handler.
//!
//! Wires the planner + the bi-BFS path executor through the
//! dispatcher and projects a `PathStream` (the executor's per-path
//! frames plus a terminal summary) into a sequence of wire
//! `PlanResponseFrame`s.
//!
//! One mid-stream frame per scored path (top-N after sort + truncate)
//! followed by a single terminal frame carrying the `plan_status`. The
//! terminal frame has `is_final = true`; mid-stream frames have
//! `is_final = false` and `plan_status = None`. An empty path stream
//! (no path found, base set empty, etc.) is surfaced as a single
//! terminal frame — clients still see exactly one final frame
//! regardless of how much the executor produced.

use brain_core::EdgeKind;
use brain_planner::{execute_path_stream, plan_path_inner, Path, PathFrame, PlanStatus};
use brain_protocol::envelope::request::PlanRequest;
use brain_protocol::envelope::response::{
    PlanResponseFrame, PlanStatus as WirePlanStatus, PlanStep, TransitionKind,
};

use crate::context::OpsContext;
use crate::error::OpError;
use crate::state::txn_lens::build_executor_with_lens;

pub async fn handle_plan(
    req: PlanRequest,
    ctx: &OpsContext,
) -> Result<Vec<PlanResponseFrame>, OpError> {
    let plan = plan_path_inner(&req, &ctx.planner_ctx)?;
    let exec_ctx = build_executor_with_lens(ctx, req.txn_id)?;
    let stream = execute_path_stream(plan, &exec_ctx).await?;

    let mut frames: Vec<PlanResponseFrame> = Vec::with_capacity(stream.paths.len() + 1);
    for path_frame in stream.paths {
        frames.push(path_frame_to_wire(path_frame));
    }
    // Terminal frame — always emitted, regardless of how many paths
    // the stream produced. Carries the aggregate status and marks
    // end-of-stream.
    frames.push(PlanResponseFrame {
        steps: Vec::new(),
        is_final: true,
        plan_status: Some(to_wire_status(stream.terminal.status)),
    });
    Ok(frames)
}

fn path_frame_to_wire(frame: PathFrame) -> PlanResponseFrame {
    PlanResponseFrame {
        steps: path_to_steps(&frame.path),
        is_final: false,
        plan_status: None,
    }
}

fn to_wire_status(s: PlanStatus) -> WirePlanStatus {
    match s {
        PlanStatus::GoalReached => WirePlanStatus::GoalReached,
        PlanStatus::BudgetExhausted => WirePlanStatus::BudgetExhausted,
        PlanStatus::NoPathFound => WirePlanStatus::NoPathFound,
        // The wire enum has no `Timeout` variant — surface a wall-time
        // stop as BudgetExhausted. A future wire revision can add
        // the variant.
        PlanStatus::Timeout => WirePlanStatus::BudgetExhausted,
    }
}

fn path_to_steps(path: &Path) -> Vec<PlanStep> {
    let n = path.nodes.len();
    path.nodes
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let transition_kind = if i == 0 {
                TransitionKind::Initial
            } else {
                edge_to_transition(path.edges[i - 1])
            };
            #[allow(clippy::cast_precision_loss)]
            let estimated_distance_to_goal = (n - 1 - i) as f32;
            PlanStep {
                step_index: u32::try_from(i).unwrap_or(u32::MAX),
                memory_id: (*id).into(),
                text: path.node_text.get(i).cloned().unwrap_or_default(),
                transition_kind,
                confidence: path.score,
                estimated_distance_to_goal,
            }
        })
        .collect()
}

fn edge_to_transition(kind: EdgeKind) -> TransitionKind {
    match kind {
        EdgeKind::Caused => TransitionKind::Causal,
        EdgeKind::FollowedBy => TransitionKind::Temporal,
        EdgeKind::SimilarTo => TransitionKind::Similarity,
        other => TransitionKind::Other(format!("{other:?}")),
    }
}
