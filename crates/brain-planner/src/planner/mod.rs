//! Per-cognitive-operation planner functions.
//!
//! Each submodule turns a typed request from `brain-protocol` into an
//! `ExecutionPlan` variant. The plan structs themselves live under
//! `crate::plan`; this module owns only the planning logic.

pub mod encode;
pub mod forget;
pub mod path;
pub mod reason;
pub mod recall;
