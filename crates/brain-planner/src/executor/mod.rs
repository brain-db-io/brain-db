//! Executor side of the planner. Async functions that consume a
//! plan + an [`ExecutorContext`] and produce a Rust-side result.
//!
//! Spec §08/08 §1: "The executor is async (returns futures). Each
//! `execute_*` method orchestrates the steps in the plan."

pub mod context;
pub mod encode;
pub mod error;
pub mod recall;
pub mod result;
pub mod writer;

pub use context::{ExecutorContext, SharedMetadataDb};
pub use encode::execute_encode;
pub use error::ExecError;
pub use recall::execute_recall;
pub use result::{EncodeResult, RecallHit, RecallResult};
pub use writer::{EdgeOutcome, EncodeAck, EncodeOp, EncodeOpEdge, WriterError, WriterHandle};
