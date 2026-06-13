//! Typed CRUD over the HyPE question-vector table.
//!
//! Free functions over [`redb::ReadTransaction`] /
//! [`redb::WriteTransaction`], composed inside the caller's own
//! transaction (the extractor worker writes question vectors in the same
//! txn as the rest of its extraction output). Callers commit themselves.

use brain_core::MemoryId;
use redb::{ReadTransaction, ReadableTable, WriteTransaction};

use crate::tables::hype::{HYPE_QUESTION_VECTORS_TABLE, HYPE_VECTOR_BYTES};

/// Errors from the HyPE CRUD layer.
#[derive(thiserror::Error, Debug)]
pub enum HypeOpError {
    #[error("redb storage error: {0}")]
    Storage(#[from] redb::StorageError),

    #[error("redb table error: {0}")]
    Table(#[from] redb::TableError),
}

/// Little-endian byte image of a question vector. Mirrors
/// `entity::ops::vector_to_bytes`; the table's fixed value size keeps the
/// dimensionality honest.
fn vector_to_bytes(vector: &[f32; 384]) -> [u8; HYPE_VECTOR_BYTES] {
    let mut out = [0u8; HYPE_VECTOR_BYTES];
    for (i, v) in vector.iter().enumerate() {
        out[i * 4..(i + 1) * 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// Inverse of [`vector_to_bytes`].
fn bytes_to_vector(bytes: &[u8; HYPE_VECTOR_BYTES]) -> [f32; 384] {
    let mut out = [0.0f32; 384];
    for (i, slot) in out.iter_mut().enumerate() {
        let chunk: [u8; 4] = bytes[i * 4..(i + 1) * 4]
            .try_into()
            .expect("invariant: fixed slice");
        *slot = f32::from_le_bytes(chunk);
    }
    out
}

/// Build the 17-byte row key: `memory_id (16) ++ question_index (1)`.
fn row_key(memory_id: MemoryId, question_index: u8) -> [u8; 17] {
    let mut key = [0u8; 17];
    key[..16].copy_from_slice(&memory_id.to_be_bytes());
    key[16] = question_index;
    key
}

/// Persist one question vector for `memory_id` at slot `question_index`.
/// Idempotent on the (memory_id, index) key, so re-running extraction for
/// the same memory overwrites rather than duplicating.
pub fn hype_vector_put(
    wtxn: &WriteTransaction,
    memory_id: MemoryId,
    question_index: u8,
    vector: &[f32; 384],
) -> Result<(), HypeOpError> {
    let bytes = vector_to_bytes(vector);
    let mut t = wtxn.open_table(HYPE_QUESTION_VECTORS_TABLE)?;
    t.insert(&row_key(memory_id, question_index), &bytes)?;
    Ok(())
}

/// Delete every question vector owned by `memory_id` (FORGET cascade).
/// Returns the number of rows removed. No-op if the memory owns none.
pub fn hype_vectors_delete_memory(
    wtxn: &WriteTransaction,
    memory_id: MemoryId,
) -> Result<usize, HypeOpError> {
    let prefix = memory_id.to_be_bytes();
    let lo = row_key(memory_id, 0);
    let hi = row_key(memory_id, u8::MAX);
    let mut t = wtxn.open_table(HYPE_QUESTION_VECTORS_TABLE)?;
    // Collect keys first (the range borrow can't coexist with remove).
    let mut keys: Vec<[u8; 17]> = Vec::new();
    for entry in t.range(lo..=hi)? {
        let (k, _) = entry?;
        let key = k.value();
        if key[..16] == prefix {
            keys.push(key);
        }
    }
    let removed = keys.len();
    for key in keys {
        t.remove(&key)?;
    }
    Ok(removed)
}

/// One row yielded by [`hype_iter_all_vectors`]: `(MemoryId, vector)`.
/// Several rows may share a `MemoryId` (one per generated question).
pub type HypeRebuildRow = (MemoryId, [f32; 384]);

/// Iterate every stored question vector, returning `(MemoryId, vector)`
/// pairs in key order. The boot rebuild feeds these straight into a fresh
/// `HypeHnswIndex` — no LLM, no embedder.
pub fn hype_iter_all_vectors(
    rtxn: &ReadTransaction,
) -> Result<Vec<HypeRebuildRow>, HypeOpError> {
    let t = rtxn.open_table(HYPE_QUESTION_VECTORS_TABLE)?;
    let mut out = Vec::new();
    for entry in t.iter()? {
        let (k, v) = entry?;
        let key = k.value();
        let mut id_bytes = [0u8; 16];
        id_bytes.copy_from_slice(&key[..16]);
        out.push((MemoryId::from_be_bytes(id_bytes), bytes_to_vector(&v.value())));
    }
    Ok(out)
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use redb::{Database, ReadableDatabase};
    use tempfile::TempDir;

    fn mem(slot: u64) -> MemoryId {
        MemoryId::pack(0, slot, 0)
    }

    fn vec_seed(seed: f32) -> [f32; 384] {
        let mut v = [0.0f32; 384];
        v[0] = seed;
        v
    }

    fn open() -> (TempDir, Database) {
        let dir = TempDir::new().unwrap();
        let db = Database::create(dir.path().join("m.redb")).unwrap();
        (dir, db)
    }

    #[test]
    fn put_then_iter_round_trips() {
        let (_d, db) = open();
        let wtxn = db.begin_write().unwrap();
        hype_vector_put(&wtxn, mem(1), 0, &vec_seed(0.5)).unwrap();
        hype_vector_put(&wtxn, mem(1), 1, &vec_seed(0.7)).unwrap();
        hype_vector_put(&wtxn, mem(2), 0, &vec_seed(0.9)).unwrap();
        wtxn.commit().unwrap();

        let rtxn = db.begin_read().unwrap();
        let rows = hype_iter_all_vectors(&rtxn).unwrap();
        assert_eq!(rows.len(), 3);
        let mem1_count = rows.iter().filter(|(m, _)| *m == mem(1)).count();
        assert_eq!(mem1_count, 2, "memory 1 owns two question vectors");
        let (_, v) = rows.iter().find(|(m, _)| *m == mem(2)).unwrap();
        assert!((v[0] - 0.9).abs() < 1e-6, "vector round-trips");
    }

    #[test]
    fn delete_memory_removes_only_its_rows() {
        let (_d, db) = open();
        let wtxn = db.begin_write().unwrap();
        hype_vector_put(&wtxn, mem(1), 0, &vec_seed(0.1)).unwrap();
        hype_vector_put(&wtxn, mem(1), 1, &vec_seed(0.2)).unwrap();
        hype_vector_put(&wtxn, mem(2), 0, &vec_seed(0.3)).unwrap();
        wtxn.commit().unwrap();

        let wtxn = db.begin_write().unwrap();
        let removed = hype_vectors_delete_memory(&wtxn, mem(1)).unwrap();
        wtxn.commit().unwrap();
        assert_eq!(removed, 2);

        let rtxn = db.begin_read().unwrap();
        let rows = hype_iter_all_vectors(&rtxn).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, mem(2), "only memory 2 survives");
    }

    #[test]
    fn put_is_idempotent_on_index() {
        let (_d, db) = open();
        let wtxn = db.begin_write().unwrap();
        hype_vector_put(&wtxn, mem(1), 0, &vec_seed(0.1)).unwrap();
        hype_vector_put(&wtxn, mem(1), 0, &vec_seed(0.2)).unwrap();
        wtxn.commit().unwrap();
        let rtxn = db.begin_read().unwrap();
        let rows = hype_iter_all_vectors(&rtxn).unwrap();
        assert_eq!(rows.len(), 1, "same (memory, index) overwrites");
        assert!((rows[0].1[0] - 0.2).abs() < 1e-6);
    }
}
