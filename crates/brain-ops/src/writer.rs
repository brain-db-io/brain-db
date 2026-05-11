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

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use brain_core::{AgentId, MemoryId};
use brain_index::Writer as HnswWriter;
use brain_metadata::tables::idempotency::{IdempotencyEntry, IDEMPOTENCY_TABLE};
use brain_metadata::tables::memory::{MemoryMetadata, MEMORIES_TABLE};
use brain_planner::{
    EdgeOutcome, EncodeAck, EncodeOp, ForgetAck, ForgetOp, ForgetOutcome, SharedMetadataDb,
    WriterError, WriterHandle,
};
use parking_lot::Mutex;
use uuid::Uuid;

use crate::idempotency::{
    decode_encode_payload, decode_forget_payload, encode_encode_payload, encode_forget_payload,
    hash_encode_request, hash_forget_request, RESPONSE_KIND_ENCODE, RESPONSE_KIND_FORGET,
};

/// Real per-shard writer backed by `MetadataDb` + `HnswWriter`. No
/// WAL — Phase 8 / 9 swap this for a WAL-backed implementation
/// without changing `WriterHandle`'s public surface.
pub struct RealWriterHandle {
    metadata: SharedMetadataDb,
    hnsw_writer: Mutex<HnswWriter<384>>,
    /// In-process slot counter. Phase 8 / 9 will replace with the
    /// arena allocator. Starts at 1.
    next_slot: AtomicU64,
    /// Memories we've tombstoned this process-lifetime. Used to
    /// surface `AlreadyTombstoned` per spec §08/06 §10 when a
    /// **different** RequestId targets a previously-tombstoned id.
    /// (Same-RequestId replay is caught by the idempotency table.)
    tombstoned: Mutex<HashSet<MemoryId>>,
    /// Agent id stamped on every memory metadata row. Phase 9 will
    /// derive this from the authenticated connection; for now it's
    /// nil. Carried as a field so tests + the future server can pin
    /// it without re-creating the writer.
    agent_id: AgentId,
}

impl RealWriterHandle {
    #[must_use]
    pub fn new(metadata: SharedMetadataDb, hnsw_writer: HnswWriter<384>) -> Self {
        // Materialise the tables we read from. redb creates tables
        // on first write_txn().open_table(), but read_txn() on a
        // never-opened table returns `TableDoesNotExist`. We do a
        // one-time empty write txn at construction so subsequent
        // idempotency + metadata reads succeed even before the
        // first submit.
        {
            let mut db = metadata.lock();
            if let Ok(wtxn) = db.write_txn() {
                let _ = wtxn.open_table(MEMORIES_TABLE);
                let _ = wtxn.open_table(IDEMPOTENCY_TABLE);
                let _ = wtxn.commit();
            }
        }
        Self {
            metadata,
            hnsw_writer: Mutex::new(hnsw_writer),
            next_slot: AtomicU64::new(1),
            tombstoned: Mutex::new(HashSet::new()),
            agent_id: AgentId(Uuid::nil()),
        }
    }

    #[must_use]
    pub fn with_agent_id(mut self, agent_id: AgentId) -> Self {
        self.agent_id = agent_id;
        self
    }
}

const _: fn() = || {
    fn require<T: Send + Sync>() {}
    require::<RealWriterHandle>();
};

fn now_unix_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

impl WriterHandle for RealWriterHandle {
    fn submit_encode<'a>(
        &'a self,
        op: EncodeOp,
    ) -> Pin<Box<dyn Future<Output = Result<EncodeAck, WriterError>> + Send + 'a>> {
        Box::pin(async move { do_encode(self, op) })
    }

    fn submit_forget<'a>(
        &'a self,
        op: ForgetOp,
    ) -> Pin<Box<dyn Future<Output = Result<ForgetAck, WriterError>> + Send + 'a>> {
        Box::pin(async move { do_forget(self, op) })
    }
}

// ---------------------------------------------------------------------------
// Encode
// ---------------------------------------------------------------------------

fn do_encode(writer: &RealWriterHandle, op: EncodeOp) -> Result<EncodeAck, WriterError> {
    let request_hash = hash_encode_request(&op);
    let request_id_bytes: [u8; 16] = op.request_id.into();

    // ── Idempotency lookup (read txn). ────────────────────────────
    {
        let db = writer.metadata.lock();
        let rtxn = db
            .read_txn()
            .map_err(|e| WriterError::Internal(format!("idempotency read_txn: {e:?}")))?;
        let table = rtxn
            .open_table(IDEMPOTENCY_TABLE)
            .map_err(|e| WriterError::Internal(format!("open IDEMPOTENCY_TABLE: {e:?}")))?;
        if let Some(access) = table
            .get(request_id_bytes)
            .map_err(|e| WriterError::Internal(format!("idempotency get: {e:?}")))?
        {
            let prior = access.value();
            if prior.request_hash != request_hash {
                return Err(WriterError::Conflict(format!(
                    "encode request_id={} hash mismatch",
                    hex_short(&request_id_bytes)
                )));
            }
            if prior.response_kind != RESPONSE_KIND_ENCODE {
                return Err(WriterError::Conflict(format!(
                    "encode request_id={} kind mismatch",
                    hex_short(&request_id_bytes)
                )));
            }
            let (memory_id, edge_outcomes) = decode_encode_payload(&prior.response_payload)
                .map_err(|e| WriterError::Internal(format!("decode encode payload: {e}")))?;
            return Ok(EncodeAck {
                memory_id,
                edge_results: edge_outcomes,
                replayed: true,
            });
        }
    }

    // ── Mint slot + MemoryId. ─────────────────────────────────────
    let slot = writer.next_slot.fetch_add(1, Ordering::Relaxed);
    let memory_id = MemoryId::pack(/* shard */ 0, slot, /* version */ 1);
    let created_at = now_unix_nanos();

    // ── Compute edge outcomes against existing memories. ──────────
    // (Read txn before the write txn; minimises lock duration.)
    let edge_outcomes: Vec<EdgeOutcome> = {
        let db = writer.metadata.lock();
        let rtxn = db
            .read_txn()
            .map_err(|e| WriterError::Internal(format!("edges read_txn: {e:?}")))?;
        let table = rtxn
            .open_table(MEMORIES_TABLE)
            .map_err(|e| WriterError::Internal(format!("edges open_table: {e:?}")))?;
        op.edges
            .iter()
            .map(|edge| {
                let exists = table
                    .get(edge.target.to_be_bytes())
                    .ok()
                    .flatten()
                    .is_some();
                if exists {
                    EdgeOutcome::Inserted
                } else {
                    EdgeOutcome::TargetMissing
                }
            })
            .collect()
    };

    // ── Apply: metadata row + idempotency entry in ONE write txn. ─
    let response_payload = encode_encode_payload(memory_id, &edge_outcomes);
    {
        let mut db = writer.metadata.lock();
        let wtxn = db
            .write_txn()
            .map_err(|e| WriterError::Internal(format!("write_txn: {e:?}")))?;
        {
            let mut memories_t = wtxn
                .open_table(MEMORIES_TABLE)
                .map_err(|e| WriterError::Internal(format!("open MEMORIES_TABLE: {e:?}")))?;
            let meta = MemoryMetadata::new_active(
                memory_id,
                writer.agent_id,
                op.context_id,
                slot,
                /* slot_version */ 1,
                op.kind,
                op.fingerprint,
                op.salience_initial,
                /* text_size */ op.text.len() as u32,
                created_at,
            );
            memories_t
                .insert(memory_id.to_be_bytes(), meta)
                .map_err(|e| WriterError::Internal(format!("memories insert: {e:?}")))?;
        }
        {
            let mut idem_t = wtxn
                .open_table(IDEMPOTENCY_TABLE)
                .map_err(|e| WriterError::Internal(format!("open IDEMPOTENCY_TABLE: {e:?}")))?;
            let entry = IdempotencyEntry::new(
                RESPONSE_KIND_ENCODE,
                Some(memory_id.to_be_bytes()),
                response_payload,
                request_hash,
                created_at,
            );
            idem_t
                .insert(request_id_bytes, entry)
                .map_err(|e| WriterError::Internal(format!("idempotency insert: {e:?}")))?;
        }
        wtxn.commit()
            .map_err(|e| WriterError::Internal(format!("commit: {e:?}")))?;
    }

    // ── HNSW insert (after the durability barrier). ───────────────
    writer
        .hnsw_writer
        .lock()
        .insert(memory_id, &op.vector)
        .map_err(|e| WriterError::Internal(format!("hnsw insert: {e:?}")))?;

    Ok(EncodeAck {
        memory_id,
        edge_results: edge_outcomes,
        replayed: false,
    })
}

// ---------------------------------------------------------------------------
// Forget
// ---------------------------------------------------------------------------

fn do_forget(writer: &RealWriterHandle, op: ForgetOp) -> Result<ForgetAck, WriterError> {
    let request_hash = hash_forget_request(&op);
    let request_id_bytes: [u8; 16] = op.request_id.into();

    // ── Idempotency lookup. ───────────────────────────────────────
    {
        let db = writer.metadata.lock();
        let rtxn = db
            .read_txn()
            .map_err(|e| WriterError::Internal(format!("idempotency read_txn: {e:?}")))?;
        let table = rtxn
            .open_table(IDEMPOTENCY_TABLE)
            .map_err(|e| WriterError::Internal(format!("open IDEMPOTENCY_TABLE: {e:?}")))?;
        if let Some(access) = table
            .get(request_id_bytes)
            .map_err(|e| WriterError::Internal(format!("idempotency get: {e:?}")))?
        {
            let prior = access.value();
            if prior.request_hash != request_hash {
                return Err(WriterError::Conflict(format!(
                    "forget request_id={} hash mismatch",
                    hex_short(&request_id_bytes)
                )));
            }
            if prior.response_kind != RESPONSE_KIND_FORGET {
                return Err(WriterError::Conflict(format!(
                    "forget request_id={} kind mismatch",
                    hex_short(&request_id_bytes)
                )));
            }
            let (memory_id, outcome) = decode_forget_payload(&prior.response_payload)
                .map_err(|e| WriterError::Internal(format!("decode forget payload: {e}")))?;
            return Ok(ForgetAck {
                memory_id,
                outcome,
                replayed: true,
            });
        }
    }

    // ── Already tombstoned (this process-lifetime)? ──────────────
    let already = writer.tombstoned.lock().contains(&op.memory_id);
    if already {
        return record_and_return(writer, &op, request_hash, ForgetOutcome::AlreadyTombstoned);
    }

    // ── Does the memory exist? ───────────────────────────────────
    let exists = {
        let db = writer.metadata.lock();
        let rtxn = db
            .read_txn()
            .map_err(|e| WriterError::Internal(format!("memory read_txn: {e:?}")))?;
        let table = rtxn
            .open_table(MEMORIES_TABLE)
            .map_err(|e| WriterError::Internal(format!("memory open_table: {e:?}")))?;
        table
            .get(op.memory_id.to_be_bytes())
            .ok()
            .flatten()
            .is_some()
    };
    if !exists {
        return record_and_return(writer, &op, request_hash, ForgetOutcome::MemoryNotFound);
    }

    // ── Tombstone in HNSW (durable in the sense it survives this
    //    process's lifetime; Phase 8/9 wires the WAL-recoverable
    //    flag). ─────────────────────────────────────────────────
    writer
        .hnsw_writer
        .lock()
        .mark_tombstoned(op.memory_id)
        .map_err(|e| WriterError::Internal(format!("mark_tombstoned: {e:?}")))?;
    writer.tombstoned.lock().insert(op.memory_id);

    record_and_return(writer, &op, request_hash, ForgetOutcome::Tombstoned)
}

/// Helper that writes the idempotency entry for a FORGET and returns
/// the matching ack. Each return path from `do_forget` goes through
/// this function so the idempotency table is always populated.
fn record_and_return(
    writer: &RealWriterHandle,
    op: &ForgetOp,
    request_hash: [u8; 32],
    outcome: ForgetOutcome,
) -> Result<ForgetAck, WriterError> {
    let request_id_bytes: [u8; 16] = op.request_id.into();
    let created_at = now_unix_nanos();
    let payload = encode_forget_payload(op.memory_id, outcome);
    {
        let mut db = writer.metadata.lock();
        let wtxn = db
            .write_txn()
            .map_err(|e| WriterError::Internal(format!("forget write_txn: {e:?}")))?;
        {
            let mut idem_t = wtxn
                .open_table(IDEMPOTENCY_TABLE)
                .map_err(|e| WriterError::Internal(format!("forget open idempotency: {e:?}")))?;
            let entry = IdempotencyEntry::new(
                RESPONSE_KIND_FORGET,
                Some(op.memory_id.to_be_bytes()),
                payload,
                request_hash,
                created_at,
            );
            idem_t
                .insert(request_id_bytes, entry)
                .map_err(|e| WriterError::Internal(format!("forget idempotency insert: {e:?}")))?;
        }
        wtxn.commit()
            .map_err(|e| WriterError::Internal(format!("forget commit: {e:?}")))?;
    }
    Ok(ForgetAck {
        memory_id: op.memory_id,
        outcome,
        replayed: false,
    })
}

fn hex_short(bytes: &[u8; 16]) -> String {
    let mut s = String::with_capacity(8);
    for b in &bytes[..4] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}
