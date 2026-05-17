# Plan: Phase 23 — Task 00, Spec backfill (semantic + graph retrievers, hybrid perf)

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1 (one `docs(spec): 23.0 — ...` commit)

---

## 1. Scope

Bring the §23 retrievers + §16/02 to phase-23 implementation
depth so 23.1 (SemanticRetriever) and 23.2 (GraphRetriever) can
cite concrete §N.M anchors. §24/00 (hybrid query / router /
executor / filter chain) is already at depth — explicit
non-goal here.

Three deliverables:

1. **`spec/23_retrievers/03_semantic_retriever.md`** (new,
   ~180 LOC). Trait surface, corpus selection, filter
   push-down, config, error taxonomy, perf bounds.
2. **`spec/23_retrievers/04_graph_retriever.md`** (new,
   ~200 LOC). Trait surface, three modes (`Star`, `Path`,
   `Subgraph`), proximity scoring, depth + branching caps,
   pre-filter push-down, errors, perf bounds.
3. **`spec/16_benchmarks_acceptance/02_latency_targets.md`**
   §2.10 amendment — hybrid query latency targets at
   100K / 1M scale. Phase-gate list renumbered §2.10 → §2.11
   with the phase-23 row added.

Also writes the phase-23 master plan
(`.claude/plans/phase-23.md`) indexing 23.0–23.12 with the
scope cuts declared up front, mirroring the 22.0 commit.

**Not in scope** (deferred to per-sub-task plans):

- §24/01 router rules as a dedicated file — left inline in
  §24/00. The 5 rules + classification features are already
  at usable depth; revisit only if 23.3 hits ambiguity.
- §24/02 filter chain / §24/03 executor / §24/04 EXPLAIN-TRACE
  dedicated files — same call; usable inline in §24/00.
- §28 wire-opcode allocation (`0x60–0x63`) — 23.9 plan.
- §29 SDK query builder — 23.10 plan.
- Learned routing — explicit out-of-scope; §24/00 §"Learned
  routing" already documents the post-v1 path.

## 2. Spec references — current state vs. needed depth

### `spec/23_retrievers/`

| File | Present? | Notes |
|---|---|---|
| `00_purpose.md` | yes (181 LOC) | All three retrievers sketched; semantic ~12 lines, graph ~25 lines, lexical ~15 lines (now superseded by §23/02). |
| `01_rrf_fusion.md` | yes | k=60 default, weight semantics, per-query override. At depth for 23.4. |
| `02_lexical_retriever.md` | yes (22.0) | Reference pattern for the new files. |
| `03_semantic_retriever.md` | **missing — new** | Full SemanticRetriever mechanics. |
| `04_graph_retriever.md` | **missing — new** | Full GraphRetriever mechanics. |
| `07_open_questions.md` | absent | Optional — only add if any 23.1/23.2 deferrals surface. |

Binding constraints already in `00_purpose.md`:

> "SemanticRetriever wraps the substrate HNSW (section 06). It operates over multiple corpora: Memory HNSW, Statement HNSW, Entity HNSW (entity resolver only, not query retrieval typically). Configuration: Search target: memory | statement | both. ef_search, top_k, similarity threshold. Returns: ranked items with cosine scores."

> "GraphRetriever operates on the entity graph (Entities + Relations + Statement subjects). Inputs: anchor entity, traversal spec (relation types, max depth, direction). Three modes: Star, Path, Subgraph. Configuration: max_depth (default 3, capped at 5), direction (outgoing/incoming/both), relation_type_filter (optional whitelist). Performance: 1-2 hops are fast (O(log N) per hop). 3+ hops can be expensive."

### `spec/24_hybrid_query/`

| File | Present? | Notes |
|---|---|---|
| `00_purpose.md` | yes (269 LOC) | Query shape, classification, 5 routing rules, filter chain + push-down, plan DAG, parallel executor, streaming, result shape, learned routing future. At depth. |

No §24 backfill in 23.0.

### `spec/16_benchmarks_acceptance/02_latency_targets.md`

Currently terminates at §2.9 (LexicalRetriever — phase 22.0).
Phase doc 23.12 wants `P50 ≤ 10 ms, P99 ≤ 50 ms hybrid`; no
§2.10 entry exists. Phase-gate table currently lists phases
16–22 with phase 23 absent.

## 3. External validation

| Item | Source | Why |
|---|---|---|
| `hnsw_rs` filter callback shape | docs.rs/hnsw_rs/latest | Confirm push-down filter API for SemanticRetriever pre-filters (kind / agent_id / created_at). |
| Statement-HNSW interface in `brain-index` | crates/brain-index/src/statement_hnsw.rs | Confirm available corpora (existing `StatementHnswIndex` from phase 17). |
| Entity-HNSW reuse | brain-index::entity_hnsw | Note that §23/03 declares entity HNSW out of scope for retrieval (resolver-only). |
| redb iteration for graph traversal | brain-metadata existing helpers | Star/Path/Subgraph need entity + relation + statement table walks; phase 18 already exposes the read paths. |

These are confirmations to ground the spec, not architectural
choices — the implementation plans (23.1 / 23.2) own those.

## 4. Architecture sketch — spec files to be written

### `spec/23_retrievers/03_semantic_retriever.md` (~180 LOC)

```
§1 Surface
  pub trait SemanticRetriever (object-safe, Send + Sync)
  fn retrieve(query: &SemanticQuery, scope: SemanticScope,
              config: &SemanticRetrieverConfig)
      -> Result<Vec<RankedItem>, SemanticError>

  enum SemanticQuery {
      Vector([f32; D]),              // pre-embedded
      Text(String),                  // brain-embed on demand
  }
  enum SemanticScope { Memory, Statement, Both }
  struct SemanticFilters { agent_id, memory_kind,
      statement_kind, predicate_id, confidence_bucket,
      created_at_ms, extracted_at_ms }
  struct SemanticRetrieverConfig { top_k, ef_search,
      similarity_threshold, timeout_ms }

§2 Embedding semantics
  - Vector input: D = VECTOR_DIM (384, BGE-small).
  - Text input: brain-embed encodes via the substrate
    embedder; vector then used as if passed directly.
  - Mismatched dimensions → QueryParseFailed.

§3 HNSW search params
  - ef_search default 64; max 500 (caps memory).
  - similarity_threshold: cosine ≥ threshold (default 0.0;
    no cutoff). Applied post-search.
  - top_k default 64.

§4 Scope dispatch
  - Memory → memory_hnsw + memory metadata for filter joins.
  - Statement → statement_hnsw + statement metadata.
  - Both → fan out both; result merged by descending cosine,
    `RankedItemId::Memory | Statement` distinguished.

§5 Filter push-down
  - HNSW filter callback: bool fn invoked per candidate
    after vector search. Rejecting candidates triggers the
    HNSW iterator to keep expanding until top_k accepted
    (capped by ef_search).
  - Filters routed to push-down where the HNSW filter
    callback can answer cheaply (agent_id, kind from
    metadata side-table). Range filters (created_at,
    extracted_at, confidence_bucket) applied post-search.

§6 Returns + idempotency
  - Same shape as §23/02: Vec<RankedItem> with dense
    1-based ranks, cosine `score`, snippet always None.

§7 Errors
  - IndexUnavailable (HNSW rebuild in progress).
  - QueryParseFailed (dim mismatch, scope+filter mismatch).
  - Timeout (config.timeout_ms exceeded).
  - EmbedderFailure (Text path; surfaces brain-embed error).

§8 Performance
  - Pin §16/02 §2.10 targets (single-corpus p50 5 ms, both
    corpora p50 8 ms at production scale).

§9 Boundaries
  - Doesn't write to HNSW (that's the embedding workers).
  - Doesn't choose scope (§24 router does).
  - Doesn't fuse (§23/01 + planner does).
```

### `spec/23_retrievers/04_graph_retriever.md` (~200 LOC)

```
§1 Surface
  pub trait GraphRetriever (object-safe, Send + Sync)
  fn retrieve(query: &GraphQuery, config: &GraphRetrieverConfig)
      -> Result<Vec<RankedItem>, GraphError>

  enum GraphQuery {
      Star { anchor: EntityId, depth: u8, direction,
             relation_types: Option<Vec<RelationTypeId>>,
             include_statements: bool },
      Path { from: EntityId, to: EntityId, max_depth: u8 },
      Subgraph { anchor: EntityId, depth: u8 },
  }
  enum Direction { Outgoing, Incoming, Both }
  struct GraphRetrieverConfig { top_k, max_depth (≤ 5),
      max_branching, timeout_ms }

§2 Proximity scoring
  - score = 1.0 / ((hop_distance as f32) + 1.0)
  - hop_distance is BFS depth from anchor; 0 for anchor
    itself (omitted from result unless explicitly included).
  - Path mode: hop_distance is the path length.
  - Ties broken by entity_id stability sort.

§3 Three modes
  Star:
    - BFS from anchor up to `depth` along relations matching
      `relation_types` (None = all) in `direction`.
    - At each hop, emit entities + (if `include_statements`)
      statements whose subject is one of the visited entities.

  Path:
    - Bidirectional BFS from `from` and `to` until they meet
      or `max_depth` exceeded.
    - Returns relations + entities on at least one shortest
      path. Multiple shortest paths → all emitted up to top_k.

  Subgraph:
    - Closed neighbourhood: every entity, relation, statement
      reachable within `depth` hops of anchor.

§4 Depth + branching caps
  - max_depth: default 3, hard cap 5. Beyond 5 → QueryParseFailed.
  - max_branching: default 200 children per node. Exceeded
    → degrade (truncate the children of that node, log).
  - Total result cap: top_k from config (default 64).

§5 Pre-filter push-down
  - relation_types and direction are pushed into the BFS
    expansion (filter before traversal, not after).
  - kind / predicate filters on emitted statements apply
    post-traversal (filters are cheap on small result sets;
    push-down through statement table is a phase 23.5 polish).

§6 Returns + idempotency
  - Same RankedItem shape; `RankedItemId::Entity(EntityId) |
    RankedItemId::Relation(RelationId) | RankedItemId::Statement(StatementId)`
    (NEW variant — needs §23/02 amendment).
  - Read-only; idempotent between commits.

§7 Errors
  - AnchorNotFound, MaxDepthExceeded, IndexUnavailable, Timeout,
    Internal.

§8 Performance
  - Pin §16/02 §2.10 targets (Star depth=1 p50 5 ms, Star
    depth=2 p50 10 ms, Subgraph depth=2 p50 15 ms).

§9 Boundaries
  - Doesn't follow `merged_into` redirects automatically; the
    caller (router) resolves the alias before invoking.
  - Doesn't expand statement objects through their nested
    statements (Statement-as-object); deferred post-v1.
```

### `spec/16_benchmarks_acceptance/02_latency_targets.md` §2.10 amendment

```
### 2.10 Hybrid query (phase 23)

Hybrid query latency is dominated by per-retriever latency;
fusion + filter overhead is sub-ms. Phase 23 gate uses three
retrievers in parallel (semantic + lexical + graph at depth 1).

| Operation | p50 | p99 |
|---|---|---|
| Hybrid 3-retriever, push-down filters | 10 ms | 50 ms |
| Hybrid 3-retriever, post-fusion filters only | 15 ms | 70 ms |
| Hybrid single-retriever (router degraded) | 7 ms | 30 ms |
| EXPLAIN (plan-only, no execution) | 500 µs | 2 ms |
| Filter chain (1K candidates, full chain) | 1 ms | 5 ms |
| RRF fusion (3 lists × 100 items) | 100 µs | 500 µs |

Hybrid query end-to-end at production scale (100K memories /
1M statements / 100K entities) is the phase-14 acceptance gate.
The §16/02 §2.10 numbers above are the phase-23 sub-task 23.12
gate at 10K corpus scale.

### 2.11 Phase perf gates  (renumbered from §2.10)

  ... phase 16-22 rows kept ...
  - **Phase 23 (sub-task 23.12)** — §2.10 hybrid query
    targets at 10K corpus / 3 retrievers.
```

Plus a one-liner under §23/02 §6 `RankedItemId` to call out
that 23.0 will expand the enum with `Entity | Relation`
variants. (Or this lands in 23.2's commit. Inline note here.)

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Backfill the two retriever files only (this plan) | Smallest surface; §24 already at depth | Doesn't preempt §24 ambiguity that may surface in 23.3 / 23.5 / 23.7 | ✓ |
| Also break out §24/01 router / §24/02 filter / §24/03 executor as dedicated files | Cleaner cross-reference targets | §24/00 already has them inline at usable depth; busywork | rejected |
| Defer §16/02 §2.10 to 23.12's plan | Less churn here | Phase-23 sub-task 23.12 needs a target to validate against; the gate must be in the spec at phase start | rejected |
| Add `RankedItemId::Entity / Relation` in 23.0 (touches `brain-index`) | Avoids 23.2 retrofit | Implementation change disguised as a spec backfill | rejected — declare in spec; impl in 23.2 |
| Add `§23/07 open_questions.md` now | Mirrors §22/07 / §27/07 | Nothing to record yet; create when 23.x defers something | rejected (open file when there's content) |

## 6. Risks / open questions

- **Risk:** `hnsw_rs` filter callback API may not match the push-down model in §3. **Mitigation:** §3 says "where the HNSW filter callback can answer cheaply"; if `hnsw_rs` doesn't support callback filtering, 23.1 falls back to post-search filtering (still correct, slightly slower). Spec stays as-is; impl plan adjusts.
- **Risk:** Statement HNSW corpus shape (vector representation) is `subject.canonical_name + predicate + object_text` per §23/00. v1 doesn't have an embedding worker for statements yet; phase 23.1 may need to backfill the workers too. **Resolution:** if the statement HNSW is empty at phase-23-start, semantic retrieval over statement scope returns `Ok(vec![])`. Document this v1 limitation.
- **Risk:** Bidirectional BFS for Path mode is non-trivial. **Mitigation:** §4 caps `max_depth ≤ 5` (small graphs); single-direction BFS from `from` with early termination at `to` is acceptable for v1 if simpler. Document the choice in 23.2 plan; spec stays direction-agnostic on the algorithm.
- **Open question:** `RankedItemId` enum amendment — does 23.0 declare the new variants or leave them to 23.2? **Resolution:** 23.0 spec mentions `Entity | Relation` variants will be added (declarative); 23.2 commits the code (and amends §23/02 §1 in the same commit).

## 7. Test plan

This sub-task ships spec only — no executable tests. The
editorial verification gate:

- [ ] Each new spec file builds the cross-references it claims (every `§N/M §X` link resolves).
- [ ] Phase doc (`docs/phases/phase-23-hybrid-query.md`) reading list updated to cite §23/03 + §23/04.
- [ ] §16/02 §2.10 added; §2.10 → §2.11 renumber correct; phase-23 row in the gate list.
- [ ] Phase-23 master plan (`.claude/plans/phase-23.md`) exists.

## 8. Commit shape

Single commit:

```
docs(spec): §23/03 + §23/04 + perf targets + plan (23.0)

Phase 23.0 spec backfill — brings the semantic + graph
retrievers to phase-23 implementation depth alongside the
§23/02 lexical retriever from 22.0. §24/00 (hybrid query +
router + executor) is already at depth; no §24 backfill.

spec/23_retrievers/03_semantic_retriever.md (new):
  §1 Surface — SemanticRetriever trait, SemanticQuery
  { Vector | Text }, SemanticScope { Memory | Statement |
  Both }, SemanticFilters, SemanticRetrieverConfig.
  §2 Embedding semantics (D=384, brain-embed for Text).
  §3 HNSW search params (ef_search, similarity_threshold).
  §4 Scope dispatch.
  §5 Filter push-down via HNSW filter callback.
  §6 Returns + idempotency.
  §7 Errors.
  §8 Performance — §16/02 §2.10 targets.

spec/23_retrievers/04_graph_retriever.md (new):
  §1 Surface — GraphRetriever trait, GraphQuery { Star |
  Path | Subgraph }, Direction, config.
  §2 Proximity scoring — 1 / (hop_distance + 1).
  §3 Three modes (Star / Path / Subgraph) with traversal
  semantics.
  §4 Depth + branching caps (max_depth 3 default, 5 cap;
  branching 200, total top_k 64).
  §5 Pre-filter push-down (relation_types + direction).
  §6 Returns + idempotency — new RankedItemId variants
  (Entity / Relation) declared here, implemented in 23.2.
  §7 Errors.
  §8 Performance — §16/02 §2.10 targets.

spec/16_benchmarks_acceptance/02_latency_targets.md:
  §2.10 Hybrid query targets — three-retriever p50/p99
  with push-down vs post-fusion filters; EXPLAIN cost;
  filter chain; RRF fusion.
  §2.10 → §2.11 phase perf gates; phase-23 row added.

.claude/plans/phase-23.md (new): master plan indexing
sub-tasks 23.0–23.12, scope cuts (learned routing post-v1,
streaming results post-v1 if hybrid query gets used through
SUBSCRIBE; cross-shard fan-out documented as a phase-23
deliverable but execution depth left to 23.7).

.claude/plans/phase-23-task-00.md (new): this plan.
```

## 9. Confirmation

Please confirm:

1. **Two new spec files only** (§23/03 + §23/04) — §24 stays inline at the depth §24/00 already has.
2. **§16/02 §2.10 numbers** above are the right ballpark (10 ms / 50 ms hybrid 3-retriever matches phase doc 23.12).
3. **`RankedItemId::Entity / Relation` variants** declared in 23.0 spec, implemented in 23.2 code (amends §23/02 §6 in lockstep at code time).
4. **Statement HNSW v1 limitation** (empty corpus if no embedding worker yet) acknowledged inline in §23/03.
5. **Single commit shape** matches 22.0 / 21.0 precedent.

After approval: write the three spec changes + master plan + this plan, run a final spec-link sanity pass, commit on `feature/phase-23-hybrid-query`.
