# 17.5 — Statement HNSW per shard

Declare-only sub-task. Adds `StatementHnswIndex` in `brain-index` with
the same surface shape as `EntityHnswIndex` (16.3), but parameterised
for the larger statement scale per spec §26/00 (M=32, ef_construction=200,
ef_search=128).

**Populator deferred to phase 21.** Phase 17 wires the index type and
exposes the insert / search / tombstone / rebuild API; phase 21's
embedding worker re-embeds statements on create / supersede and feeds
this index. Tests cover the API contract using hand-crafted one-hot
vectors (mirroring entity_hnsw's test style).

## Spec refs

- `spec/26_knowledge_storage/00_purpose.md` §"statement.hnsw" — params,
  embedded representation, maintenance ownership.
- `spec/06_ann_index/02_parameters.md` — range envelope for M /
  ef_construction / ef_search.
- `spec/19_statements/07_references.md` table — confirms
  `crates/brain-index/src/statement_hnsw.rs` is the phase-17.5 home.
- `spec/16_benchmarks_acceptance/02_latency_targets.md` §2.4 — confirms
  semantic-search numbers don't gate phase 17; only the write/read API
  is verified now.

## Reads-only files

- `crates/brain-index/src/entity_hnsw.rs` — pattern to clone.
- `crates/brain-index/src/params.rs` — shared `VECTOR_DIM`,
  `IndexParamsError`, `MAX_LAYER`.
- `crates/brain-index/src/tombstones.rs` — shared `TombstoneBitmap`.

## Key design decisions

### D1 — `StatementHnswParams::default_v1` matches §26 explicitly

Statement HNSW gets bigger knobs than entity HNSW because statements
are denser (≈0.1–1× memory count per shard). Spec §26/00:

```
M=32, ef_construction=200, ef_search=128
```

Capacity hint: `1024` (mid-point between memory `1024` and
entity `256`). Tunable later in phase 21 once real workloads land.

### D2 — Same `hnsw_rs::Hnsw<DistCosine>` engine

Statement embeddings come from the same BGE-small model as memory and
entity embeddings (384-dim, L2-normalised). Cosine distance is the
spec primitive (§06/03). No new distance metric.

### D3 — No persistence in 17.5

The substrate's HNSW persistence (`persistence.rs`) is for the memory
HNSW only; entity HNSW also lacks persistence in v1 (16.3 F-2 deferral).
Statement HNSW follows the same path — in-memory only for now. Phase 23
will revisit when snapshot/restore lands across all three HNSWs.

### D4 — No concurrency wrapper

Like entity HNSW, statement HNSW is `!Send` + single-writer-by-&mut-self.
The shard's worker discipline (one writer per shard) is enough.

### D5 — `mark_tombstoned(StatementId)` mirrors the entity-side
contract

Statement tombstones (via `statement_ops::statement_tombstone`) write
the soft-delete bit on the redb row; the HNSW caller is expected to
invoke `mark_tombstoned` on the matching id so semantic search filters
the row out. This wiring lands in phase 21 (embedding worker subscribes
to `STATEMENT_TOMBSTONED` events); 17.5 just exposes the API.

## Plan

### Step 1 — Module skeleton

New file `crates/brain-index/src/statement_hnsw.rs`. Imports:

```rust
use std::collections::HashMap;

use brain_core::StatementId;
use hnsw_rs::prelude::{DistCosine, Hnsw, Neighbour};
use thiserror::Error;

use crate::params::{IndexParamsError, MAX_LAYER, VECTOR_DIM};
use crate::tombstones::TombstoneBitmap;

const OVER_FACTOR: usize = 2;
```

### Step 2 — `StatementHnswParams`

```rust
pub struct StatementHnswParams {
    pub m: usize,
    pub ef_construction: usize,
    pub ef_search: usize,
    pub ef_search_max: usize,
    pub capacity_hint: usize,
}

impl StatementHnswParams {
    pub const fn default_v1() -> Self {
        Self {
            m: 32,                    // spec §26/00 — vs entity's 16
            ef_construction: 200,     // spec §26/00 — vs entity's 100
            ef_search: 128,           // spec §26/00 — vs entity's 64
            ef_search_max: 500,
            capacity_hint: 1024,
        }
    }
    pub fn validate(&self) -> Result<(), IndexParamsError> { ... }
}
```

### Step 3 — `StatementHnswError`

```rust
pub enum StatementHnswError {
    InvalidParams(#[from] IndexParamsError),
    DuplicateStatement(StatementId),
    UnknownStatement(StatementId),
    EfSearchTooLarge { ef: usize, max: usize },
}
```

### Step 4 — `StatementHnswIndex`

Same surface as `EntityHnswIndex`:

```rust
pub struct StatementHnswIndex {
    inner: Hnsw<'static, f32, DistCosine>,
    params: StatementHnswParams,
    forward: Vec<Option<StatementId>>,
    reverse: HashMap<StatementId, u32>,
    tombstones: TombstoneBitmap,
}

impl StatementHnswIndex {
    pub fn new(params: StatementHnswParams) -> Result<Self, StatementHnswError>;
    pub fn insert(&mut self, id: StatementId, vector: &[f32; VECTOR_DIM]) -> Result<(), StatementHnswError>;
    pub fn search(&self, query: &[f32; VECTOR_DIM], k: usize) -> Result<Vec<(StatementId, f32)>, StatementHnswError>;
    pub fn search_with_ef(&self, query: &[f32; VECTOR_DIM], k: usize, ef: Option<usize>) -> Result<Vec<(StatementId, f32)>, StatementHnswError>;
    pub fn mark_tombstoned(&mut self, id: StatementId) -> Result<(), StatementHnswError>;
    pub fn is_tombstoned(&self, id: StatementId) -> bool;
    pub fn contains(&self, id: StatementId) -> bool;
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn tombstone_count(&self) -> usize;
    pub fn params(&self) -> StatementHnswParams;
    pub fn rebuild<I>(&mut self, items: I) -> Result<RebuildReport, StatementHnswError>
    where I: IntoIterator<Item = (StatementId, [f32; VECTOR_DIM])>;
}
```

`RebuildReport` reuses the entity-side struct (also a single
`inserted` + `duplicates_skipped` pair). Either share or duplicate —
cheap enough to duplicate, keeps each module self-contained. Choice:
duplicate inside `statement_hnsw.rs` for symmetry with the entity
module (which already duplicates instead of importing from `rebuild.rs`).

### Step 5 — Tests

Colocated `#[cfg(test)] mod tests`. ~12 tests mirroring `entity_hnsw`:

- `params_default_matches_spec` — assert `m=32`, `ef_construction=200`,
  `ef_search=128`, `capacity_hint=1024`.
- `params_validate_rejects_out_of_range` — same 4 error variants as
  entity HNSW.
- `insert_then_contains`.
- `insert_rejects_duplicate`.
- `search_empty_returns_empty`.
- `search_returns_inserted_with_high_similarity` — one-hot vectors;
  exact match returns similarity ≈ 1.0.
- `search_orders_by_similarity_descending`.
- `search_excludes_tombstoned`.
- `mark_tombstoned_unknown_id_errors`.
- `tombstone_idempotent`.
- `rebuild_replaces_index_and_clears_tombstones`.
- `rebuild_skips_duplicates`.

### Step 6 — Re-exports

Update `crates/brain-index/src/lib.rs`:

```rust
pub mod statement_hnsw;
pub use statement_hnsw::{
    StatementHnswError, StatementHnswIndex, StatementHnswParams,
    RebuildReport as StatementRebuildReport,
};
```

## Files written

| Path | Change |
|---|---|
| `crates/brain-index/src/statement_hnsw.rs` | New. ~400 lines (impl + ~12 tests). |
| `crates/brain-index/src/lib.rs` | Add module + re-exports. |

## Files NOT written this sub-task

- The embedding worker that populates the index — phase 21.
- Wire surface — not exposed via a wire opcode in v1 (semantic search
  lands through the hybrid query router in phase 23).
- Persistence (`statement.hnsw` snapshot/restore) — phase 23.
- Cross-shard fan-out — phase 23 query router.
- Statement HNSW ↔ statement_ops wiring — phase 21 (worker subscribes
  to events).

## Verification gate

```
cargo test -p brain-index statement_hnsw
cargo zigbuild --target x86_64-unknown-linux-gnu --workspace --tests
cargo clippy -p brain-index --all-targets -- -D warnings
```

All clean before committing.

## Commit message draft

```
feat(brain-index): statement HNSW index (17.5)

StatementHnswIndex mirrors EntityHnswIndex (16.3) — single-writer-by-
&mut-self, in-memory only, tombstone bitmap, cosine distance over
384-dim BGE-small vectors. Parameters per spec §26/00:
M=32, ef_construction=200, ef_search=128, capacity_hint=1024.

Insert / search / tombstone / rebuild surface ready for phase 21's
embedding worker to drive. ~12 unit tests cover param validation,
duplicate rejection, similarity ordering, tombstone filtering, and
rebuild idempotency.

Phase 21 wires the embedding worker that produces vectors from
(subject_canonical_name, predicate_name, object_text) and subscribes
to STATEMENT_CREATED / _SUPERSEDED / _TOMBSTONED events.

Plan: .claude/plans/phase-17-task-05.md.
```

## Risks

- **Code duplication with `entity_hnsw.rs`.** ~85% structural overlap.
  Generalising into a `KnowledgeHnswIndex<Id>` is tempting but
  out-of-scope here — keeps the entity-side stable. A phase-23
  refactor can unify once relations also need their own HNSW.
- **Parameters are spec defaults, not benchmark-tuned.** Real numbers
  come from phase 21's perf gate. 17.5 ships the spec values verbatim.
- **No populator means no end-to-end smoke test in 17.5.** Acceptable:
  the entity HNSW shipped the same way in 16.3 (resolver wiring in
  16.5+ proved the contract).

## Out of scope (this sub-task)

- Embedding worker (phase 21).
- Persistence (phase 23).
- Wire opcode for semantic search (phase 23 hybrid query router).
- Tombstone wiring with statement_ops (phase 21 — worker subscribes
  to events).
