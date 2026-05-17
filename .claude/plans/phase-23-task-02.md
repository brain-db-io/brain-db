# Plan: Phase 23 — Task 02, GraphRetriever impl

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1

---

## 1. Scope

Materialise §23/04. Implement `GraphRetriever` over the
entity / relation / statement redb tables exposed by
brain-metadata (phases 16–18). Adds the new `RankedItemId`
variants the spec declared in 23.0.

Concrete deliverables:

1. New trait + value types in `brain-index::graph_retriever`
   (matching the 23.1 split: trait in `brain-index`, impl in
   `brain-ops`).
2. New impl `BrainGraphRetriever` in
   `brain-ops::ops::graph_retriever` over a `Arc<Mutex<MetadataDb>>`.
3. Three modes: `Star`, `Path`, `Subgraph` (§23/04 §3).
4. **`RankedItemId` extension** — `tantivy_shard::retriever`'s
   `RankedItemId` enum grows `Entity(EntityId)` and
   `Relation(RelationId)` variants. The §23/02 §6 spec
   amendment lands inline with this commit.
5. `OpsContext.graph_retriever: Option<Arc<dyn GraphRetriever>>`
   slot wired at shard spawn.
6. Unit tests covering Star (depth 1 + 2), Path (success +
   no-path), Subgraph, relation-type filter, direction,
   branching cap, depth cap rejection, anchor-not-found,
   merged_into noted as v1 limitation.

NOT in scope:
- Following `Entity.merged_into` redirects automatically
  (§23/04 §9 — router resolves before invoking).
- Recursing into meta-statements (statement object = another
  StatementId) — §23/04 §9.
- Cross-shard traversal — §23/04 §9.
- Per-statement filter push-down via STATEMENTS_BY_SUBJECT
  secondary index optimisation — punted to 23.5 polish.

## 2. Spec references

- `spec/23_retrievers/04_graph_retriever.md` (landed in 23.0)
  — binding for trait surface, proximity scoring, three
  modes, caps, push-down, errors, v1 limitations.
- `spec/23_retrievers/02_lexical_retriever.md` §6 — amends
  `RankedItemId` to add `Entity` + `Relation` variants.
- `spec/23_retrievers/01_rrf_fusion.md` — consumes the
  `RankedItem` shape that this sub-task expands.

## 3. External validation

| Item | Source | Confirmed |
|---|---|---|
| `relation_list_from(rtxn, entity, &filter)` | `brain-metadata::relation_ops:153` | Yes; returns `Vec<Relation>` with relation-type filter, current_only, limit. ✓ |
| `relation_list_to(rtxn, entity, &filter)` | `brain-metadata::relation_ops:162` | Yes. ✓ |
| `entity_get(rtxn, id)` | `brain-metadata::entity_ops:99` | Yes. ✓ |
| `statement_list(rtxn, &filter)` with `subject: Option<EntityId>` | `brain-metadata::statement_ops:209` | Yes (used in §17 list-by-subject paths). ✓ |
| `Relation.is_symmetric` for direction normalisation | `brain-core::knowledge::relation:102` | Yes; mirrored on row for fast dispatch. ✓ |

## 4. Architecture sketch

### Trait + types in `brain-index`

```rust
// crates/brain-index/src/graph_retriever.rs (new)

use brain_core::{EntityId, RelationId, RelationTypeId, StatementId};

use crate::tantivy_shard::{RankedItem, RankedItemId};

pub trait GraphRetriever: Send + Sync {
    fn retrieve(
        &self,
        query: &GraphQuery,
        config: &GraphRetrieverConfig,
    ) -> Result<Vec<RankedItem>, GraphError>;
}

#[derive(Debug, Clone)]
pub enum GraphQuery {
    Star {
        anchor: EntityId,
        depth: u8,
        direction: Direction,
        relation_types: Option<Vec<RelationTypeId>>,
        include_statements: bool,
    },
    Path { from: EntityId, to: EntityId, max_depth: u8 },
    Subgraph { anchor: EntityId, depth: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction { Outgoing, Incoming, Both }

#[derive(Debug, Clone, Copy)]
pub struct GraphRetrieverConfig {
    pub top_k: usize,         // default 64
    pub max_depth: u8,        // hard cap 5 (§23/04 §4)
    pub max_branching: u32,   // default 200
    pub timeout_ms: u32,
}

impl Default for GraphRetrieverConfig { /* §23/04 §4 defaults */ }

#[derive(Debug, thiserror::Error)]
pub enum GraphError {
    #[error("anchor entity not found: {0:?}")]
    AnchorNotFound(EntityId),
    #[error("max depth {got} exceeds hard cap 5")]
    MaxDepthExceeded { got: u8 },
    #[error("index unavailable: {0}")]
    IndexUnavailable(String),
    #[error("query timed out after {0} ms")]
    Timeout(u32),
    #[error("internal: {0}")]
    Internal(String),
}
```

### `RankedItemId` extension (lockstep edit in `brain-index::tantivy_shard::retriever`)

```rust
// crates/brain-index/src/tantivy_shard/retriever.rs (amend)
pub enum RankedItemId {
    Memory(MemoryId),
    Statement(StatementId),
    Entity(EntityId),         // NEW (23.2)
    Relation(RelationId),     // NEW (23.2)
}
```

Existing match sites (in `tantivy_shard::retriever::project`,
the e2e tests, etc.) get a `_ => unreachable!()` arm or
explicit handling. The lexical retriever never emits the new
variants — it returns Memory or Statement per scope.

### Impl in `brain-ops`

```rust
// crates/brain-ops/src/ops/graph_retriever.rs (new)

use std::sync::Arc;
use std::collections::{HashMap, VecDeque};

use brain_core::{EntityId, RelationId};
use brain_index::{
    Direction, GraphError, GraphQuery, GraphRetriever, GraphRetrieverConfig,
    RankedItem, RankedItemId,
};
use brain_metadata::entity_ops::entity_get;
use brain_metadata::relation_ops::{relation_list_from, relation_list_to, RelationListFilter};
use brain_metadata::statement_ops::{statement_list, StatementListFilter};
use brain_metadata::MetadataDb;
use parking_lot::Mutex;

pub struct BrainGraphRetriever {
    metadata: Arc<Mutex<MetadataDb>>,
}

impl BrainGraphRetriever {
    pub fn new(metadata: Arc<Mutex<MetadataDb>>) -> Self { Self { metadata } }
}

impl GraphRetriever for BrainGraphRetriever {
    fn retrieve(&self, query: &GraphQuery, config: &GraphRetrieverConfig)
        -> Result<Vec<RankedItem>, GraphError>
    {
        validate_depth(query, config)?;

        let db_guard = self.metadata.lock();
        let rtxn = db_guard.read_txn()
            .map_err(|e| GraphError::IndexUnavailable(format!("read_txn: {e}")))?;

        match query {
            GraphQuery::Star { anchor, depth, direction,
                               relation_types, include_statements } => {
                run_star(&rtxn, *anchor, *depth, *direction,
                         relation_types.as_deref(), *include_statements, config)
            }
            GraphQuery::Path { from, to, max_depth } => {
                run_path(&rtxn, *from, *to, *max_depth, config)
            }
            GraphQuery::Subgraph { anchor, depth } => {
                run_subgraph(&rtxn, *anchor, *depth, config)
            }
        }
    }
}

fn run_star(rtxn, anchor, depth, direction, relation_types, include_statements, config)
    -> Result<Vec<RankedItem>, GraphError>
{
    // Verify anchor exists.
    let Some(_) = entity_get(rtxn, anchor)? else {
        return Err(GraphError::AnchorNotFound(anchor));
    };

    let mut frontier: HashMap<EntityId, u8> = HashMap::new();
    frontier.insert(anchor, 0);
    let mut visited: HashMap<EntityId, u8> = HashMap::new();
    let mut emitted_relations: Vec<(RelationId, u8)> = Vec::new();
    let mut emitted_entities: Vec<(EntityId, u8)> = Vec::new();
    let mut emitted_statements: Vec<(StatementId, u8)> = Vec::new();

    for hop in 0..depth {
        let mut next: HashMap<EntityId, u8> = HashMap::new();
        for (e, d) in frontier.drain() {
            if visited.contains_key(&e) { continue; }
            visited.insert(e, d);
            if d > 0 {  // omit the anchor itself
                emitted_entities.push((e, d));
            }
            if include_statements {
                // Collect statements with subject = e.
                let stmts = statement_list(rtxn,
                    &StatementListFilter {
                        subject: Some(e), current_only: true,
                        limit: config.max_branching as usize,
                        ..Default::default()
                    })?;
                for s in stmts {
                    emitted_statements.push((s.id, d));
                }
            }
            // Expand via relations matching direction + types.
            let mut children: Vec<(RelationId, EntityId)> = Vec::new();
            for r in fetch_relations(rtxn, e, direction, relation_types, config)? {
                let next_entity = match direction {
                    Direction::Outgoing => r.to_entity,
                    Direction::Incoming => r.from_entity,
                    Direction::Both => {
                        // Pick the neighbour (the one that isn't `e`).
                        if r.from_entity == e { r.to_entity } else { r.from_entity }
                    }
                };
                children.push((r.id, next_entity));
                if children.len() >= config.max_branching as usize {
                    tracing::warn!(
                        target: "brain_ops::graph_retriever",
                        anchor = ?e, max_branching = config.max_branching,
                        "graph retriever branching truncated",
                    );
                    break;
                }
            }
            children.sort_by_key(|(rid, _)| rid.to_bytes());
            for (rid, nx) in children {
                emitted_relations.push((rid, d));
                if !visited.contains_key(&nx) {
                    next.entry(nx).or_insert(d + 1);
                }
            }
            if emitted_relations.len() + emitted_entities.len()
                 + emitted_statements.len() >= config.top_k * 2 {
                break;  // early termination — sort + truncate below
            }
        }
        frontier = next;
    }

    // Project to RankedItem and sort by score (descending).
    let mut items: Vec<RankedItem> = Vec::new();
    for (id, d) in emitted_entities {
        items.push(rank_item(RankedItemId::Entity(id), d));
    }
    for (id, d) in emitted_relations {
        items.push(rank_item(RankedItemId::Relation(id), d));
    }
    for (id, d) in emitted_statements {
        items.push(rank_item(RankedItemId::Statement(id), d));
    }
    items.sort_by(|a, b| b.score.partial_cmp(&a.score)
                       .unwrap_or(std::cmp::Ordering::Equal)
                       .then_with(|| id_bytes_for_sort(&a.id).cmp(&id_bytes_for_sort(&b.id))));
    items.truncate(config.top_k);
    rerank_dense(&mut items);
    Ok(items)
}

fn rank_item(id: RankedItemId, hop_distance: u8) -> RankedItem {
    let score = 1.0 / (f32::from(hop_distance) + 1.0);
    RankedItem { id, rank: 0 /* set by rerank_dense */, score, snippet: None }
}
```

`run_path` does a single-source BFS from `from` with early
termination at `to`. `run_subgraph` is `run_star` with
`relation_types = None` and `include_statements = true`.

### OpsContext + shard spawn wiring

```rust
// brain-ops/src/context.rs
pub graph_retriever: Option<Arc<dyn GraphRetriever>>,
```

Shard spawn (after the semantic retriever construction):

```rust
let graph_for_ops: Option<Arc<dyn brain_index::GraphRetriever>> = {
    let r = brain_ops::ops::graph_retriever::BrainGraphRetriever::new(
        metadata.clone(),
    );
    Some(Arc::new(r))
};
// ...with_graph_retriever(graph_for_ops)
```

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Trait in `brain-index`, impl in `brain-ops` (this plan) | Mirrors 23.1; brain-index stays native-buildable | Two-crate plumbing | ✓ |
| Single direction BFS for Path with early termination (§23/04 §3) | Simple, deterministic | Worst-case O(b^d) on dense graphs at d=5 | ✓ — §23/04 §3 binds bidirectional as post-v1 |
| Bidirectional BFS | Faster on dense graphs | Algorithm complexity (merge midpoint) | rejected — §23/04 §3 v1 |
| Deterministic child-sort by `RelationId.to_bytes()` (this plan) | Reproducible outputs at tie | Ignores semantic "weight" of relation types | acceptable for v1; ties broken by id sort per §23/04 §2 |
| Emit anchor itself in Star results | Caller convenience | §23/04 §2 explicitly omits anchor (caller has it) | rejected — spec-compliant |
| Apply post-traversal predicate filter on statements inside the retriever | Self-contained | Adds an enum field to `GraphQuery`; filter chain (§24/00) already does this post-retrieval | rejected — leave to phase 23.5 |

## 6. Risks / open questions

- **Risk:** `relation_list_from` may return 1000s for high-degree entities. **Mitigation:** `RelationListFilter.limit` is respected; we set it to `max_branching` per call. Beyond that, the BFS truncates with a warn log.
- **Risk:** `Direction::Both` for symmetric relations might double-count. **Mitigation:** `visited` set keys on `EntityId`, so revisiting via a symmetric relation doesn't re-emit the entity. The relation row is emitted once per direction unless it's symmetric (then the from/to indexes both contain it but the same `RelationId` doesn't appear twice in `emitted_relations` because we sort/dedupe).
- **Risk:** Statement-list-by-subject can be expensive for popular entities. **Mitigation:** `StatementListFilter.limit` capped at `max_branching` per node. Documented in code.
- **Open question:** What's the cost of `entity_get` for the anchor verification — one redb get per query. Acceptable.
- **Open question:** Should `Path` mode include the `from` and `to` entities themselves in the result? **Resolution:** yes, per §23/04 §3 ("Emit: to and from with score = 1.0"). v1 impl emits both.

## 7. Test plan

Unit tests in `crates/brain-ops/src/ops/graph_retriever/tests.rs`:

- `star_depth_1_returns_neighbours` — A → B via relation; query Star anchor=A depth=1; returns B + the relation.
- `star_depth_2_includes_second_hop` — A → B → C; depth=2 returns B (d=1), C (d=2), both relations.
- `star_relation_type_filter` — A → B via type X, A → C via type Y; filter type X; only B + that relation.
- `star_direction_outgoing_vs_incoming` — A→B, C→A; outgoing returns B, incoming returns C.
- `star_include_statements_emits_statements` — A has a statement with subject=A; query with `include_statements=true` includes the statement.
- `star_branching_truncation` — A with 250 outgoing relations + `max_branching=100`; BFS truncates; result has at most ~100 emitted neighbours.
- `path_finds_direct_link` — A → B; Path A→B returns A, B, the relation, all with score 1.0 / 0.5 / 0.5.
- `path_no_path_returns_endpoints_only` — A, B with no connection; Path A→B returns A + B both with score 0.5.
- `subgraph_returns_closed_neighbourhood` — A → B → C; subgraph A depth=2 returns A, B, C + 2 relations.
- `max_depth_above_cap_errors` — depth=6 → MaxDepthExceeded.
- `anchor_not_found_errors` — random EntityId → AnchorNotFound.
- `ranks_are_dense_and_one_based` — output ranks 1, 2, 3, …, no gaps.

## 8. Commit shape

Single commit:

```
feat(index,ops,server): 23.2 — GraphRetriever trait + impl

- crates/brain-index/src/graph_retriever.rs (new): trait +
  GraphQuery { Star | Path | Subgraph } + Direction +
  GraphRetrieverConfig + GraphError taxonomy.
- crates/brain-index/src/tantivy_shard/retriever.rs (amend):
  RankedItemId gains Entity(EntityId) + Relation(RelationId)
  variants per §23/04 §1. Existing match sites updated.
- crates/brain-index/src/lib.rs: re-export the new surface.
- crates/brain-ops/src/ops/graph_retriever.rs (new):
  BrainGraphRetriever impl. Star/Path/Subgraph BFS with
  visited-set loop break, deterministic child sort,
  branching truncation, top_k early termination. Single-
  source BFS for Path (§23/04 §3 v1).
- crates/brain-ops/src/ops/graph_retriever/tests.rs (new):
  12 unit tests covering all three modes + edges.
- crates/brain-ops/src/context.rs: `graph_retriever:
  Option<Arc<dyn GraphRetriever>>` + with_graph_retriever.
- crates/brain-server/src/shard/mod.rs: shard spawn
  constructs BrainGraphRetriever::new(metadata.clone()) and
  installs on OpsContext.
```

## 9. Confirmation

Please confirm:

1. **Trait in `brain-index`, impl in `brain-ops`** — matches 23.1 pattern.
2. **`RankedItemId` enum amendment** — adds `Entity` + `Relation` variants in the same commit (impacts §23/02 §6 spec line; amends spec inline).
3. **Single-source BFS for Path mode** with early termination at `to` (vs bidirectional BFS) — §23/04 §3 v1 binding.
4. **Deterministic child-sort by `RelationId.to_bytes()` ascending** for tie-breaking.
5. **Star omits anchor itself** from results (`hop_distance == 0` skipped) per §23/04 §2.

After approval: implement + tests + commit.
