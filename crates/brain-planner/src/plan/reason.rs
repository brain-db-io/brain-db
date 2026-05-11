//! `ReasonPlan` shell. Spec §08/05 §8–§10. 6.5 fleshes out the BFS
//! traversal + path-scoring details.

use brain_protocol::request::ObservationInput;

#[derive(Debug, Clone)]
pub struct ReasonPlan {
    pub observation: ObservationInput,
    pub depth: u32,
    pub confidence_threshold: f32,
    pub max_inferences: u32,
    pub budget_wall_time_ms: u32,
    pub estimated_cost_ms: f32,
}
