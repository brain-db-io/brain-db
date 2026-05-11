//! RECALL handler (sub-task 7.4).
//!
//! Wires the planner (6.5) + executor (6.5) through the dispatcher
//! and maps `brain_planner::RecallResult` into the wire
//! `RecallResponseFrame`. Single-frame for v1 (streaming chunker
//! lands in Phase 9).

use brain_planner::{execute_recall, plan_recall_inner, RecallHit};
use brain_protocol::request::RecallRequest;
use brain_protocol::response::{MemoryResult, RecallResponseFrame};

use crate::context::OpsContext;
use crate::error::OpError;

pub async fn handle_recall(
    req: RecallRequest,
    ctx: &OpsContext,
) -> Result<RecallResponseFrame, OpError> {
    let plan = plan_recall_inner(&req, &ctx.planner_ctx)?;
    let result = execute_recall(plan, &ctx.executor).await?;

    let results: Vec<MemoryResult> = result.hits.into_iter().map(hit_to_wire).collect();
    let cumulative_count = u32::try_from(results.len()).unwrap_or(u32::MAX);

    Ok(RecallResponseFrame {
        results,
        is_final: true,
        cumulative_count,
        estimated_remaining: None,
    })
}

/// Map an executor `RecallHit` to the wire `MemoryResult`.
///
/// v1 gaps (see `.claude/plans/phase-07-task-04.md` §2.2):
/// - `text` empty until the executor grows a `TextFetchStep`.
/// - `last_accessed_at_unix_nanos` mirrors `created_at_unix_nanos`
///   until a Phase 11 worker tracks access.
/// - `vector_offset` / `vector_dim` are `0` until Phase 9 exposes
///   the arena window.
/// - `edges` is `None` until edge fetch is wired.
fn hit_to_wire(hit: RecallHit) -> MemoryResult {
    MemoryResult {
        memory_id: hit.memory_id.into(),
        text: hit.text.unwrap_or_default(),
        similarity_score: hit.score,
        // Spec §09/03 §4: confidence == similarity for v1.
        confidence: hit.score,
        salience: hit.salience,
        kind: hit.kind.into(),
        context_id: hit.context_id.into(),
        created_at_unix_nanos: hit.created_at_unix_nanos,
        last_accessed_at_unix_nanos: hit.created_at_unix_nanos,
        vector_offset: 0,
        vector_dim: 0,
        edges: None,
    }
}
