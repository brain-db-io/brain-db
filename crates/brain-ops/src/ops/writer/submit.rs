//! Universal `submit(Write)` — the unified write path's entry point.
//!
//! Replaces (eventually) the five specialised `submit_encode` /
//! `submit_forget` / `submit_link` / `submit_unlink` / `submit_batch`
//! methods on [`super::RealWriterHandle`]. Lives alongside them during
//! P3-P4 so handlers can migrate one at a time; the old methods get
//! deleted in P4 once the last caller is gone.
//!
//! ## The pipeline
//!
//! For every submitted [`Write`] the writer does:
//!
//! 1. Idempotency check (in-memory cache for now; durable redb-backed
//!    cache lands in P3c).
//! 2. Open ONE `WriteTransaction`.
//! 3. For each phase: call [`apply::dispatch`] against the wtxn.
//! 4. Commit.
//! 5. Stamp the idempotency cache.
//! 6. Return the [`WriteAck`].
//!
//! WAL framing (P3b) and post-commit event publishing (P3c) layer on
//! top — both are additive to this skeleton. The phase apply functions
//! never read clocks / mint ids / publish events, so the writer is the
//! only place those side-effects live; adding them later doesn't
//! require apply changes.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use brain_planner::WriterError;
use brain_storage::wal::record::Lsn;

use crate::apply::{dispatch, ApplyError};
use crate::write::{PhaseAck, Write, WriteAck, WriteId};

use super::RealWriterHandle;

/// In-memory idempotency cache. Indexed by [`WriteId`] → cached ack.
///
/// Two reasons to keep it in-memory for P3:
/// 1. The redb-backed durable cache (which keys substrate ops by
///    `RequestId` today) lands in P3c — same redb table, just keyed
///    by `WriteId` so universal writes share the cache.
/// 2. P3 needs a working idempotency story to exercise retry semantics
///    in unit tests without dragging in the WAL.
#[derive(Default)]
pub struct WriteIdempotencyCache {
    entries: parking_lot::Mutex<std::collections::HashMap<WriteId, Arc<WriteAck>>>,
}

impl WriteIdempotencyCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Lookup; returns the cached ack if present.
    pub fn lookup(&self, id: WriteId) -> Option<Arc<WriteAck>> {
        self.entries.lock().get(&id).cloned()
    }

    /// Stamp; replaces any prior entry for `id`. (Replays carry the
    /// same `WriteId`; a different ack for the same id means the
    /// caller used a fresh `WriteId::new()` — that's a different write.)
    pub fn stamp(&self, id: WriteId, ack: Arc<WriteAck>) {
        self.entries.lock().insert(id, ack);
    }

    /// How many entries the cache currently holds. Used by tests +
    /// metrics; the production cache will spill / evict.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.lock().len()
    }

    /// Whether the cache is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.lock().is_empty()
    }
}

impl RealWriterHandle {
    /// Submit a [`Write`]. Universal entry point for the unified write
    /// path. Applies all phases atomically against one redb wtxn.
    ///
    /// # Errors
    /// - [`WriterError::Internal`] for storage / apply failures (the
    ///   wtxn auto-rolls-back on drop).
    /// - [`WriterError::Conflict`] for idempotency mismatch — same
    ///   `WriteId`, different phases. (Not yet wired in P3; the cache
    ///   just returns the cached ack on hit.)
    pub async fn submit(&self, write: Write) -> Result<WriteAck, WriterError> {
        // 1. Idempotency.
        let cache = self.write_idempotency_cache();
        if let Some(cached) = cache.lookup(write.write_id) {
            return Ok((*cached).clone());
        }

        // 2-4. Open wtxn, apply each phase, commit.
        let started_at = self.now_unix_nanos_or_zero(write.started_at_unix_nanos);
        let phase_acks: Vec<PhaseAck> = {
            let mut db = self.metadata().lock();
            let wtxn = db
                .write_txn()
                .map_err(|e| WriterError::Internal(format!("write_txn: {e:?}")))?;

            let mut acks = Vec::with_capacity(write.phases.len());
            for phase in &write.phases {
                let ack = dispatch(&wtxn, phase, &write).map_err(map_apply_err)?;
                acks.push(ack);
            }

            wtxn.commit()
                .map_err(|e| WriterError::Internal(format!("commit: {e:?}")))?;
            acks
        };

        // 5. Stamp the cache.
        let committed_at = now_unix_nanos();
        let ack = WriteAck {
            write_id: write.write_id,
            committed_at_unix_nanos: committed_at,
            // WAL framing lands in P3b — until then every Write occupies
            // the synthetic LSN range [0, 0). Recovery never sees these.
            lsn_first: Lsn(0),
            lsn_last: Lsn(0),
            phase_acks,
        };
        let arc_ack = Arc::new(ack.clone());
        cache.stamp(write.write_id, arc_ack);

        let _ = started_at; // reserved for tracing / metrics in P3c

        Ok(ack)
    }

    fn now_unix_nanos_or_zero(&self, recorded: u64) -> u64 {
        if recorded != 0 {
            recorded
        } else {
            now_unix_nanos()
        }
    }
}

fn now_unix_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

/// Map [`ApplyError`] into [`WriterError`].
///
/// Storage / metadata / phase mis-shape all surface as `Internal` — the
/// writer is the boundary at which apply errors become wire errors. The
/// schema-admission and not-found variants get richer wire mappings in
/// P4 when the handler-side projection lands.
fn map_apply_err(e: ApplyError) -> WriterError {
    match e {
        ApplyError::Storage(s) => WriterError::Internal(format!("storage: {s}")),
        ApplyError::NotFound { what, detail } => {
            WriterError::Internal(format!("{what} not found: {detail}"))
        }
        ApplyError::Invariant(s) => WriterError::Internal(format!("invariant: {s}")),
        ApplyError::SchemaAdmission(s) => WriterError::Internal(format!("schema: {s}")),
        ApplyError::Metadata(s) => WriterError::Internal(format!("metadata: {s}")),
        ApplyError::PhaseMisShape(s) => WriterError::Internal(format!("phase mis-shape: {s}")),
        ApplyError::NotYetImplemented(s) => {
            WriterError::Internal(format!("apply not yet implemented: {s}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::writer::RealWriterHandle;
    use crate::write::{Phase, Write, WriteId};
    use brain_core::{AgentId, ContextId, EdgeKind, EdgeKindRef, MemoryId, MemoryKind, NodeRef};
    use brain_embed::VECTOR_DIM;
    use brain_index::{IndexParams, SharedHnsw};
    use brain_metadata::tables::edge::zero_disambiguator;
    use brain_metadata::MetadataDb;
    use brain_planner::SharedMetadataDb;
    use parking_lot::Mutex;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn build_writer() -> (TempDir, RealWriterHandle) {
        let dir = TempDir::new().unwrap();
        let db = MetadataDb::open(dir.path().join("meta.redb")).unwrap();
        let metadata: SharedMetadataDb = Arc::new(Mutex::new(db));
        let (_shared, hnsw_writer) =
            SharedHnsw::<VECTOR_DIM>::new(IndexParams::default_v1()).unwrap();
        let writer = RealWriterHandle::new(metadata, hnsw_writer);
        (dir, writer)
    }

    #[tokio::test]
    async fn submit_single_phase_link_round_trips() {
        let (_dir, writer) = build_writer();
        let phase = Phase::Link {
            from: NodeRef::Memory(MemoryId::pack(0, 1, 0)),
            to: NodeRef::Memory(MemoryId::pack(0, 2, 0)),
            kind: EdgeKindRef::Builtin(EdgeKind::SimilarTo),
            weight: 0.42,
            origin: 0,
            derived_by: 0,
            disambiguator: zero_disambiguator(),
            created_at_unix_nanos: 1_700_000_000_000,
        };
        let write = Write::single(WriteId::new(), AgentId::default(), phase);
        let ack = writer.submit(write).await.expect("submit");
        assert_eq!(ack.phase_acks.len(), 1);
        assert!(matches!(ack.single_phase(), PhaseAck::Linked));
    }

    #[tokio::test]
    async fn submit_replay_returns_cached_ack() {
        let (_dir, writer) = build_writer();
        let id = WriteId::new();
        let phase = Phase::Link {
            from: NodeRef::Memory(MemoryId::pack(0, 1, 0)),
            to: NodeRef::Memory(MemoryId::pack(0, 2, 0)),
            kind: EdgeKindRef::Builtin(EdgeKind::SimilarTo),
            weight: 0.42,
            origin: 0,
            derived_by: 0,
            disambiguator: zero_disambiguator(),
            created_at_unix_nanos: 1_700_000_000_000,
        };
        let write = Write::single(id, AgentId::default(), phase);
        let first = writer.submit(write.clone()).await.expect("first submit");
        let second = writer.submit(write).await.expect("second submit");
        assert_eq!(first.write_id, second.write_id);
        assert_eq!(
            first.committed_at_unix_nanos,
            second.committed_at_unix_nanos
        );
        assert_eq!(writer.write_idempotency_cache().len(), 1);
    }

    #[tokio::test]
    async fn submit_multi_phase_applies_all_atomically() {
        let (_dir, writer) = build_writer();
        let agent = AgentId::new();

        let upsert = Phase::UpsertMemory {
            id: MemoryId::pack(0, 1, 0),
            text: "hello".into(),
            vector: Box::new([0.0_f32; VECTOR_DIM]),
            kind: MemoryKind::Episodic,
            salience: brain_core::Salience::default(),
            context: ContextId(0),
            created_at_unix_nanos: 1_700_000_000_000,
            arena_slot: 1,
            fingerprint: [0xAA; 16],
        };
        let link = Phase::Link {
            from: NodeRef::Memory(MemoryId::pack(0, 1, 0)),
            to: NodeRef::Memory(MemoryId::pack(0, 1, 0)),
            kind: EdgeKindRef::Builtin(EdgeKind::SimilarTo),
            weight: 1.0,
            origin: 0,
            derived_by: 0,
            disambiguator: zero_disambiguator(),
            created_at_unix_nanos: 1_700_000_000_000,
        };

        let write = Write::from_phases(WriteId::new(), agent, vec![upsert, link]);
        let ack = writer.submit(write).await.expect("submit");
        assert_eq!(ack.phase_acks.len(), 2);
        assert!(matches!(ack.phase_acks[0], PhaseAck::UpsertedMemory(_)));
        assert!(matches!(ack.phase_acks[1], PhaseAck::Linked));
    }

    #[tokio::test]
    async fn submit_phase_with_no_apply_function_surfaces_internal_error() {
        let (_dir, writer) = build_writer();
        // UpsertEntity is stubbed in P2 with NotYetImplemented.
        let phase = Phase::UpsertEntity {
            id: brain_core::knowledge::EntityId::new(),
            ty: brain_core::knowledge::EntityType::PERSON_ID,
            canonical: "Alice".into(),
            normalized: "alice".into(),
            attributes: brain_core::knowledge::EntityAttributes::empty(),
            created_at_unix_nanos: 0,
        };
        let write = Write::single(WriteId::new(), AgentId::default(), phase);
        let err = writer.submit(write).await.unwrap_err();
        let WriterError::Internal(msg) = err else {
            panic!("expected Internal error");
        };
        assert!(msg.contains("not yet implemented"));
    }
}
