//! `ForgetPlan` shell. Spec §08/06 §2 distinguishes soft vs hard
//! forget; 6.6 fills in the bulk-forget cap, cascade options, and
//! per-memory error tolerance.

use brain_core::MemoryId;
use brain_protocol::request::ForgetMode;

use super::common::ShardId;

#[derive(Debug, Clone, Copy)]
pub struct ForgetPlan {
    pub shard: ShardId,
    pub memory_id: MemoryId,
    pub mode: ForgetMode,
    pub estimated_cost_ms: f32,
}
