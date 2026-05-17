# Plan: Phase 23 — Task 05, Filter chain

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1

---

## 1. Scope

Implement the post-fusion filter chain per §24/00 §"Filter chain".
Takes a `Vec<FusedItem>` (output of 23.4 RRF), reads metadata
from redb to evaluate each filter predicate against each item,
drops items that don't pass, applies the final limit, returns
the surviving slice.

Concrete deliverables:

1. New module `crates/brain-planner/src/knowledge/filters.rs`:
   - `FilterChain` config struct (per-filter knobs, all defaulted).
   - `FilterChainStats` for EXPLAIN/TRACE (per-filter drop counts).
   - `FilterError` taxonomy.
   - `apply_filter_chain(items: Vec<FusedItem>, chain: &FilterChain,
     metadata: &MetadataDb, limit: u32) -> Result<(Vec<FusedItem>, FilterChainStats), FilterError>`.
2. Five filters applied in §24/00's documented order:
   1. **Type** — `kind_filter` on Memory + Statement variants;
      `predicate_filter` on Statement variants. Entity / Relation
      pass through (no v1 type filter for those).
   2. **Temporal** — Memory: `created_at_unix_nanos` within range.
      Statement: `event_at` (Event) or `[valid_from, valid_to)`
      window (Fact / Preference). Relation: `[valid_from, valid_to)`.
      Entity: `created_at`. `None` bounds = open-ended.
   3. **Confidence** — Statement: `confidence ≥ threshold`.
      Relation: same. Memory: `salience ≥ threshold` (the
      substrate's analog to confidence; documented inline).
      Entity: no confidence; passes through.
   4. **Tombstone** — drop tombstoned rows unless
      `include_tombstoned == true`. Per-variant flag check:
      Memory `flags & ACTIVE == 0`, Statement
      `tombstoned == 1`, Relation `tombstoned == true`.
   5. **Supersession** — Statement / Relation:
      `superseded_by.is_some()` → drop unless
      `include_superseded == true`. Memory has no
      supersession.
3. **Limit** applied after all five filters.
4. Each filter reads metadata via a single shared read-txn
   opened once at the top of `apply_filter_chain`.
5. Per-filter drop counts in `FilterChainStats` for EXPLAIN/TRACE
   (23.8) — operators can see which filter narrowed the result
   the most.

NOT in scope:
- Push-down to retrievers — §24/00 §"Filter as retriever vs
  filter" describes type / temporal push-down into HNSW's
  filter callback / tantivy's query AST. v1 routes the
  push-down signal through the router's `temporal_pushdown`
  bit (23.3) but the actual retriever pre-filter wiring is
  deferred to 23.6's planner (or 23.6+ polish). The 23.5
  chain handles "post-fusion remaining filters" only.
- Confidence on Memory rows actually uses `salience` (not
  `confidence`) — documented in code; v1 binds Memory salience
  to the same `confidence_min` threshold per §24/00 §"Filter
  chain" (only mentions confidence but the only Memory analog
  is salience).
- Entity-type filter — not specified in §24/00 chain; entities
  pass through.

## 2. Spec references

- `spec/24_hybrid_query/00_purpose.md` §"Filter chain" — the
  ordering, the per-filter mechanics, and the push-down
  trade-off.
- `spec/19_statements/...` — `Statement.tombstoned`,
  `superseded_by`, `valid_from / valid_to`, `event_at`.
- `spec/20_relations/...` — `Relation.tombstoned`,
  `superseded_by`, `valid_from / valid_to`.
- `spec/07_metadata_graph/03_memory_metadata.md` —
  `MemoryMetadata.flags` ACTIVE bit; `created_at_unix_nanos`.

## 3. External validation

| Item | Source | Confirmed |
|---|---|---|
| `MemoryMetadata.flags` ACTIVE bit at `brain_metadata::tables::memory::flags::ACTIVE` | `brain-metadata::tables::memory` | Yes — `pub const ACTIVE: u32 = 1 << 0;`. |
| `Statement.tombstoned` + `superseded_by` accessors | `brain-metadata::statement_ops::statement_get -> Statement` | Yes — `Statement` is the brain-core type with the boolean + Option fields. |
| `Relation.tombstoned` + `superseded_by` | `brain-core::knowledge::relation::Relation` | Yes — same shape as Statement. |
| `event_at_unix_nanos` Option on Statement | brain-core | Yes — `Option<u64>`; only present for Event kind. |
| `MetadataDb::read_txn` | brain-metadata | Cheap; we open once per chain call. |

## 4. Architecture sketch

```rust
// crates/brain-planner/src/knowledge/filters.rs

use brain_core::knowledge::StatementKind;
use brain_core::{MemoryKind, PredicateId};
use brain_index::RankedItemId;
use brain_metadata::MetadataDb;

use super::fusion::FusedItem;
use super::router::TimeRange;

#[derive(Debug, Clone, Default)]
pub struct FilterChain {
    pub kind_filter: Vec<StatementKind>,         // empty = pass all
    pub memory_kind_filter: Vec<MemoryKind>,
    pub predicate_filter: Vec<PredicateId>,
    pub time_filter: Option<TimeRange>,
    pub confidence_min: Option<f32>,
    pub include_tombstoned: bool,
    pub include_superseded: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FilterChainStats {
    pub before: u32,
    pub after_type: u32,
    pub after_temporal: u32,
    pub after_confidence: u32,
    pub after_tombstone: u32,
    pub after_supersession: u32,
    pub after_limit: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum FilterError {
    #[error("metadata: {0}")]
    Metadata(String),
}

pub fn apply_filter_chain(
    mut items: Vec<FusedItem>,
    chain: &FilterChain,
    metadata: &MetadataDb,
    limit: u32,
) -> Result<(Vec<FusedItem>, FilterChainStats), FilterError> {
    let mut stats = FilterChainStats::default();
    stats.before = items.len() as u32;

    let rtxn = metadata.read_txn()
        .map_err(|e| FilterError::Metadata(format!("read_txn: {e}")))?;

    // §24/00 order:
    // 1. Type
    items = filter_type(items, chain, &rtxn)?;
    stats.after_type = items.len() as u32;

    // 2. Temporal
    items = filter_temporal(items, chain, &rtxn)?;
    stats.after_temporal = items.len() as u32;

    // 3. Confidence
    items = filter_confidence(items, chain, &rtxn)?;
    stats.after_confidence = items.len() as u32;

    // 4. Tombstone
    items = filter_tombstone(items, chain, &rtxn)?;
    stats.after_tombstone = items.len() as u32;

    // 5. Supersession
    items = filter_supersession(items, chain, &rtxn)?;
    stats.after_supersession = items.len() as u32;

    // 6. Limit
    if limit > 0 && items.len() > limit as usize {
        items.truncate(limit as usize);
    }
    stats.after_limit = items.len() as u32;

    Ok((items, stats))
}
```

Per-filter functions:

```rust
fn filter_type(
    items: Vec<FusedItem>,
    chain: &FilterChain,
    rtxn: &ReadTransaction,
) -> Result<Vec<FusedItem>, FilterError> {
    if chain.kind_filter.is_empty()
        && chain.memory_kind_filter.is_empty()
        && chain.predicate_filter.is_empty()
    {
        return Ok(items);
    }

    let mem_tbl = open_memory_table(rtxn)?;
    let stmt_tbl = open_statement_table(rtxn)?;

    items.into_iter().filter_map(|item| {
        let keep = match item.id {
            RankedItemId::Memory(id) => {
                if chain.memory_kind_filter.is_empty() {
                    true
                } else {
                    memory_kind_matches(&mem_tbl, id, &chain.memory_kind_filter)
                }
            }
            RankedItemId::Statement(id) => {
                statement_kind_or_predicate_matches(
                    &stmt_tbl, id, &chain.kind_filter, &chain.predicate_filter,
                )
            }
            RankedItemId::Entity(_) | RankedItemId::Relation(_) => true,
        };
        if keep { Some(item) } else { None }
    }).collect::<Vec<_>>().pipe(Ok)
}
```

(Same shape for the other four filters; each opens whatever
tables it needs from the shared rtxn.)

Confidence handling on Memory:

```rust
// §24/00 only specifies confidence on Statement / Relation
// rows. For Memory items in the fused result, v1 maps
// `confidence_min` against the `MemoryMetadata.salience`
// field — the substrate's analog. Documented inline.
fn memory_passes_confidence(meta: &MemoryMetadata, min: f32) -> bool {
    meta.salience >= min
}
```

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Single shared read-txn per chain call (this plan) | Amortises txn open; consistent snapshot | One lock acquisition (Mutex on MetadataDb); blocks writers briefly | ✓ — phase-22 indexers already follow the same pattern |
| Per-item read-txn | Less lock contention | 5x txn opens per item × N items; visible latency hit | rejected |
| Apply filters in a single pass | Save N - 1 vector allocations | Harder to compute per-filter stats; less testable | rejected — clarity wins, allocation cost minimal at top_k=64 |
| Use mut Vec<FusedItem> + drain in-place | Marginally cheaper | Borrow-checker dance; modest savings; we already build a fresh Vec per filter for stats | rejected |
| Bind Memory confidence to salience (this plan) | Closest analog to "confidence on Memory" | Spec only mentions confidence on Statement / Relation; Memory salience semantics differ | ✓ with inline documentation; minor — clients can omit `confidence_min` for memory-only queries |
| Apply confidence to Memory only via `salience_min` field | More precise | Adds another knob; v1 reuses `confidence_min` | rejected |

## 6. Risks / open questions

- **Risk:** A `FusedItem.id` points to a row that no longer exists (e.g. tombstoned between fusion and filter). **Mitigation:** treat as "drop the item" — caller already has the fused score; staleness windows are short.
- **Risk:** Limit applied last means we may compute filters over items that get truncated. **Mitigation:** matches §24/00 ordering; if measured perf hurts, future polish can early-terminate during filtering. v1 keeps the explicit order.
- **Risk:** Memory confidence-via-salience may surprise callers. **Mitigation:** documented inline; explicit clients can split queries (memory-only with no confidence filter; statement-only with one).
- **Open question:** Should `kind_filter` apply across both Memory and Statement (`StatementKind::Fact` doesn't apply to Memory)? **Resolution:** v1 splits them — `kind_filter` is statement-only; `memory_kind_filter` is memory-only. Caller passes whichever applies. (This is a deviation from a strict reading of §24/00 §"Type filter" — the spec mentions just "kind", but Memory and Statement use different enums; merging them is awkward.)
- **Open question:** Should we cache the read-txn at the planner level (23.6) and re-use across retrievers + fusion + filters? **Resolution:** out of scope for 23.5; the planner can later open the txn earlier and pass it down. v1 opens once per filter-chain call.

## 7. Test plan

Unit tests in `crates/brain-planner/src/knowledge/filters/tests.rs`:

- `empty_chain_passes_all` — no filters set → all items returned (up to limit).
- `kind_filter_drops_non_matching_statements` — 2 facts + 1 preference; filter `kind = [Preference]` → 1 item.
- `predicate_filter_drops_non_matching_statements` — 3 statements, 2 predicates; filter selects one.
- `memory_kind_filter_drops_non_matching_memories` — Episodic + Semantic; filter Episodic → 1.
- `time_filter_for_memory_uses_created_at` — 3 memories with different timestamps; range filters to one.
- `time_filter_for_statement_uses_event_at_for_event_kind` — Event statement with event_at outside range → dropped.
- `confidence_filter_for_statement` — 0.4 / 0.6 / 0.9 statements; threshold 0.5 → 2 items.
- `confidence_filter_for_memory_uses_salience` — analogous via salience.
- `tombstone_filter_drops_inactive_memory` — flags & ACTIVE == 0 → dropped unless include_tombstoned.
- `tombstone_filter_drops_tombstoned_statement` — same.
- `supersession_filter_drops_superseded_statement` — superseded_by Some → dropped unless include_superseded.
- `entity_passes_unfiltered` — entities pass through every filter (no per-entity predicates configured).
- `relation_passes_filters_for_type_chain` — relations pass tombstone + supersession filters.
- `filter_chain_stats_reflect_per_step_counts` — 10 items in, drops at each step → stats record the cumulative survivors.
- `limit_applied_last` — 5 items survive filters; limit 3 → 3 returned but stats.after_supersession == 5.

## 8. Commit shape

Single commit:

```
feat(planner): 23.5 — post-fusion filter chain

- crates/brain-planner/src/knowledge/filters.rs (new):
  apply_filter_chain + FilterChain + FilterChainStats +
  FilterError. Five filters in §24/00 order (type →
  temporal → confidence → tombstone → supersession),
  then limit. Single read-txn shared across all filters.
  Per-variant filter logic (Memory / Statement / Entity /
  Relation); entities pass through type / confidence /
  supersession; relations pass through some.
- crates/brain-planner/src/knowledge/filters/tests.rs (new):
  ~15 unit tests over a fresh MetadataDb seeded with
  representative Memory / Statement / Relation rows.
- crates/brain-planner/src/knowledge/mod.rs: pub mod filters.
```

## 9. Confirmation

Please confirm:

1. **`kind_filter` splits into `kind_filter` (statement) +
   `memory_kind_filter` (memory)** — vs a single enum that
   covers both. v1 splits because StatementKind and
   MemoryKind are distinct.
2. **Memory confidence binds to `MemoryMetadata.salience`** as
   the substrate analog (vs introducing a Memory-only
   threshold).
3. **Entity items pass through every v1 filter** — no
   entity_type_filter in §24/00; v1 doesn't invent one.
4. **Limit applied after the five filters** — matches §24/00.
5. **`FilterChainStats` exposes per-step survivor counts**
   for EXPLAIN/TRACE (23.8).

After approval: implement + tests + commit.
