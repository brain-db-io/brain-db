//! ENCODE handler (sub-task 7.3).
//!
//! Wires the planner (6.4) + executor (6.4 + 7.2's `RealWriterHandle`)
//! through the dispatcher and maps the result into the wire
//! `EncodeResponse`.

use brain_planner::{execute_encode, plan_encode_inner, EdgeOutcome};
use brain_protocol::request::EncodeRequest;
use brain_protocol::response::EncodeResponse;

use crate::context::OpsContext;
use crate::error::OpError;

pub async fn handle_encode(
    req: EncodeRequest,
    ctx: &OpsContext,
) -> Result<EncodeResponse, OpError> {
    // 1. Plan.
    let plan = plan_encode_inner(&req, &ctx.planner_ctx)?;

    // 2. Capture salience for the response. The planner pins it on
    //    the WalAppendStep; execute_encode consumes the plan, so we
    //    read it off here.
    let salience = plan.wal_append.salience_initial;

    // 3. Execute.
    let result = execute_encode(plan, &ctx.executor).await?;

    // 4. Map to wire. Spec §09/02 §3:
    //    - was_deduplicated ← replay flag (only dedupe path in v1)
    //    - auto_edges_added ← count of Inserted edge outcomes
    let auto_edges_added = result
        .edge_results
        .iter()
        .filter(|o| matches!(o, EdgeOutcome::Inserted))
        .count() as u32;

    Ok(EncodeResponse {
        memory_id: result.memory_id.into(),
        was_deduplicated: result.replayed,
        salience,
        auto_edges_added,
    })
}
