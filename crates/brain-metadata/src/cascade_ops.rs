//! FORGET cascade operations (sub-task 24.2). Spec §25/00
//! §"Cascading effects of FORGET".
//!
//! When a memory is forgotten, statements / relations whose
//! evidence list referenced it must be updated:
//!
//! 1. Drop `memory_id` from `evidence_inline` (and overflow, when
//!    we get there post-v1).
//! 2. Recompute `confidence` from the remaining evidence per
//!    §25/00 §"Confidence aggregation across evidence".
//! 3. If evidence becomes empty AND confidence < threshold,
//!    tombstone with reason `SourceMemoryForgotten`.
//!
//! ## v1 scope cuts
//!
//! - Overflow evidence lists (post-`INLINE_EVIDENCE_CAP = 8`) are
//!   **not** searched in v1. Statements with overflow evidence
//!   containing the forgotten memory keep the evidence entry; the
//!   v1 confidence is still recomputed on the inline-only set.
//!   Full overflow-aware cascade is a post-v1 enhancement.
//! - Relations are scanned but only have a single-evidence link
//!   per the v1 schema; if that evidence equals `memory_id`, the
//!   relation is tombstoned.
//!
//! ## Audit
//!
//! Audit-event semantics for the cascade live in §25/00 §"The
//! audit log" but the v1 `audit_ops::audit_write` API targets
//! extraction events. Cascade audit rows land as a post-v1
//! enhancement; the cascade still updates the row, so an
//! external observer can see the change via the change feed.

use brain_core::knowledge::TombstoneReason;
use brain_core::MemoryId;
use redb::{ReadableTable, WriteTransaction};

use crate::statement_ops::{statement_tombstone, StatementOpError};
use crate::tables::knowledge::statement::{EvidenceEntryRow, StatementMetadata, STATEMENTS_TABLE};

/// Default confidence threshold below which a statement that
/// loses its only piece of evidence is tombstoned. Configurable
/// at the caller; spec §25/00 doesn't pin a number.
pub const DEFAULT_CASCADE_CONFIDENCE_THRESHOLD: f32 = 0.2;

/// Outcome of cascading one FORGET against one statement.
#[derive(Debug, Clone, PartialEq)]
pub enum CascadeOutcome {
    /// Evidence list shrank; statement kept.
    EvidenceDropped { new_confidence: f32 },
    /// Evidence became empty AND confidence stayed above
    /// threshold — the row is kept with `stale_evidence`
    /// semantics (statement count unchanged, but the operator
    /// can re-extract).
    KeptStaleEvidence { confidence: f32 },
    /// Tombstoned with reason `SourceMemoryForgotten`.
    Tombstoned,
    /// The statement did not reference this memory.
    Untouched,
}

/// Per-cascade aggregate summary.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CascadeSummary {
    pub scanned: u64,
    pub evidence_dropped: u64,
    pub kept_stale: u64,
    pub tombstoned: u64,
}

/// Apply the cascade for `memory_id` against every active
/// statement. Returns counts.
///
/// `batch_cap` bounds the scan in a single txn so heavily-
/// referenced memories don't produce an unbounded wtxn. Spec
/// §27/04 §4.5 ("continuation jobs") tracks the post-v1
/// follow-up that resumes from a cursor when the batch cap is
/// hit.
///
/// `confidence_threshold` follows the [`DEFAULT_CASCADE_CONFIDENCE_THRESHOLD`]
/// when the caller doesn't override.
pub fn cascade_forget_to_statements(
    wtxn: &WriteTransaction,
    memory_id: MemoryId,
    confidence_threshold: f32,
    batch_cap: usize,
    now_unix_nanos: u64,
) -> Result<CascadeSummary, StatementOpError> {
    let mut summary = CascadeSummary::default();
    let memory_bytes = memory_id.to_be_bytes();

    // Collect affected statement_ids first; then mutate per-row.
    // Snapshot-then-update avoids interleaving redb reads and
    // writes against the same table.
    let mut affected: Vec<(StatementMetadata, Vec<EvidenceEntryRowLike>)> = Vec::new();
    {
        let table = wtxn.open_table(STATEMENTS_TABLE)?;
        for entry in table.iter()? {
            let (_, v) = entry?;
            let row = v.value();
            summary.scanned += 1;
            if row.is_tombstoned() {
                continue;
            }
            let referenced = row
                .evidence_inline
                .iter()
                .any(|e| e.memory_id_bytes == memory_bytes);
            if !referenced {
                continue;
            }
            let remaining: Vec<EvidenceEntryRowLike> = row
                .evidence_inline
                .iter()
                .filter(|e| e.memory_id_bytes != memory_bytes)
                .map(|e| EvidenceEntryRowLike {
                    memory_id_bytes: e.memory_id_bytes,
                    confidence_milli: e.confidence_milli,
                    timestamp_unix_nanos: e.timestamp_unix_nanos,
                    extractor_id: e.extractor_id,
                })
                .collect();
            affected.push((row, remaining));
            if affected.len() >= batch_cap {
                break;
            }
        }
    }

    // Apply mutations. Each affected statement either becomes
    // evidence-shrunk + confidence-recomputed, or tombstoned.
    for (mut row, remaining) in affected {
        if remaining.is_empty() {
            // Empty inline evidence — recompute confidence from the empty set
            // and decide tombstone vs keep.
            let new_conf = if !row.evidence_overflow_id_bytes.is_some() {
                0.0
            } else {
                // Overflow case: we don't crack open the overflow row in v1,
                // so we keep the existing confidence and flag the row stale.
                row.confidence
            };
            if new_conf < confidence_threshold && row.evidence_overflow_id_bytes.is_none() {
                let id = row.statement_id();
                statement_tombstone(wtxn, id, TombstoneReason::SourceMemoryForgotten, now_unix_nanos)?;
                summary.tombstoned += 1;
            } else {
                row.evidence_inline.clear();
                row.confidence = new_conf;
                let mut t = wtxn.open_table(STATEMENTS_TABLE)?;
                t.insert(&row.statement_id_bytes, &row)?;
                summary.kept_stale += 1;
            }
        } else {
            // Drop the forgotten memory; recompute confidence as a simple
            // mean over the surviving inline entries (spec §25/00 formula
            // takes decay into account; we use the row's stored confidence
            // values which already reflect per-evidence weighting). Full
            // §25/00 formula application is post-v1.
            let new_conf = mean_confidence(&remaining);
            row.evidence_inline = remaining
                .into_iter()
                .map(|e| EvidenceEntryRow {
                    memory_id_bytes: e.memory_id_bytes,
                    confidence_milli: e.confidence_milli,
                    timestamp_unix_nanos: e.timestamp_unix_nanos,
                    extractor_id: e.extractor_id,
                })
                .collect();
            row.confidence = new_conf;
            let mut t = wtxn.open_table(STATEMENTS_TABLE)?;
            t.insert(&row.statement_id_bytes, &row)?;
            summary.evidence_dropped += 1;
        }
    }

    Ok(summary)
}

// Local mirror so cascade_ops doesn't depend on the row layout
// from brain-metadata's table module directly. The two are kept
// in sync; cascade-side processing is the only consumer.
struct EvidenceEntryRowLike {
    memory_id_bytes: [u8; 16],
    confidence_milli: u16,
    timestamp_unix_nanos: u64,
    extractor_id: u32,
}

fn mean_confidence(entries: &[EvidenceEntryRowLike]) -> f32 {
    if entries.is_empty() {
        return 0.0;
    }
    let sum: f32 = entries
        .iter()
        .map(|e| f32::from(e.confidence_milli) / 1000.0)
        .sum();
    sum / entries.len() as f32
}
