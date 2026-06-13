//! `HypeHnswIndex` — per-shard HNSW over hypothetical-question
//! embeddings (HyPE: Hypothetical Prompt Embeddings).
//!
//! At write time an LLM generates several diverse questions whose answer
//! is a given memory; each question is embedded and inserted here as one
//! point. At read time the user's query vector probes this pool, and a
//! hit maps back to the memory the question was generated for — bridging
//! the query↔memory phrasing gap that the direct passage embedding
//! misses.
//!
//! It differs from [`crate::entity_hnsw::EntityHnswIndex`] in one
//! structural way: the mapping is **many-to-one**. A single memory owns
//! several question points, so there is no by-memory dedup on insert and
//! [`HypeHnswIndex::search`] collapses the raw question hits to the best
//! similarity per memory before returning.
//!
//! - In-memory only; the vectors persist in redb (`hype_question_vectors`
//!   table) and this index is a derived structure rebuilt on boot.
//! - Single-owner; no concurrency wrapper (the shard wraps it in an
//!   `Arc<RwLock<_>>` like the entity index).

use std::collections::HashMap;

use brain_core::MemoryId;
use hnsw_rs::prelude::{DistCosine, Hnsw, Neighbour};
use thiserror::Error;

use crate::entity_hnsw::EntityHnswParams;
use crate::params::{MAX_LAYER, VECTOR_DIM};
use crate::tombstones::TombstoneBitmap;

/// Over-fetch multiplier for `search`. Higher than the entity index's
/// `2` because several question points collapse to one memory: to return
/// `k` distinct memories we must pull enough raw points that `k` survive
/// the per-memory dedup. Sized for the ~5–8 questions a memory typically
/// owns.
const OVER_FACTOR: usize = 8;

/// Default HNSW knobs for the HyPE pool. Reuses [`EntityHnswParams`] (the
/// knobs are index-agnostic) but with a larger `capacity_hint`: the pool
/// holds several points per memory, so it is bigger than either the
/// entity or memory index.
#[must_use]
pub fn hype_default_params() -> EntityHnswParams {
    EntityHnswParams {
        m: 16,
        ef_construction: 100,
        ef_search: 64,
        ef_search_max: 500,
        capacity_hint: 4096,
    }
}

// ---------------------------------------------------------------------------
// Errors.
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum HypeHnswError {
    #[error("invalid params: {0}")]
    InvalidParams(#[from] crate::params::IndexParamsError),

    /// `ef_search` override exceeded `ef_search_max`.
    #[error("ef_search {ef} above ef_search_max {max}")]
    EfSearchTooLarge { ef: usize, max: usize },
}

// ---------------------------------------------------------------------------
// RebuildReport.
// ---------------------------------------------------------------------------

/// Outcome of [`HypeHnswIndex::rebuild`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebuildReport {
    /// Number of question points re-inserted from the input iterator.
    pub inserted: usize,
    /// Number of distinct memories represented by those points.
    pub memories: usize,
}

// ---------------------------------------------------------------------------
// HypeHnswIndex.
// ---------------------------------------------------------------------------

/// Per-shard HNSW over hypothetical-question embeddings (384-dim,
/// BGE-small). Many question points map to one [`MemoryId`].
///
/// **Single-writer** by `&mut self` discipline.
pub struct HypeHnswIndex {
    inner: Hnsw<'static, f32, DistCosine>,
    params: EntityHnswParams,
    /// Internal u32 point id → owning `MemoryId`. One entry per inserted
    /// question vector; several entries may share a `MemoryId`.
    forward: Vec<MemoryId>,
    /// `MemoryId` → the internal point ids it owns. Supports
    /// [`Self::mark_memory_tombstoned`] (drop every question of a
    /// forgotten memory) and [`Self::contains_memory`].
    by_memory: HashMap<MemoryId, Vec<u32>>,
    tombstones: TombstoneBitmap,
}

impl HypeHnswIndex {
    /// Construct an empty index with the given parameters.
    pub fn new(params: EntityHnswParams) -> Result<Self, HypeHnswError> {
        params.validate()?;
        let inner = Hnsw::<f32, DistCosine>::new(
            params.m,
            params.capacity_hint,
            MAX_LAYER,
            params.ef_construction,
            DistCosine,
        );
        Ok(Self {
            inner,
            params,
            forward: Vec::new(),
            by_memory: HashMap::new(),
            tombstones: TombstoneBitmap::new(),
        })
    }

    /// Insert one question `vector` owned by `memory_id`. Unlike the
    /// entity index this never rejects "duplicates" — a memory is
    /// expected to own several question points.
    pub fn insert(&mut self, memory_id: MemoryId, vector: &[f32; VECTOR_DIM]) {
        let internal_id = u32::try_from(self.forward.len())
            .expect("invariant: HyPE point count per shard never reaches u32::MAX");
        self.forward.push(memory_id);
        self.by_memory.entry(memory_id).or_default().push(internal_id);
        self.inner
            .insert_slice((vector.as_slice(), internal_id as usize));
    }

    /// Search the top-`k` nearest **memories** to `query`. Raw question
    /// hits are collapsed to the best (highest) similarity per memory, so
    /// a memory whose several questions all match counts once, at its
    /// strongest. Returns `(MemoryId, similarity)` sorted descending by
    /// similarity. Tombstoned points are excluded.
    pub fn search(
        &self,
        query: &[f32; VECTOR_DIM],
        k: usize,
    ) -> Result<Vec<(MemoryId, f32)>, HypeHnswError> {
        self.search_with_ef(query, k, None)
    }

    /// Variant of [`Self::search`] with an explicit `ef_search` override.
    /// `None` uses the configured default; `Some(v)` is clamped to
    /// `[fetch_k, ef_search_max]`.
    pub fn search_with_ef(
        &self,
        query: &[f32; VECTOR_DIM],
        k: usize,
        ef: Option<usize>,
    ) -> Result<Vec<(MemoryId, f32)>, HypeHnswError> {
        if k == 0 || self.forward.is_empty() {
            return Ok(Vec::new());
        }
        // Pull enough raw points that `k` distinct memories survive the
        // per-memory collapse below.
        let fetch_k = k.saturating_mul(OVER_FACTOR).min(self.forward.len());
        let ef = match ef {
            None => self.params.ef_search.max(fetch_k),
            Some(v) => {
                if v > self.params.ef_search_max {
                    return Err(HypeHnswError::EfSearchTooLarge {
                        ef: v,
                        max: self.params.ef_search_max,
                    });
                }
                v.max(fetch_k)
            }
        };

        let neighbours: Vec<Neighbour> = self.inner.search(query.as_slice(), fetch_k, ef);
        // Best similarity per memory.
        let mut best: HashMap<MemoryId, f32> = HashMap::new();
        for n in neighbours {
            let Ok(internal_id) = u32::try_from(n.d_id) else {
                continue;
            };
            if self.tombstones.is_set(internal_id) {
                continue;
            }
            let Some(memory_id) = self.forward.get(internal_id as usize).copied() else {
                tracing::warn!(
                    internal_id,
                    "HyPE HNSW returned an internal id with no MemoryId mapping; dropping"
                );
                continue;
            };
            let sim = 1.0 - n.distance;
            best.entry(memory_id)
                .and_modify(|cur| {
                    if sim > *cur {
                        *cur = sim;
                    }
                })
                .or_insert(sim);
        }

        let mut out: Vec<(MemoryId, f32)> = best.into_iter().collect();
        out.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.raw().cmp(&b.0.raw()))
        });
        out.truncate(k);
        Ok(out)
    }

    /// Tombstone every question point owned by `memory_id` (the FORGET
    /// cascade analogue). No-op if the memory owns no points.
    pub fn mark_memory_tombstoned(&mut self, memory_id: MemoryId) {
        if let Some(ids) = self.by_memory.get(&memory_id) {
            for id in ids {
                self.tombstones.set(*id);
            }
        }
    }

    #[must_use]
    pub fn contains_memory(&self, memory_id: MemoryId) -> bool {
        self.by_memory.contains_key(&memory_id)
    }

    /// Number of question points (including tombstoned).
    #[must_use]
    pub fn len(&self) -> usize {
        self.forward.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.forward.is_empty()
    }

    /// Number of distinct memories with at least one point.
    #[must_use]
    pub fn memory_count(&self) -> usize {
        self.by_memory.len()
    }

    #[must_use]
    pub fn tombstone_count(&self) -> usize {
        self.tombstones.count()
    }

    #[must_use]
    pub fn params(&self) -> EntityHnswParams {
        self.params
    }

    /// Discard the current index and re-insert every `(MemoryId,
    /// vector)` from `points`. Tombstones are cleared and the underlying
    /// `hnsw_rs::Hnsw` is replaced with a fresh instance. Callers should
    /// pre-filter points of tombstoned memories; this does not honor any
    /// prior tombstone state.
    pub fn rebuild<I>(&mut self, points: I) -> RebuildReport
    where
        I: IntoIterator<Item = (MemoryId, [f32; VECTOR_DIM])>,
    {
        self.inner = Hnsw::<f32, DistCosine>::new(
            self.params.m,
            self.params.capacity_hint,
            MAX_LAYER,
            self.params.ef_construction,
            DistCosine,
        );
        self.forward.clear();
        self.by_memory.clear();
        self.tombstones.clear();

        let mut report = RebuildReport::default();
        for (memory_id, vector) in points {
            self.insert(memory_id, &vector);
            report.inserted += 1;
        }
        report.memories = self.by_memory.len();
        report
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn one_hot(seed: usize) -> [f32; VECTOR_DIM] {
        let mut v = [0.0; VECTOR_DIM];
        v[seed % VECTOR_DIM] = 1.0;
        v
    }

    fn mem(slot: u64) -> MemoryId {
        MemoryId::pack(0, slot, 0)
    }

    #[test]
    fn insert_allows_many_points_per_memory() {
        let mut idx = HypeHnswIndex::new(hype_default_params()).unwrap();
        let m = mem(1);
        idx.insert(m, &one_hot(0));
        idx.insert(m, &one_hot(1));
        idx.insert(m, &one_hot(2));
        assert_eq!(idx.len(), 3, "three question points");
        assert_eq!(idx.memory_count(), 1, "one owning memory");
        assert!(idx.contains_memory(m));
    }

    #[test]
    fn search_collapses_to_best_per_memory() {
        let mut idx = HypeHnswIndex::new(hype_default_params()).unwrap();
        let m = mem(7);
        // Two questions for the same memory; one is the exact query.
        idx.insert(m, &one_hot(7));
        idx.insert(m, &one_hot(200));
        let r = idx.search(&one_hot(7), 5).unwrap();
        assert_eq!(r.len(), 1, "one memory despite two points");
        assert_eq!(r[0].0, m);
        assert!(r[0].1 > 0.99, "best (self) similarity surfaces; got {}", r[0].1);
    }

    #[test]
    fn search_returns_k_distinct_memories() {
        let mut idx = HypeHnswIndex::new(hype_default_params()).unwrap();
        for slot in 0..10u64 {
            // Two points each.
            idx.insert(mem(slot), &one_hot(slot as usize));
            idx.insert(mem(slot), &one_hot(slot as usize + 300));
        }
        let r = idx.search(&one_hot(0), 5).unwrap();
        assert!(r.len() <= 5, "k bounds distinct memories: {}", r.len());
        let mut seen = std::collections::HashSet::new();
        for (id, _) in &r {
            assert!(seen.insert(*id), "memories are distinct in the result");
        }
    }

    #[test]
    fn search_empty_returns_empty() {
        let idx = HypeHnswIndex::new(hype_default_params()).unwrap();
        assert!(idx.search(&one_hot(0), 5).unwrap().is_empty());
    }

    #[test]
    fn tombstone_drops_all_questions_of_a_memory() {
        let mut idx = HypeHnswIndex::new(hype_default_params()).unwrap();
        let a = mem(1);
        let b = mem(2);
        idx.insert(a, &one_hot(0));
        idx.insert(a, &one_hot(1));
        idx.insert(b, &one_hot(2));
        idx.mark_memory_tombstoned(a);
        assert_eq!(idx.tombstone_count(), 2);
        let r = idx.search(&one_hot(0), 5).unwrap();
        let ids: Vec<MemoryId> = r.iter().map(|(id, _)| *id).collect();
        assert!(!ids.contains(&a), "tombstoned memory's questions excluded");
    }

    #[test]
    fn search_with_ef_above_max_errors() {
        let mut idx = HypeHnswIndex::new(hype_default_params()).unwrap();
        idx.insert(mem(1), &one_hot(0));
        let err = idx
            .search_with_ef(&one_hot(0), 5, Some(1000))
            .expect_err("over max");
        assert!(matches!(
            err,
            HypeHnswError::EfSearchTooLarge { ef: 1000, max: 500 }
        ));
    }

    #[test]
    fn rebuild_resets_and_reinserts() {
        let mut idx = HypeHnswIndex::new(hype_default_params()).unwrap();
        idx.insert(mem(1), &one_hot(0));
        idx.mark_memory_tombstoned(mem(1));
        let report = idx.rebuild(vec![
            (mem(5), one_hot(5)),
            (mem(5), one_hot(6)),
            (mem(6), one_hot(7)),
        ]);
        assert_eq!(report.inserted, 3);
        assert_eq!(report.memories, 2);
        assert_eq!(idx.tombstone_count(), 0);
        assert!(!idx.contains_memory(mem(1)));
        assert!(idx.contains_memory(mem(5)));
    }
}
