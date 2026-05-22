//! `ForgetCascadeWorker` metric family.
//!
//! Mirrors `AutoEdgeMetrics`: the writer bumps `drops_total` on a full
//! channel; the worker bumps everything else from inside its cycle
//! loop. Both ends hold the same `Arc<ForgetCascadeMetrics>` so a
//! `/metrics` snapshot covers them in one read.

use std::sync::atomic::{AtomicU64, Ordering};

/// Per-shard counters surfacing the cascade's behaviour. All fields are
/// monotonic — `snapshot()` is a point-in-time read suitable for
/// Prometheus exposition.
#[derive(Debug)]
pub struct ForgetCascadeMetrics {
    drops_total: AtomicU64,
    jobs_processed: AtomicU64,
    statements_evidence_dropped: AtomicU64,
    statements_tombstoned: AtomicU64,
    statements_kept_stale: AtomicU64,
    relations_tombstoned: AtomicU64,
    relations_evidence_dropped: AtomicU64,
    edges_unlinked: AtomicU64,
}

impl ForgetCascadeMetrics {
    /// Construct a zeroed instance. One per shard at startup, shared
    /// by `Arc` between the writer's enqueue path and the worker's
    /// cycle loop.
    #[must_use]
    pub fn new() -> Self {
        Self {
            drops_total: AtomicU64::new(0),
            jobs_processed: AtomicU64::new(0),
            statements_evidence_dropped: AtomicU64::new(0),
            statements_tombstoned: AtomicU64::new(0),
            statements_kept_stale: AtomicU64::new(0),
            relations_tombstoned: AtomicU64::new(0),
            relations_evidence_dropped: AtomicU64::new(0),
            edges_unlinked: AtomicU64::new(0),
        }
    }

    /// Bumped by the writer's `try_send` path when the bounded
    /// cascade channel is full. The FORGET op itself still succeeds —
    /// readers will just see a stale-confidence statement until a
    /// later cascade catches up (admin manual re-trigger or a
    /// subsequent FORGET that drains the queue).
    pub fn inc_drop(&self) {
        self.drops_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Bumped once per processed job after a successful commit.
    pub fn add_job_processed(&self) {
        self.jobs_processed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn add_statements_evidence_dropped(&self, n: u64) {
        self.statements_evidence_dropped
            .fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_statements_tombstoned(&self, n: u64) {
        self.statements_tombstoned.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_statements_kept_stale(&self, n: u64) {
        self.statements_kept_stale.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_relations_tombstoned(&self, n: u64) {
        self.relations_tombstoned.fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_relations_evidence_dropped(&self, n: u64) {
        self.relations_evidence_dropped
            .fetch_add(n, Ordering::Relaxed);
    }

    pub fn add_edges_unlinked(&self, n: u64) {
        self.edges_unlinked.fetch_add(n, Ordering::Relaxed);
    }

    #[must_use]
    pub fn snapshot(&self) -> ForgetCascadeMetricsSnapshot {
        ForgetCascadeMetricsSnapshot {
            drops_total: self.drops_total.load(Ordering::Relaxed),
            jobs_processed: self.jobs_processed.load(Ordering::Relaxed),
            statements_evidence_dropped: self.statements_evidence_dropped.load(Ordering::Relaxed),
            statements_tombstoned: self.statements_tombstoned.load(Ordering::Relaxed),
            statements_kept_stale: self.statements_kept_stale.load(Ordering::Relaxed),
            relations_tombstoned: self.relations_tombstoned.load(Ordering::Relaxed),
            relations_evidence_dropped: self.relations_evidence_dropped.load(Ordering::Relaxed),
            edges_unlinked: self.edges_unlinked.load(Ordering::Relaxed),
        }
    }
}

impl Default for ForgetCascadeMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Plain-data snapshot of [`ForgetCascadeMetrics`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForgetCascadeMetricsSnapshot {
    pub drops_total: u64,
    pub jobs_processed: u64,
    pub statements_evidence_dropped: u64,
    pub statements_tombstoned: u64,
    pub statements_kept_stale: u64,
    pub relations_tombstoned: u64,
    pub relations_evidence_dropped: u64,
    pub edges_unlinked: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascade_counters_start_at_zero() {
        let m = ForgetCascadeMetrics::new();
        let s = m.snapshot();
        assert_eq!(s.drops_total, 0);
        assert_eq!(s.jobs_processed, 0);
        assert_eq!(s.statements_evidence_dropped, 0);
        assert_eq!(s.statements_tombstoned, 0);
        assert_eq!(s.statements_kept_stale, 0);
        assert_eq!(s.relations_tombstoned, 0);
        assert_eq!(s.relations_evidence_dropped, 0);
        assert_eq!(s.edges_unlinked, 0);
    }

    #[test]
    fn cascade_increments_round_trip() {
        let m = ForgetCascadeMetrics::new();
        m.inc_drop();
        m.add_job_processed();
        m.add_statements_evidence_dropped(3);
        m.add_statements_tombstoned(1);
        m.add_statements_kept_stale(2);
        m.add_relations_tombstoned(4);
        m.add_relations_evidence_dropped(5);
        m.add_edges_unlinked(6);
        let s = m.snapshot();
        assert_eq!(s.drops_total, 1);
        assert_eq!(s.jobs_processed, 1);
        assert_eq!(s.statements_evidence_dropped, 3);
        assert_eq!(s.statements_tombstoned, 1);
        assert_eq!(s.statements_kept_stale, 2);
        assert_eq!(s.relations_tombstoned, 4);
        assert_eq!(s.relations_evidence_dropped, 5);
        assert_eq!(s.edges_unlinked, 6);
    }
}
