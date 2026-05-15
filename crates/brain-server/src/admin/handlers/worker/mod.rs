//! Admin HTTP handlers for `worker` (spec §14/06 §6; sub-task 10.11).
//!
//! Routes:
//! - `GET /v1/workers[?shard=N]` → 200 + per-shard worker snapshots.
//! - `POST /v1/workers/{name}/{stop|start|run-now}` — F-13 wired the
//!   live control plane. `stop` pauses the loop's `run_cycle`; the
//!   loop keeps ticking on its interval so the worker can be resumed
//!   without restarting the shard. `start` resumes a paused worker
//!   (and kicks the wake channel so the next cycle runs without
//!   waiting out the current sleep). `run-now` triggers a single
//!   immediate cycle.

mod control;
mod list;

pub use control::control;
pub use list::list;

/// Workers known to the Phase-3 scheduler. Shared with the control
/// endpoint for input validation.
pub(super) const KNOWN_WORKERS: &[&str] = &[
    "decay",
    "access_boost",
    "consolidation",
    "hnsw_maintenance",
    "idempotency_cleanup",
    "slot_reclamation",
    "wal_retention",
    "edge_scrub",
    "counter_reconcile",
    "statistics",
    "embedder_cache_evict",
    "snapshot",
];

/// Control actions accepted on the deferred `POST /v1/workers/{name}/{action}`.
pub(super) const KNOWN_ACTIONS: &[&str] = &["stop", "start", "run-now"];
