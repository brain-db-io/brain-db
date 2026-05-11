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

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use brain_core::{AgentId, EdgeKind, MemoryId};
use brain_index::Writer as HnswWriter;
use brain_metadata::tables::edge::{
    self, derived_by, origin, EdgeData, EDGES_IN_TABLE, EDGES_OUT_TABLE,
};
use brain_metadata::tables::idempotency::{IdempotencyEntry, IDEMPOTENCY_TABLE};
use brain_metadata::tables::memory::{MemoryMetadata, MEMORIES_TABLE};
use brain_planner::{
    EdgeOutcome, EncodeAck, EncodeOp, ForgetAck, ForgetOp, ForgetOutcome, LinkAck, LinkOp,
    SharedMetadataDb, UnlinkAck, UnlinkOp, WriterError, WriterHandle,
};
use parking_lot::Mutex;
use redb::ReadableTable;
use uuid::Uuid;

use crate::idempotency::{
    decode_encode_payload, decode_forget_payload, decode_link_payload, decode_unlink_payload,
    encode_encode_payload, encode_forget_payload, encode_link_payload, encode_unlink_payload,
    hash_encode_request, hash_forget_request, hash_link_request, hash_unlink_request,
    RESPONSE_KIND_ENCODE, RESPONSE_KIND_FORGET, RESPONSE_KIND_LINK, RESPONSE_KIND_UNLINK,
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
                let _ = wtxn.open_table(EDGES_OUT_TABLE);
                let _ = wtxn.open_table(EDGES_IN_TABLE);
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

    fn submit_link<'a>(
        &'a self,
        op: LinkOp,
    ) -> Pin<Box<dyn Future<Output = Result<LinkAck, WriterError>> + Send + 'a>> {
        Box::pin(async move { do_link(self, op) })
    }

    fn submit_unlink<'a>(
        &'a self,
        op: UnlinkOp,
    ) -> Pin<Box<dyn Future<Output = Result<UnlinkAck, WriterError>> + Send + 'a>> {
        Box::pin(async move { do_unlink(self, op) })
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

    // ── Apply: metadata row + idempotency entry + edges in ONE write txn. ─
    let response_payload = encode_encode_payload(memory_id, &edge_outcomes);
    {
        let mut db = writer.metadata.lock();
        let wtxn = db
            .write_txn()
            .map_err(|e| WriterError::Internal(format!("write_txn: {e:?}")))?;

        // First open all the tables we'll touch.
        let mut new_memory_outgoing: u32 = 0;
        let target_count_bumps: Vec<MemoryId> = op
            .edges
            .iter()
            .zip(edge_outcomes.iter())
            .filter(|(_, o)| matches!(o, EdgeOutcome::Inserted))
            .map(|(e, _)| e.target)
            .collect();

        {
            let mut edges_out_t = wtxn
                .open_table(EDGES_OUT_TABLE)
                .map_err(|e| WriterError::Internal(format!("open EDGES_OUT: {e:?}")))?;
            let mut edges_in_t = wtxn
                .open_table(EDGES_IN_TABLE)
                .map_err(|e| WriterError::Internal(format!("open EDGES_IN: {e:?}")))?;

            // Insert edges whose target exists (Inserted outcomes).
            for (edge, outcome) in op.edges.iter().zip(edge_outcomes.iter()) {
                if !matches!(outcome, EdgeOutcome::Inserted) {
                    continue;
                }
                let data = EdgeData::new(
                    edge.weight,
                    origin::EXPLICIT,
                    derived_by::CLIENT,
                    created_at,
                );
                edge::link(
                    &mut edges_out_t,
                    &mut edges_in_t,
                    memory_id,
                    edge.kind,
                    edge.target,
                    &data,
                )
                .map_err(|e| WriterError::Internal(format!("edge::link: {e:?}")))?;
                new_memory_outgoing += 1;
            }
        }

        // Bump edges_in_count on the targets.
        {
            let mut memories_t = wtxn
                .open_table(MEMORIES_TABLE)
                .map_err(|e| WriterError::Internal(format!("open MEMORIES_TABLE: {e:?}")))?;
            for target_id in &target_count_bumps {
                let key = target_id.to_be_bytes();
                let prior = memories_t
                    .get(key)
                    .map_err(|e| WriterError::Internal(format!("memories get: {e:?}")))?
                    .map(|access| access.value());
                if let Some(mut meta) = prior {
                    meta.edges_in_count = meta.edges_in_count.saturating_add(1);
                    memories_t
                        .insert(key, meta)
                        .map_err(|e| WriterError::Internal(format!("memories update: {e:?}")))?;
                }
            }

            // Insert the new memory row with the right outgoing count.
            let mut meta = MemoryMetadata::new_active(
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
            meta.edges_out_count = new_memory_outgoing;
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
        return record_and_return_forget(
            writer,
            &op,
            request_hash,
            ForgetOutcome::AlreadyTombstoned,
        );
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
        return record_and_return_forget(writer, &op, request_hash, ForgetOutcome::MemoryNotFound);
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

    record_and_return_forget(writer, &op, request_hash, ForgetOutcome::Tombstoned)
}

fn record_and_return_forget(
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

// ---------------------------------------------------------------------------
// Link
// ---------------------------------------------------------------------------

fn do_link(writer: &RealWriterHandle, op: LinkOp) -> Result<LinkAck, WriterError> {
    let request_hash = hash_link_request(&op);
    let request_id_bytes: [u8; 16] = op.request_id.into();

    // ── Idempotency lookup. ───────────────────────────────────────
    {
        let db = writer.metadata.lock();
        let rtxn = db
            .read_txn()
            .map_err(|e| WriterError::Internal(format!("link idempotency read_txn: {e:?}")))?;
        let table = rtxn
            .open_table(IDEMPOTENCY_TABLE)
            .map_err(|e| WriterError::Internal(format!("link open IDEMPOTENCY: {e:?}")))?;
        if let Some(access) = table
            .get(request_id_bytes)
            .map_err(|e| WriterError::Internal(format!("link idempotency get: {e:?}")))?
        {
            let prior = access.value();
            if prior.request_hash != request_hash {
                return Err(WriterError::Conflict(format!(
                    "link request_id={} hash mismatch",
                    hex_short(&request_id_bytes)
                )));
            }
            if prior.response_kind != RESPONSE_KIND_LINK {
                return Err(WriterError::Conflict(format!(
                    "link request_id={} kind mismatch",
                    hex_short(&request_id_bytes)
                )));
            }
            let (weight, created_at, already_existed) =
                decode_link_payload(&prior.response_payload)
                    .map_err(|e| WriterError::Internal(format!("decode link payload: {e}")))?;
            return Ok(LinkAck {
                source: op.source,
                target: op.target,
                kind: op.kind,
                weight,
                created_at_unix_nanos: created_at,
                already_existed,
                replayed: true,
            });
        }
    }

    // ── Validate both endpoints exist. ────────────────────────────
    let (source_exists, target_exists) = {
        let db = writer.metadata.lock();
        let rtxn = db
            .read_txn()
            .map_err(|e| WriterError::Internal(format!("link read_txn: {e:?}")))?;
        let table = rtxn
            .open_table(MEMORIES_TABLE)
            .map_err(|e| WriterError::Internal(format!("link open MEMORIES: {e:?}")))?;
        let s = table
            .get(op.source.to_be_bytes())
            .map_err(|e| WriterError::Internal(format!("link memory get: {e:?}")))?
            .is_some();
        let t = table
            .get(op.target.to_be_bytes())
            .map_err(|e| WriterError::Internal(format!("link memory get: {e:?}")))?
            .is_some();
        (s, t)
    };
    if !source_exists {
        return Err(WriterError::Internal(format!(
            "LINK source memory {} not found",
            op.source.raw()
        )));
    }
    if !target_exists {
        return Err(WriterError::Internal(format!(
            "LINK target memory {} not found",
            op.target.raw()
        )));
    }

    let created_at = now_unix_nanos();

    // ── Check whether the canonical (source, kind, target) already exists. ─
    let already_existed = {
        let db = writer.metadata.lock();
        let rtxn = db
            .read_txn()
            .map_err(|e| WriterError::Internal(format!("link edges read_txn: {e:?}")))?;
        let table = rtxn
            .open_table(EDGES_OUT_TABLE)
            .map_err(|e| WriterError::Internal(format!("link open EDGES_OUT: {e:?}")))?;
        let key = (
            op.source.to_be_bytes(),
            op.kind as u8,
            op.target.to_be_bytes(),
        );
        table
            .get(&key)
            .map_err(|e| WriterError::Internal(format!("link edges get: {e:?}")))?
            .is_some()
    };

    // ── Apply: edge insert + count bumps + idempotency in one txn. ─
    {
        let mut db = writer.metadata.lock();
        let wtxn = db
            .write_txn()
            .map_err(|e| WriterError::Internal(format!("link write_txn: {e:?}")))?;
        {
            let mut edges_out_t = wtxn
                .open_table(EDGES_OUT_TABLE)
                .map_err(|e| WriterError::Internal(format!("link open EDGES_OUT: {e:?}")))?;
            let mut edges_in_t = wtxn
                .open_table(EDGES_IN_TABLE)
                .map_err(|e| WriterError::Internal(format!("link open EDGES_IN: {e:?}")))?;
            let data = EdgeData::new(op.weight, origin::EXPLICIT, derived_by::CLIENT, created_at);
            edge::link(
                &mut edges_out_t,
                &mut edges_in_t,
                op.source,
                op.kind,
                op.target,
                &data,
            )
            .map_err(|e| WriterError::Internal(format!("edge::link: {e:?}")))?;
        }
        if !already_existed {
            // Bump counts on both endpoints.
            let mut memories_t = wtxn
                .open_table(MEMORIES_TABLE)
                .map_err(|e| WriterError::Internal(format!("link open MEMORIES: {e:?}")))?;
            bump_edge_count(&mut memories_t, op.source, /* out */ true, 1)?;
            bump_edge_count(&mut memories_t, op.target, /* out */ false, 1)?;
        }
        {
            let mut idem_t = wtxn
                .open_table(IDEMPOTENCY_TABLE)
                .map_err(|e| WriterError::Internal(format!("link open IDEMPOTENCY: {e:?}")))?;
            let payload = encode_link_payload(op.weight, created_at, already_existed);
            let entry =
                IdempotencyEntry::new(RESPONSE_KIND_LINK, None, payload, request_hash, created_at);
            idem_t
                .insert(request_id_bytes, entry)
                .map_err(|e| WriterError::Internal(format!("link idempotency insert: {e:?}")))?;
        }
        wtxn.commit()
            .map_err(|e| WriterError::Internal(format!("link commit: {e:?}")))?;
    }

    Ok(LinkAck {
        source: op.source,
        target: op.target,
        kind: op.kind,
        weight: op.weight,
        created_at_unix_nanos: created_at,
        already_existed,
        replayed: false,
    })
}

// ---------------------------------------------------------------------------
// Unlink
// ---------------------------------------------------------------------------

fn do_unlink(writer: &RealWriterHandle, op: UnlinkOp) -> Result<UnlinkAck, WriterError> {
    let request_hash = hash_unlink_request(&op);
    let request_id_bytes: [u8; 16] = op.request_id.into();

    // ── Idempotency lookup. ───────────────────────────────────────
    {
        let db = writer.metadata.lock();
        let rtxn = db
            .read_txn()
            .map_err(|e| WriterError::Internal(format!("unlink idempotency read_txn: {e:?}")))?;
        let table = rtxn
            .open_table(IDEMPOTENCY_TABLE)
            .map_err(|e| WriterError::Internal(format!("unlink open IDEMPOTENCY: {e:?}")))?;
        if let Some(access) = table
            .get(request_id_bytes)
            .map_err(|e| WriterError::Internal(format!("unlink idempotency get: {e:?}")))?
        {
            let prior = access.value();
            if prior.request_hash != request_hash {
                return Err(WriterError::Conflict(format!(
                    "unlink request_id={} hash mismatch",
                    hex_short(&request_id_bytes)
                )));
            }
            if prior.response_kind != RESPONSE_KIND_UNLINK {
                return Err(WriterError::Conflict(format!(
                    "unlink request_id={} kind mismatch",
                    hex_short(&request_id_bytes)
                )));
            }
            let removed = decode_unlink_payload(&prior.response_payload)
                .map_err(|e| WriterError::Internal(format!("decode unlink payload: {e}")))?;
            return Ok(UnlinkAck {
                source: op.source,
                target: op.target,
                kind: op.kind,
                removed,
                replayed: true,
            });
        }
    }

    let created_at = now_unix_nanos();

    // ── Apply: edge remove + count decrement + idempotency. ───────
    let removed = {
        let mut db = writer.metadata.lock();
        let wtxn = db
            .write_txn()
            .map_err(|e| WriterError::Internal(format!("unlink write_txn: {e:?}")))?;
        let removed = {
            let mut edges_out_t = wtxn
                .open_table(EDGES_OUT_TABLE)
                .map_err(|e| WriterError::Internal(format!("unlink open EDGES_OUT: {e:?}")))?;
            let mut edges_in_t = wtxn
                .open_table(EDGES_IN_TABLE)
                .map_err(|e| WriterError::Internal(format!("unlink open EDGES_IN: {e:?}")))?;
            edge::unlink(
                &mut edges_out_t,
                &mut edges_in_t,
                op.source,
                op.kind,
                op.target,
            )
            .map_err(|e| WriterError::Internal(format!("edge::unlink: {e:?}")))?
        };
        if removed {
            let mut memories_t = wtxn
                .open_table(MEMORIES_TABLE)
                .map_err(|e| WriterError::Internal(format!("unlink open MEMORIES: {e:?}")))?;
            bump_edge_count(&mut memories_t, op.source, /* out */ true, -1)?;
            bump_edge_count(&mut memories_t, op.target, /* out */ false, -1)?;
        }
        {
            let mut idem_t = wtxn
                .open_table(IDEMPOTENCY_TABLE)
                .map_err(|e| WriterError::Internal(format!("unlink open IDEMPOTENCY: {e:?}")))?;
            let payload = encode_unlink_payload(removed);
            let entry = IdempotencyEntry::new(
                RESPONSE_KIND_UNLINK,
                None,
                payload,
                request_hash,
                created_at,
            );
            idem_t
                .insert(request_id_bytes, entry)
                .map_err(|e| WriterError::Internal(format!("unlink idempotency insert: {e:?}")))?;
        }
        wtxn.commit()
            .map_err(|e| WriterError::Internal(format!("unlink commit: {e:?}")))?;
        removed
    };

    Ok(UnlinkAck {
        source: op.source,
        target: op.target,
        kind: op.kind,
        removed,
        replayed: false,
    })
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// Bump `edges_out_count` (or `edges_in_count`) on `memory_id` by
/// `delta`. No-op if the memory row doesn't exist (the LINK / UNLINK
/// paths validate existence separately; this is defensive).
fn bump_edge_count(
    memories_t: &mut redb::Table<'_, [u8; 16], MemoryMetadata>,
    memory_id: MemoryId,
    out: bool,
    delta: i32,
) -> Result<(), WriterError> {
    let key = memory_id.to_be_bytes();
    let prior = memories_t
        .get(key)
        .map_err(|e| WriterError::Internal(format!("bump_edge_count get: {e:?}")))?
        .map(|access| access.value());
    let Some(mut meta) = prior else {
        return Ok(());
    };
    let cur = if out {
        meta.edges_out_count
    } else {
        meta.edges_in_count
    };
    let new = if delta >= 0 {
        cur.saturating_add(delta as u32)
    } else {
        cur.saturating_sub((-delta) as u32)
    };
    if out {
        meta.edges_out_count = new;
    } else {
        meta.edges_in_count = new;
    }
    memories_t
        .insert(key, meta)
        .map_err(|e| WriterError::Internal(format!("bump_edge_count insert: {e:?}")))?;
    Ok(())
}

fn hex_short(bytes: &[u8; 16]) -> String {
    let mut s = String::with_capacity(8);
    for b in &bytes[..4] {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// Silence unused-import warnings when EdgeKind is referenced only
// inside conditional code paths.
#[allow(dead_code)]
fn _kind_use(_: EdgeKind) {}
