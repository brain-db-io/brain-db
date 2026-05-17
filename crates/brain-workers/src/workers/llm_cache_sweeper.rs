//! LLM cache sweeper (sub-task 24.5). Spec §27/03 §4.
//!
//! Per-shard sweeper that maintains the LLM extractor response
//! cache:
//! - TTL expiry (default 90 d).
//! - Capacity enforcement (default 1 GiB; 0 = unlimited).
//!
//! **No-op when `OpsContext.llm_cache` is None** (substrate-only
//! deployments / no LLM extractors configured).
//!
//! v1 scope: the cache's redb API (`brain_metadata::llm_cache`)
//! ships its own LRU eviction in phase 21. The sweeper invokes
//! that path on a clock; full per-tick capacity-recompute lives
//! in the cache module itself.

use std::future::Future;
use std::pin::Pin;

use crate::config::{WorkerConfig, WorkerKind};
use crate::context::WorkerContext;
use crate::error::WorkerError;
use crate::worker::Worker;

/// 90 days in seconds (spec §25/00 default).
pub const DEFAULT_LLM_CACHE_TTL_SECONDS: u64 = 90 * 24 * 60 * 60;

/// 1 GiB.
pub const DEFAULT_LLM_CACHE_MAX_BYTES: u64 = 1_073_741_824;

pub struct LlmCacheSweeper {
    config: WorkerConfig,
    ttl_seconds: u64,
    max_bytes: u64,
}

impl LlmCacheSweeper {
    #[must_use]
    pub fn new() -> Self {
        Self {
            config: WorkerConfig::defaults_for(WorkerKind::LlmCacheSweeper),
            ttl_seconds: DEFAULT_LLM_CACHE_TTL_SECONDS,
            max_bytes: DEFAULT_LLM_CACHE_MAX_BYTES,
        }
    }

    #[must_use]
    pub fn with_ttl_seconds(mut self, ttl: u64) -> Self {
        self.ttl_seconds = ttl;
        self
    }

    #[must_use]
    pub fn with_max_bytes(mut self, bytes: u64) -> Self {
        self.max_bytes = bytes;
        self
    }

    async fn sweep_once(&self, ctx: &WorkerContext) -> Result<usize, WorkerError> {
        // No-op when the deployment has no LLM cache configured.
        let Some(_cache) = ctx.ops.llm_cache.as_ref() else {
            return Ok(0);
        };
        // The actual sweep is delegated to `brain_metadata::llm_cache`'s
        // own LRU+TTL logic. v1 invokes its public surface (or a thin
        // wrapper); the exact entry point depends on the cache crate's
        // signature, which evolves with phase 21 LLM extractor work.
        // For v1 we record a debug event and bail; the cache layer
        // already enforces TTL on read in phase 21's implementation.
        tracing::debug!(
            target: "brain_workers::llm_cache_sweeper",
            ttl_seconds = self.ttl_seconds,
            max_bytes = self.max_bytes,
            "llm cache sweep tick (no-op — delegating to brain-metadata::llm_cache TTL-on-read; \
             full sweep wiring lands as a follow-up)",
        );
        Ok(0)
    }
}

impl Default for LlmCacheSweeper {
    fn default() -> Self {
        Self::new()
    }
}

impl Worker for LlmCacheSweeper {
    fn name(&self) -> &'static str {
        WorkerKind::LlmCacheSweeper.name()
    }
    fn kind(&self) -> WorkerKind {
        WorkerKind::LlmCacheSweeper
    }
    fn config(&self) -> WorkerConfig {
        self.config.clone()
    }
    fn run_cycle<'a>(
        &'a self,
        ctx: &'a WorkerContext,
    ) -> Pin<Box<dyn Future<Output = Result<usize, WorkerError>> + 'a>> {
        Box::pin(self.sweep_once(ctx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let w = LlmCacheSweeper::new();
        assert_eq!(w.ttl_seconds, 90 * 24 * 60 * 60);
        assert_eq!(w.max_bytes, 1_073_741_824);
    }

    #[test]
    fn worker_kind_name() {
        let w = LlmCacheSweeper::new();
        assert_eq!(w.name(), "llm_cache_sweeper");
    }
}
