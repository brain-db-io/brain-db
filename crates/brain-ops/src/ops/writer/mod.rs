//! Real per-shard write surface.
//!
//! Implements `brain_planner::WriterHandle` against real
//! `MetadataDb` + `HnswWriter`. Idempotency lives here because spec
//! §08/04 §4 + §07/06 §3 mandate the lookup-then-act protocol with
//! the response payload written in the **same redb txn** as the
//! memory row.
//!
//! **No WAL** — spec §08/08 §10's group-commit channel-fed writer
//! lands in Phase 8 / 9. The trait surface doesn't change; production
//! swaps the implementation.
//!
//! Concurrency: every interior mutable piece is `Mutex`-wrapped.
//! Concurrent submits serialise on the metadata mutex; throughput is
//! bounded by redb's single-writer-per-database lock, which matches
//! the spec §07/08 §3 single-writer-per-shard discipline.
//!
//! ## Edge maintenance
//!
//! - **Encode-inline edges** (spec §09/02 §1.5): each `EncodeOpEdge`
//!   targeting a live memory is inserted into `edges_out` + `edges_in`
//!   via [`brain_metadata::tables::edge::link`], and the source /
//!   target memory rows' `edges_out_count` / `edges_in_count` denorms
//!   are bumped — all inside the same write txn as the memory row.
//! - **LINK** (spec §09/07 §1-§3): same pattern. `do_link` returns
//!   `already_existed=true` when the canonical `(source, kind, target)`
//!   was present (overwrite-weight semantics, no count bump).
//! - **UNLINK** (spec §09/07 §4-§5): non-existent edge is a no-op
//!   (`removed=false`), not an error. Successful unlink decrements
//!   both counts.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use brain_core::{AgentId, MemoryId, ShardId};
use brain_index::Writer as HnswWriter;
use brain_metadata::tables::edge::{EDGES_REVERSE_TABLE, EDGES_TABLE};
use brain_metadata::tables::fingerprint::FINGERPRINTS_TABLE;
use brain_metadata::tables::idempotency::IDEMPOTENCY_TABLE;
use brain_metadata::tables::memory::{MEMORIES_BY_AGENT_TIMELINE_TABLE, MEMORIES_TABLE};
use brain_metadata::tables::text::TEXTS_TABLE;
use brain_planner::{SharedMetadataDb, WriterError, WriterHandle};
use parking_lot::Mutex;
use uuid::Uuid;

use crate::subscribe::EventBus;

/// Real per-shard writer backed by `MetadataDb` + `HnswWriter`. No
/// WAL — Phase 8 / 9 swap this for a WAL-backed implementation
/// without changing `WriterHandle`'s public surface.
pub struct RealWriterHandle {
    metadata: SharedMetadataDb,
    hnsw_writer: Mutex<HnswWriter<384>>,
    /// In-process slot counter. Phase 8 / 9 will replace with the
    /// arena allocator. Starts at 1.
    next_slot: AtomicU64,
    /// Agent id stamped on every memory metadata row. Phase 9 will
    /// derive this from the authenticated connection; for now it's
    /// nil. Carried as a field so tests + the future server can pin
    /// it without re-creating the writer.
    agent_id: AgentId,
    /// Shard id stamped into every `MemoryId` this writer issues.
    /// Routing back to the owning shard (LINK / UNLINK / FORGET in
    /// `brain-server::network::dispatch::shard_for_memory`) reads
    /// the shard prefix from the `MemoryId`, so stamping the wrong
    /// shard here silently steers follow-up ops to the wrong shard
    /// and surfaces as `NotFound`. Defaults to `0`; production
    /// callers must override via [`Self::with_shard_id`].
    shard_id: ShardId,
    /// Change-feed publisher (sub-task 7.10). Single-op encode/forget
    /// commits and TXN_COMMIT batches publish here *after* the redb
    /// commit() succeeds. Optional so existing callers don't break
    /// (defaults to no publication — events are dropped on the floor).
    events: Option<Arc<EventBus>>,
    /// WAL append sink (Phase 9 wiring). When `Some`, every write
    /// op appends a typed [`brain_storage::wal::payload::WalPayload`]
    /// record to the WAL **before** mutating redb — establishing the
    /// spec §05/07 durability barrier. The returned LSN is stamped
    /// onto the published event so subscribe-replay finds the right
    /// position. When `None`, the writer falls back to the legacy
    /// "redb-first, EventBus mints LSN" path used by unit tests that
    /// don't spin up a shard.
    wal_sink: Option<Arc<dyn wal_sink::WalSink>>,
    /// Optional non-blocking sender feeding the per-shard
    /// AutoEdgeWorker. Each successful ENCODE enqueues
    /// `(memory_id, vector)` post-fsync + post-commit + post-HNSW; the
    /// worker drains the channel and writes SimilarTo edges back into
    /// the unified edge tables. `None` means the worker isn't wired
    /// for this build (gated by config); enqueue becomes a no-op.
    // TODO(part-3): make non-optional when auto-edge is unconditionally
    // wired at shard spawn.
    auto_edge_tx: Option<flume::Sender<AutoEdgeEnqueue>>,
    /// Optional non-blocking sender feeding the per-shard
    /// ExtractorWorker. Each successful ENCODE enqueues
    /// `(memory_id, text)` post-WAL-fsync + post-commit + post-HNSW;
    /// the worker drains the channel and runs the three-tier
    /// extractor pipeline against the text. `None` means the worker
    /// isn't wired (gated by config); enqueue becomes a no-op. The
    /// `Arc<str>` keeps the payload cheap to push and avoids the
    /// worker re-reading text from the metadata DB on a hot path.
    // TODO(part-3): make non-optional when extractor pool is unconditionally
    // wired at shard spawn (entity HNSW + statement HNSW dependencies land then).
    extractor_tx: Option<flume::Sender<ExtractorEnqueue>>,
    /// Optional non-blocking sender feeding the per-shard
    /// TemporalEdgeWorker. Each successful ENCODE enqueues
    /// `(memory_id, agent_id, context_id, created_at_unix_nanos)`
    /// post-commit; the worker looks up the previous memory for the
    /// same agent + context in `MEMORIES_BY_AGENT_TIMELINE_TABLE`
    /// and writes a `FollowedBy` auto-edge with decay-weighted
    /// strength. `None` → worker disabled.
    temporal_edge_tx: Option<flume::Sender<TemporalEdgeEnqueue>>,
    /// Shared metric handle for the AutoEdgeWorker family. When
    /// wired (production), `try_enqueue_auto_edge` bumps `drops_total`
    /// on `Full`. The worker holds the same `Arc` and publishes
    /// edges-written / cycle-duration / neighbours-found into it.
    /// `None` in test fixtures that don't care about observability.
    auto_edge_metrics: Option<Arc<crate::worker_metrics::AutoEdgeMetrics>>,
    /// Companion to [`Self::auto_edge_metrics`] for the extractor
    /// pipeline. Writer bumps `drops_total`; worker publishes the
    /// rest.
    extractor_metrics: Option<Arc<crate::worker_metrics::ExtractorMetrics>>,
    /// Companion to [`Self::auto_edge_metrics`] for the
    /// TemporalEdgeWorker. Writer bumps `drops_total` on `Full`;
    /// the worker publishes edges-written / cycle-duration / skip
    /// reasons.
    temporal_edge_metrics: Option<Arc<crate::worker_metrics::TemporalEdgeMetrics>>,
    /// Optional memory tantivy dispatcher. Wired by the shard's
    /// spawn path when the deployment has a tantivy handle. After
    /// each successful ENCODE (single op or TXN batch) the writer
    /// dispatches a `MemoryTextOp::Upsert` so lexical search sees
    /// the new memory in the same coordinate system as HNSW + redb.
    /// Lives on the writer (not the outer handler) so the batch
    /// path doesn't have to know about lexical indexing.
    memory_text_dispatcher: Option<Arc<crate::ops::text_indexer::MemoryTextDispatcher>>,
    /// Per-writer idempotency cache for the universal `submit(Write)`
    /// path. Distinct from the redb-backed substrate cache (which keys
    /// by `RequestId` and lives in `IDEMPOTENCY_TABLE`). The two will
    /// merge in P3c when this cache becomes redb-backed and keys by
    /// `WriteId`.
    write_idempotency: Arc<submit::WriteIdempotencyCache>,
}

/// What the writer pushes into the AutoEdgeWorker's channel after a
/// successful ENCODE. The vector is carried inline so the worker can
/// run `HNSW.search_active` without re-reading from arena or HNSW (the
/// HNSW reader has no public vector accessor, and arena reads would
/// require crossing the storage boundary into the worker).
pub type AutoEdgeEnqueue = (brain_core::MemoryId, [f32; brain_embed::VECTOR_DIM]);

/// What the writer pushes into the ExtractorWorker's channel after a
/// successful ENCODE. Text travels inline as an `Arc<str>` so the
/// worker can clone cheaply across cycles without re-reading the
/// row from redb (which would re-cross the metadata lock on the
/// extractor's hot path).
pub type ExtractorEnqueue = (brain_core::MemoryId, std::sync::Arc<str>);

/// What the writer pushes into the TemporalEdgeWorker's channel after
/// a successful ENCODE. The worker only needs the indexable fields:
/// the freshly-inserted memory's `(memory_id, agent_id, context_id,
/// created_at_unix_nanos)`. It looks up the predecessor via
/// `MEMORIES_BY_AGENT_TIMELINE_TABLE` itself — no need to carry the
/// vector or text.
pub type TemporalEdgeEnqueue = (
    brain_core::MemoryId,
    brain_core::AgentId,
    brain_core::ContextId,
    u64,
);

/// What the ExtractorWorker pushes into the CausalEdgeWorker's channel
/// post-statement-commit, when the statement's predicate is in the
/// resolved causal whitelist. Only the `StatementId` travels — the
/// CausalEdgeWorker reads the full row + walks the cause-side
/// `STATEMENTS_BY_SUBJECT` index inside its own rtxn. Carrying the
/// statement inline would make the enqueue tuple awkward (variable-size
/// evidence) without saving any redb work on the worker side.
pub type CausalEdgeEnqueue = brain_core::StatementId;

impl RealWriterHandle {
    #[must_use]
    pub fn new(metadata: SharedMetadataDb, hnsw_writer: HnswWriter<384>) -> Self {
        // Materialise the tables we read from. redb creates tables
        // on first write_txn().open_table(), but read_txn() on a
        // never-opened table returns `TableDoesNotExist`. We do a
        // one-time empty write txn at construction so subsequent
        // idempotency + metadata reads succeed even before the
        // first submit. Every substrate table that any reader path
        // touches (dedup lookup, timeline walks, edge planner reads)
        // must be listed here, otherwise the first read on a fresh
        // shard explodes.
        {
            let mut db = metadata.lock();
            if let Ok(wtxn) = db.write_txn() {
                let _ = wtxn.open_table(MEMORIES_TABLE);
                let _ = wtxn.open_table(MEMORIES_BY_AGENT_TIMELINE_TABLE);
                let _ = wtxn.open_table(IDEMPOTENCY_TABLE);
                let _ = wtxn.open_table(EDGES_TABLE);
                let _ = wtxn.open_table(EDGES_REVERSE_TABLE);
                let _ = wtxn.open_table(FINGERPRINTS_TABLE);
                let _ = wtxn.open_table(TEXTS_TABLE);
                let _ = wtxn.commit();
            }
        }
        Self {
            metadata,
            hnsw_writer: Mutex::new(hnsw_writer),
            next_slot: AtomicU64::new(1),
            agent_id: AgentId(Uuid::nil()),
            shard_id: 0,
            events: None,
            wal_sink: None,
            auto_edge_tx: None,
            extractor_tx: None,
            temporal_edge_tx: None,
            auto_edge_metrics: None,
            extractor_metrics: None,
            temporal_edge_metrics: None,
            memory_text_dispatcher: None,
            write_idempotency: Arc::new(submit::WriteIdempotencyCache::new()),
        }
    }

    /// Accessor for the unified write-path idempotency cache.
    /// Used by [`submit::submit`] and exposed for tests + future
    /// admin observability.
    #[must_use]
    pub fn write_idempotency_cache(&self) -> &submit::WriteIdempotencyCache {
        &self.write_idempotency
    }

    /// Accessor for the shared metadata DB. Lets [`submit::submit`]
    /// lock + open wtxns; lets tests inspect post-commit state. Kept
    /// pub(crate) so external code goes through the writer's submit
    /// methods rather than the metadata directly.
    #[must_use]
    pub(crate) fn metadata(&self) -> &SharedMetadataDb {
        &self.metadata
    }

    /// Accessor for the optional EventBus. Lets [`submit::submit`]
    /// publish post-commit events to subscribers. `None` when the
    /// writer was constructed without `with_event_bus` (test path).
    #[must_use]
    pub(crate) fn event_bus(&self) -> Option<&Arc<EventBus>> {
        self.events.as_ref()
    }

    /// Accessor for the optional WAL sink. Lets the unified
    /// `submit(Write)` path append durable records before opening
    /// the redb wtxn. `None` when the writer was constructed without
    /// `with_wal_sink` (test path).
    #[must_use]
    pub(crate) fn wal_sink_ref(&self) -> Option<&Arc<dyn wal_sink::WalSink>> {
        self.wal_sink.as_ref()
    }

    /// Lock the HNSW writer for the unified path's side-effect step.
    /// Returns a `MutexGuard` so the caller holds the lock for the
    /// minimum window (single insert / mark_tombstoned).
    pub(crate) fn hnsw_writer_lock(&self) -> parking_lot::MutexGuard<'_, brain_index::Writer<384>> {
        self.hnsw_writer.lock()
    }

    #[must_use]
    pub fn with_agent_id(mut self, agent_id: AgentId) -> Self {
        self.agent_id = agent_id;
        self
    }

    /// Stamp the shard prefix on every `MemoryId` this writer issues.
    /// Production must call this with the owning shard's id;
    /// otherwise LINK / UNLINK / FORGET route to the wrong shard and
    /// return `NotFound`. Tests on a single shard can keep the
    /// default (`0`).
    #[must_use]
    pub fn with_shard_id(mut self, shard_id: ShardId) -> Self {
        self.shard_id = shard_id;
        self
    }

    /// Wire the change-feed bus. After this call every successful
    /// commit publishes an [`EventEnvelope`] onto the bus.
    #[must_use]
    pub fn with_event_bus(mut self, bus: Arc<EventBus>) -> Self {
        self.events = Some(bus);
        self
    }

    /// Wire the WAL sink. After this call every write op runs the
    /// spec §05/07 ordering — WAL append → fsync → redb → indexes →
    /// publish — so a restart can replay the durable log onto a fresh
    /// shard and a subscribe-replay can synthesise the change feed
    /// from segment data.
    #[must_use]
    pub fn with_wal_sink(mut self, sink: Arc<dyn wal_sink::WalSink>) -> Self {
        self.wal_sink = Some(sink);
        self
    }

    /// Wire the AutoEdgeWorker's feed channel. After this call every
    /// successful ENCODE enqueues `(memory_id, vector)` into the
    /// channel post-fsync + post-commit + post-HNSW. Without this call
    /// the enqueue path is a no-op and no auto-derived edges are
    /// produced — used by unit-test fixtures and by builds that have
    /// the worker disabled in config.
    ///
    /// The channel must be bounded; on `Full` the writer logs a warn
    /// and drops the enqueue (encode still succeeds — auto-edges are
    /// best-effort). On `Disconnected` the writer logs at debug and
    /// continues.
    pub fn set_auto_edge_sender(&mut self, sender: flume::Sender<AutoEdgeEnqueue>) {
        self.auto_edge_tx = Some(sender);
    }

    /// Accessor for the auto-edge sender so the encode + batch paths
    /// can call `try_send` without re-borrowing the whole writer.
    pub(super) fn auto_edge_sender(&self) -> Option<&flume::Sender<AutoEdgeEnqueue>> {
        self.auto_edge_tx.as_ref()
    }

    /// Wire the ExtractorWorker's feed channel. After this call every
    /// successful ENCODE enqueues `(memory_id, text)` post-fsync +
    /// post-commit + post-HNSW. Without this call the enqueue path is
    /// a no-op and no auto-extraction happens — used by unit-test
    /// fixtures and by builds that have the worker disabled in
    /// config.
    ///
    /// Channel must be bounded; on `Full` the writer logs a warn and
    /// drops the enqueue (encode still succeeds — extraction is best-
    /// effort). On `Disconnected` the writer logs at debug.
    pub fn set_extractor_sender(&mut self, sender: flume::Sender<ExtractorEnqueue>) {
        self.extractor_tx = Some(sender);
    }

    /// Accessor for the extractor sender so the encode + batch paths
    /// can call `try_send` without re-borrowing the whole writer.
    pub(super) fn extractor_sender(&self) -> Option<&flume::Sender<ExtractorEnqueue>> {
        self.extractor_tx.as_ref()
    }

    /// Install the shared `AutoEdgeMetrics` handle. The same `Arc`
    /// must be threaded to the matching `AutoEdgeWorker` so both
    /// sides see the same counters. Drop counters become visible to
    /// `/metrics` as soon as this is wired.
    pub fn set_auto_edge_metrics(&mut self, metrics: Arc<crate::worker_metrics::AutoEdgeMetrics>) {
        self.auto_edge_metrics = Some(metrics);
    }

    pub(super) fn auto_edge_metrics(&self) -> Option<&Arc<crate::worker_metrics::AutoEdgeMetrics>> {
        self.auto_edge_metrics.as_ref()
    }

    /// Install the shared `ExtractorMetrics` handle. Same semantics
    /// as [`Self::set_auto_edge_metrics`].
    pub fn set_extractor_metrics(&mut self, metrics: Arc<crate::worker_metrics::ExtractorMetrics>) {
        self.extractor_metrics = Some(metrics);
    }

    pub(super) fn extractor_metrics(
        &self,
    ) -> Option<&Arc<crate::worker_metrics::ExtractorMetrics>> {
        self.extractor_metrics.as_ref()
    }

    /// Wire the TemporalEdgeWorker's feed channel. After this call
    /// every successful ENCODE enqueues
    /// `(memory_id, agent_id, context_id, created_at_unix_nanos)`
    /// post-commit. Without this call the enqueue path is a no-op
    /// (matches `set_auto_edge_sender`).
    pub fn set_temporal_edge_sender(&mut self, sender: flume::Sender<TemporalEdgeEnqueue>) {
        self.temporal_edge_tx = Some(sender);
    }

    pub(super) fn temporal_edge_sender(&self) -> Option<&flume::Sender<TemporalEdgeEnqueue>> {
        self.temporal_edge_tx.as_ref()
    }

    /// Install the shared `TemporalEdgeMetrics` handle. Same semantics
    /// as [`Self::set_auto_edge_metrics`].
    pub fn set_temporal_edge_metrics(
        &mut self,
        metrics: Arc<crate::worker_metrics::TemporalEdgeMetrics>,
    ) {
        self.temporal_edge_metrics = Some(metrics);
    }

    pub(super) fn temporal_edge_metrics(
        &self,
    ) -> Option<&Arc<crate::worker_metrics::TemporalEdgeMetrics>> {
        self.temporal_edge_metrics.as_ref()
    }

    /// Install the memory text dispatcher. After this call, every
    /// successful ENCODE (single-op or TXN batch) enqueues a
    /// `MemoryTextOp::Upsert` to the dispatcher so the lexical
    /// indexer worker picks it up. Without this call the writer
    /// skips lexical dispatch — used by tests that don't open
    /// tantivy.
    pub fn set_memory_text_dispatcher(
        &mut self,
        dispatcher: Arc<crate::ops::text_indexer::MemoryTextDispatcher>,
    ) {
        self.memory_text_dispatcher = Some(dispatcher);
    }
}

impl WriterHandle for RealWriterHandle {
    fn reserve_memory_id<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<MemoryId, WriterError>> + 'a>> {
        Box::pin(async move {
            let slot = self.next_slot.fetch_add(1, Ordering::Relaxed);
            // Lookup the persisted version for this slot — fresh slots
            // have no row (mint as version 1); recycled slots carry
            // the post-reclaim bumped version. Stale references to the
            // prior occupant then mismatch on every read path.
            let version: u32 = {
                let db = self.metadata.lock();
                let rtxn = db
                    .read_txn()
                    .map_err(|e| WriterError::Internal(format!("reserve slot ver read: {e:?}")))?;
                match rtxn.open_table(brain_metadata::tables::slot_version::SLOT_VERSIONS_TABLE) {
                    Ok(table) => table
                        .get(&slot)
                        .map_err(|e| WriterError::Internal(format!("reserve slot ver get: {e:?}")))?
                        .map_or(1, |a| a.value()),
                    Err(redb::TableError::TableDoesNotExist(_)) => 1,
                    Err(e) => {
                        return Err(WriterError::Internal(format!(
                            "reserve slot ver open: {e:?}"
                        )))
                    }
                }
            };
            Ok(MemoryId::pack(self.shard_id, slot, version))
        })
    }

    fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn enqueue_for_extraction(&self, memory_id: MemoryId, text: &str) -> bool {
        let Some(sender) = self.extractor_sender() else {
            return false;
        };
        let payload: ExtractorEnqueue = (memory_id, std::sync::Arc::from(text));
        match sender.try_send(payload) {
            Ok(()) => true,
            Err(flume::TrySendError::Full(_)) => {
                tracing::warn!(
                    memory_id = ?memory_id,
                    "extract_backfill: extractor channel full; dropping enqueue"
                );
                false
            }
            Err(flume::TrySendError::Disconnected(_)) => {
                tracing::debug!(
                    memory_id = ?memory_id,
                    "extract_backfill: extractor worker disconnected"
                );
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handler modules — one per cognitive op handler.
// ---------------------------------------------------------------------------

pub mod submit;
pub mod wal_map;
pub mod wal_sink;

pub use wal_sink::{
    channel_wal_sink, channel_wal_sink_with_capacity, ChannelWalSink, FailingWalSink, NoopWalSink,
    RecordingWalSink, WalAppendMessage, WalSink, WalSinkError, DEFAULT_WAL_DRAIN_CAPACITY,
};

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Enqueue `(memory_id, vector)` onto the AutoEdgeWorker channel if
/// one is wired. Non-blocking; full channel logs a warn and drops
/// (encode succeeds without auto-edges). Disconnected channel logs at
/// debug. This is the single enqueue point both the single-encode and
/// TXN batch paths route through.
pub(crate) fn try_enqueue_auto_edge(
    writer: &RealWriterHandle,
    memory_id: MemoryId,
    vector: &[f32; brain_embed::VECTOR_DIM],
) {
    let Some(sender) = writer.auto_edge_sender() else {
        return;
    };
    match sender.try_send((memory_id, *vector)) {
        Ok(()) => {
            tracing::trace!(
                memory_id = ?memory_id,
                "auto_edge enqueue"
            );
        }
        Err(flume::TrySendError::Full(_)) => {
            if let Some(m) = writer.auto_edge_metrics() {
                m.inc_drop();
            }
            tracing::warn!(
                memory_id = ?memory_id,
                "auto_edge channel full; dropping enqueue"
            );
        }
        Err(flume::TrySendError::Disconnected(_)) => {
            tracing::debug!(
                memory_id = ?memory_id,
                "auto_edge worker disconnected; encode continues"
            );
        }
    }
}

/// Enqueue `(memory_id, agent_id, context_id, created_at_unix_nanos)`
/// onto the TemporalEdgeWorker channel if one is wired. Mirrors
/// [`try_enqueue_auto_edge`] semantics — full channel drops with a
/// counter bump; disconnected logs at debug.
pub(crate) fn try_enqueue_temporal_edge(
    writer: &RealWriterHandle,
    memory_id: MemoryId,
    agent_id: brain_core::AgentId,
    context_id: brain_core::ContextId,
    created_at_unix_nanos: u64,
) {
    let Some(sender) = writer.temporal_edge_sender() else {
        return;
    };
    let payload: TemporalEdgeEnqueue = (memory_id, agent_id, context_id, created_at_unix_nanos);
    match sender.try_send(payload) {
        Ok(()) => {
            tracing::trace!(memory_id = ?memory_id, "temporal_edge enqueue");
        }
        Err(flume::TrySendError::Full(_)) => {
            if let Some(m) = writer.temporal_edge_metrics() {
                m.inc_drop();
            }
            tracing::warn!(
                memory_id = ?memory_id,
                "temporal_edge channel full; dropping enqueue"
            );
        }
        Err(flume::TrySendError::Disconnected(_)) => {
            tracing::debug!(
                memory_id = ?memory_id,
                "temporal_edge worker disconnected; encode continues"
            );
        }
    }
}

/// Enqueue `(memory_id, text)` onto the ExtractorWorker channel if
/// one is wired. Non-blocking; full channel logs a warn and drops
/// (encode succeeds without extraction). Disconnected channel logs
/// at debug. This is the single enqueue point both the single-encode
/// and TXN batch paths route through.
pub(crate) fn try_enqueue_extractor(writer: &RealWriterHandle, memory_id: MemoryId, text: &str) {
    let Some(sender) = writer.extractor_sender() else {
        return;
    };
    let payload: ExtractorEnqueue = (memory_id, std::sync::Arc::from(text));
    match sender.try_send(payload) {
        Ok(()) => {
            tracing::trace!(
                memory_id = ?memory_id,
                "extractor enqueue"
            );
        }
        Err(flume::TrySendError::Full(_)) => {
            if let Some(m) = writer.extractor_metrics() {
                m.inc_drop();
            }
            tracing::warn!(
                memory_id = ?memory_id,
                "extractor channel full; dropping enqueue"
            );
        }
        Err(flume::TrySendError::Disconnected(_)) => {
            tracing::debug!(
                memory_id = ?memory_id,
                "extractor worker disconnected; encode continues"
            );
        }
    }
}
