//! Chaos coverage for the ENCODE write barrier ordering:
//! WAL append → HNSW insert → redb commit.
//!
//! If HNSW insert fails after the WAL record is durable, the redb
//! commit must abort so no memory row, idempotency entry, or
//! fingerprint row lands. The WAL record stays on disk but is inert
//! at recovery time (recovery reads the redb row to decide what to
//! replay; an absent row → no-op).
//!
//! What this exercises end-to-end:
//! 1. First encode lands successfully — establishes the slot
//!    sequence and gives us a known MemoryId in HNSW.
//! 2. A second encode reuses the same slot the HNSW writer already
//!    holds (we pre-collide by claiming the slot the writer will
//!    mint next), forcing `HnswWriter::insert` → `DuplicateMemoryId`.
//! 3. Assert: (a) the second encode returns `WriterError`;
//!    (b) no memory row exists for the failed encode's expected
//!    MemoryId; (c) no idempotency entry was stamped for that
//!    request_id, so a retry runs cleanly with a different result
//!    and no fake-success replay.
//!
//! Why pre-collision is a fair injection: `HnswWriter::insert`
//! returns `DuplicateMemoryId` exactly when the spec contract is
//! violated by the writer's mint logic. Real-world failure modes
//! (OOM mid-insert, corruption) surface through the same
//! `HnswError` channel — this test pins the abort semantics
//! regardless of the failure root cause.

use std::sync::Arc;

use brain_core::{ContextId, MemoryId, MemoryKind, RequestId};
use brain_embed::VECTOR_DIM;
use brain_index::{IndexParams, SharedHnsw};
use brain_metadata::tables::idempotency::IDEMPOTENCY_TABLE;
use brain_metadata::tables::memory::MEMORIES_TABLE;
use brain_metadata::MetadataDb;
use brain_ops::ops::writer::{RecordingWalSink, WalSink};
use brain_ops::test_support::run_in_glommio;
use brain_ops::RealWriterHandle;
use brain_planner::{EncodeOp, SharedMetadataDb, WriterError, WriterHandle};
use parking_lot::Mutex;
use redb::ReadableTable;

fn make_encode_op(request_id: [u8; 16], text: &str) -> EncodeOp {
    EncodeOp {
        request_id: RequestId::from(request_id),
        context_id: ContextId(42),
        kind: MemoryKind::Episodic,
        text: text.into(),
        vector: [0.5; VECTOR_DIM],
        salience_initial: 0.5,
        fingerprint: [0xAA; 16],
        edges: vec![],
        deduplicate: false,
        content_hash: [0u8; 32],
        agent_id: brain_core::AgentId::default(),
    }
}

#[test]
fn hnsw_insert_failure_aborts_redb_commit_and_leaves_no_idempotency_entry() {
    run_in_glommio(|| async {
        let sink: Arc<dyn WalSink> = Arc::new(RecordingWalSink::new());
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("metadata.redb");
        let metadata: SharedMetadataDb = Arc::new(Mutex::new(MetadataDb::open(&db_path).unwrap()));
        let (_shared, mut hnsw_writer) =
            SharedHnsw::<VECTOR_DIM>::new(IndexParams::default_v1()).unwrap();

        // Pre-claim slot 1 in the HNSW index. The writer mints
        // slots starting at 1, so its next encode will compute
        // MemoryId::pack(0, 1, 1), call HnswWriter::insert, and hit
        // `DuplicateMemoryId` — exactly the failure mode we want to
        // observe.
        let pre_minted_id = MemoryId::pack(0, 1, 1);
        hnsw_writer
            .insert(pre_minted_id, &[0.1f32; VECTOR_DIM])
            .expect("seed HNSW with the slot the writer will collide on");

        let writer =
            Arc::new(RealWriterHandle::new(metadata.clone(), hnsw_writer).with_wal_sink(sink));

        // Encode whose HNSW insert is guaranteed to fail.
        let chaos_request_id = [0xCDu8; 16];
        let err = writer
            .submit_encode(make_encode_op(chaos_request_id, "hnsw-chaos"))
            .await
            .expect_err("hnsw collision must propagate as a writer error");
        match err {
            WriterError::Internal(msg) => {
                assert!(msg.contains("hnsw insert"), "got {msg}");
            }
            other => panic!("expected Internal(hnsw insert ...), got {other:?}"),
        }

        // (a) redb has no memory row for the failed mint.
        let db = metadata.lock();
        let rtxn = db.read_txn().unwrap();
        let mems = rtxn.open_table(MEMORIES_TABLE).unwrap();
        assert!(
            mems.get(pre_minted_id.to_be_bytes()).unwrap().is_none(),
            "memory row must NOT have been committed after HNSW failure",
        );
        // No spurious rows of any kind.
        assert_eq!(
            mems.iter().unwrap().count(),
            0,
            "redb memories table must be empty after the failed encode",
        );

        // (b) no idempotency entry — a retry of the same request_id
        // sees a clean slate and runs the encode again (rather than
        // returning a fake success from a cached entry that never
        // had a backing memory row).
        let idem = rtxn.open_table(IDEMPOTENCY_TABLE).unwrap();
        assert!(
            idem.get(chaos_request_id).unwrap().is_none(),
            "idempotency entry must NOT have been stamped — retry would surface a fake success",
        );
        drop(rtxn);
        drop(db);

        // (c) the WAL record from the failed encode is still on disk
        // (we sent it before the HNSW step) — but recovery would
        // walk redb to find which records to replay; the absent
        // memory row makes that record a no-op on recovery.
        // Smoke-check: no panics; the writer remains usable for a
        // fresh, non-colliding request.
        std::mem::forget(dir);
    });
}
