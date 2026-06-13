//! HyPE family — 1 table.
//!
//! - [`HYPE_QUESTION_VECTORS_TABLE`] — durable hypothetical-question
//!   embeddings, the source from which the in-RAM HyPE HNSW is rebuilt
//!   on boot.
//!
//! HyPE ("Hypothetical Prompt Embeddings") is a write-time bridge for
//! the query↔memory phrasing gap: an LLM generates several questions
//! whose answer is a memory, each is embedded, and the vectors live
//! here. A read probes the derived HNSW with the user's query vector and
//! a hit maps back to the owning memory.
//!
//! The key is `MemoryId.to_be_bytes() ++ [question_index]` so a memory
//! owns a contiguous run of rows: rebuild range-scans the whole table,
//! and a FORGET cascade can range-delete a single memory's questions by
//! its 16-byte prefix. A `u8` index caps a memory at 256 questions,
//! far above the ~5–8 generated.

use redb::TableDefinition;

/// Bytes per persisted question vector — 384 f32 components × 4 bytes,
/// pinned to BGE-small. Identical layout to the entity-vector table.
pub const HYPE_VECTOR_BYTES: usize = 384 * 4;

/// `MemoryId.to_be_bytes() ++ [question_index]` (17 bytes) →
/// little-endian `[f32; 384]` byte image. One row per generated
/// question; several rows share a memory's 16-byte prefix.
pub const HYPE_QUESTION_VECTORS_TABLE: TableDefinition<'static, [u8; 17], [u8; HYPE_VECTOR_BYTES]> =
    TableDefinition::new("hype_question_vectors");
