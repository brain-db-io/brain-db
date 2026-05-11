//! Planner configuration with spec defaults baked in.
//!
//! Values pinned by the spec:
//! - `default_ef_search = 64`             (spec §08/03 §4)
//! - `max_ef_search = 500`                (spec §08/03 §4)
//! - `max_candidates_per_search = 1000`   (spec §08/03 §5)
//! - `cost_budget_ms = 1000.0`            (spec §08/07 §5)
//! - `max_k = 1000`                       (spec §08/03 §1)
//! - `max_edges_per_encode = 64`          (spec §08/04 §12)
//!
//! Operators may override these at startup; the substrate uses
//! `PlannerConfig::default()` if no overrides are provided.

#[derive(Debug, Clone, Copy)]
pub struct PlannerConfig {
    pub default_ef_search: usize,
    pub max_ef_search: usize,
    pub max_candidates_per_search: usize,
    pub cost_budget_ms: f32,
    pub max_k: usize,
    pub max_edges_per_encode: usize,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            default_ef_search: 64,
            max_ef_search: 500,
            max_candidates_per_search: 1000,
            cost_budget_ms: 1000.0,
            max_k: 1000,
            max_edges_per_encode: 64,
        }
    }
}
