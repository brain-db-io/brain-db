//! Executor side of the planner.
//!
//! After the unified write path migration, the only executor surface
//! is the read-only / non-write paths (recall, plan, reason, path)
//! plus the WriterHandle trait + WriterError + EdgeOutcome /
//! ForgetOutcome that wire handlers still use for outcome
//! classification.
//!
//! The legacy execute_encode / execute_forget / dispatch::execute
//! functions and the matching submit_* WriterHandle trait methods
//! were deleted with the unified write path migration; all writes
//! now go through `brain_ops::RealWriterHandle::submit(Write)`.

pub mod context;
pub mod error;
pub mod path;
pub mod reason;
pub mod recall;
pub mod result;
pub mod writer;

pub use context::{ExecutorContext, PendingMemorySnapshot, SharedMetadataDb, TxnSnapshot};
pub use error::ExecError;
pub use path::execute_path;
pub use reason::execute_reason;
pub use recall::execute_recall;
pub use result::{
    EncodeResult, EvidenceItem, ForgetResult, Path, PathResult, PlanStatus, ReasonResult,
    ReasonStatus, RecallHit, RecallResult,
};
pub use writer::{
    EdgeOutcome, EncodeOp, EncodeOpEdge, ForgetOp, ForgetOutcome, LinkOp, UnlinkOp, WriterError,
    WriterHandle,
};
