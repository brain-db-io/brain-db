//! `PathPlan` — the plan for a `PLAN` request (the cognitive
//! operation, not the planner's output).
//!
//! Named `PathPlan` to avoid `ExecutionPlan::Plan(PlanPlan)` confusion.
//! Spec §08/05 §3+§9 covers the full shape; sub-task 6.5 fleshes out
//! the traversal steps. 6.1 ships a shell carrying the request's
//! parameters so the enum compiles and the executor signature stabilises.

use brain_protocol::request::{PlanBudget, PlanState, PlanStrategy};

#[derive(Debug, Clone)]
pub struct PathPlan {
    pub start: PlanState,
    pub goal: PlanState,
    pub budget: PlanBudget,
    pub strategy: PlanStrategy,
    /// Filled by 6.2's cost model when 6.5 builds the plan.
    pub estimated_cost_ms: f32,
    // Traversal steps land in 6.5.
}
