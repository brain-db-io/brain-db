# Phase 23: Hybrid Query Engine ✓

## Status

**Complete** — tag `phase-23-complete`. Thirteen sub-tasks (23.0–23.12) landed on `feature/phase-23-hybrid-query`. Per-sub-task plans live under [`.claude/plans/phase-23-task-0[0-9].md`](../../.claude/plans/) and `phase-23-task-1[0-2].md`; each captures the trade-offs that sub-task took.

## Goal

Implement the query router, RRF fusion, filter chain, and full hybrid query execution. EXPLAIN and TRACE work. RECALL transparently uses hybrid retrieval when a schema is declared.

## Prerequisites

- Phases 17, 21, and 22 complete.

## Reading list

- [`spec/13_retrievers/00_purpose.md`](../../spec/13_retrievers/00_purpose.md) (full)
- [`spec/13_retrievers/01_rrf_fusion.md`](../../spec/13_retrievers/01_rrf_fusion.md)
- [`spec/13_retrievers/03_semantic_retriever.md`](../../spec/13_retrievers/03_semantic_retriever.md) — landed in 23.0.
- [`spec/13_retrievers/04_graph_retriever.md`](../../spec/13_retrievers/04_graph_retriever.md) — landed in 23.0.
- [`spec/13_retrievers/05_hybrid_query.md`](../../spec/13_retrievers/05_hybrid_query.md)
- [`spec/19_benchmarks/02_performance_targets.md`](../../spec/19_benchmarks/02_performance_targets.md) §2.10 — landed in 23.0.
- [`spec/04_wire_protocol/09_typed_graph_admin.md`](../../spec/04_wire_protocol/09_typed_graph_admin.md) §5 — RECALL transparent-routing contract.

## Outputs

- [x] §13/03 (`SemanticRetriever`), §13/04 (`GraphRetriever`), §02/02 §2.10 (hybrid perf targets) brought to phase-23 implementation depth.
- [x] `SemanticRetriever` trait + `BrainSemanticRetriever` (Memory + Statement scopes; push-down filters via `SemanticFilters`).
- [x] `GraphRetriever` trait + `BrainGraphRetriever` (Star / Path / Subgraph queries).
- [x] Rule-based query router (5 routing rules from §13/05).
- [x] Reciprocal Rank Fusion (`fuse_rrf` with `k = 60` default).
- [x] Post-fusion filter chain (type / temporal / confidence / tombstone / supersession) reading metadata via a single redb `ReadTransaction`.
- [x] Query planner (`plan(req)`) producing an immutable `QueryPlan` DAG.
- [x] Query executor (`execute(plan, req, ctx)`) — sequential retriever invocation, soft timeout, RRF fuse, filter chain, project.
- [x] `render_plan` (EXPLAIN) + `render_trace` (TRACE) renderers.
- [x] Four wire opcodes (`QUERY 0x0160`, `QUERY_EXPLAIN 0x0161`, `QUERY_TRACE 0x0162`, `RECALL_HYBRID 0x0163`) with rkyv-archivable types.
- [x] Fluent SDK query builder (`client.query()` with `.execute()` / `.explain()` / `.trace()`); no `recall_hybrid` SDK verb (kept domain-only).
- [x] Transparent hybrid routing on substrate `RECALL_REQ` when a schema is declared (per-shard `SchemaGate(Arc<ArcSwap<bool>>)`).
- [x] `MemoryResult.contributing_retrievers` + `MemoryResult.fused_score` wire fields populated by the hybrid path.

## Sub-tasks

### 23.0 §13/03 + §13/04 + §02/02 §2.10 spec backfill ✓

**Landed in:** [`.claude/plans/phase-23-task-00.md`](../../.claude/plans/phase-23-task-00.md).
**Done when:** three spec files at phase-23 implementation depth; phase doc reading list links them.

### 23.1 SemanticRetriever impl ✓

**Landed in:** [`.claude/plans/phase-23-task-01.md`](../../.claude/plans/phase-23-task-01.md).
**Done when:** trait + types in `brain-index` (macOS-buildable); impl in `brain-ops::ops::semantic_retriever` (Linux-only). Memory + Statement + Both scopes; push-down filters via `SemanticFilters`.

### 23.2 GraphRetriever impl ✓

**Landed in:** [`.claude/plans/phase-23-task-02.md`](../../.claude/plans/phase-23-task-02.md).
**Done when:** trait + types in `brain-index`; impl in `brain-ops::ops::graph_retriever`. Star + Path + Subgraph queries; relation-type push-down; bounded branching / depth.

### 23.3 Query router (rule-based) ✓

**Landed in:** [`.claude/plans/phase-23-task-03.md`](../../.claude/plans/phase-23-task-03.md).
**Done when:** `brain-planner::knowledge::router::route(req)` implements the 5 routing rules from §13/05; emits `RoutingDecision` with retrievers + weights + `temporal_pushdown` hint.

### 23.4 RRF fusion ✓

**Landed in:** [`.claude/plans/phase-23-task-04.md`](../../.claude/plans/phase-23-task-04.md).
**Done when:** `fuse_rrf(outputs, k, weights)` implements `Σ w_i / (k + rank_i)`; stable sort with deterministic tie-break by (kind, bytes); `DEFAULT_K = 60`.

### 23.5 Filter chain ✓

**Landed in:** [`.claude/plans/phase-23-task-05.md`](../../.claude/plans/phase-23-task-05.md).
**Done when:** type → temporal → confidence → tombstone → supersession order; single redb `ReadTransaction` shared across the five filters; per-step `FilterChainStats` for TRACE.

### 23.6 Query planner ✓

**Landed in:** [`.claude/plans/phase-23-task-06.md`](../../.claude/plans/phase-23-task-06.md).
**Done when:** `plan(req) -> Result<QueryPlan, PlanError>`; routes via 23.3; expands into `PlannedRetriever` configs with per-retriever defaults and a single pre-filter push-down per retriever (temporal > predicate > kind precedence in v1).

### 23.7 Query executor ✓

**Landed in:** [`.claude/plans/phase-23-task-07.md`](../../.claude/plans/phase-23-task-07.md).
**Done when:** `execute(plan, req, ctx) -> Result<QueryResult, ExecutionError>` runs each retriever sequentially under Glommio's single-threaded executor, soft-timeout post-hoc, RRF fuse, filter chain, project to `Vec<FusedItem>` + `QueryMetadata`.

### 23.8 EXPLAIN + TRACE renderers ✓

**Landed in:** [`.claude/plans/phase-23-task-08.md`](../../.claude/plans/phase-23-task-08.md).
**Done when:** `render_plan(plan)` and `render_trace(plan, metadata)` produce monospace text blocks with PLAN / RETRIEVERS / FUSION / POST_FILTERS / LIMIT / EXECUTION sections per §13/05 §"Plan structure".

### 23.9 Wire opcodes 0x0160-0x0163 ✓

**Landed in:** [`.claude/plans/phase-23-task-09.md`](../../.claude/plans/phase-23-task-09.md).
**Done when:** `Query / QueryExplain / QueryTrace / RecallHybrid` opcode pairs in `brain-protocol::opcode`; wire request/response types in `brain-protocol::knowledge::query`; handlers + dispatch in `brain-ops::ops::knowledge_query`; six integration smoke tests.

### 23.10 SDK fluent query builder ✓

**Landed in:** [`.claude/plans/phase-23-task-10.md`](../../.claude/plans/phase-23-task-10.md).
**Done when:** `Client::query()` returns `QueryBuilder` with `.execute()` / `.explain()` / `.trace()`; SDK-owned domain types with real methods (no `pub use X as Y` aliasing); validation at `.execute()` so setters stay infallible. No `client.recall_hybrid` verb — domain verbs only.

### 23.11 RECALL transparent hybrid ✓

**Landed in:** [`.claude/plans/phase-23-task-11.md`](../../.claude/plans/phase-23-task-11.md).
**Done when:** per-shard `SchemaGate(Arc<ArcSwap<bool>>)` seeded from metadata; `handle_schema_upload` flips it post-commit; `handle_recall` routes through the hybrid pipeline when the gate is set AND no txn is attached; falls back to substrate on `MissingRetriever`. Substrate `MemoryResult` gains `contributing_retrievers` + `fused_score`; txn'd recalls stay substrate (hybrid + RYW deferred to post-v1).

### 23.12 Phase exit ✓

**Landed in:** [`.claude/plans/phase-23-task-12.md`](../../.claude/plans/phase-23-task-12.md).
**Done when:** 4 phase-exit integration tests via the TCP wire path; 3 criterion benches at 10K corpus scale against §02/02 §2.10; ROADMAP + this phase doc updated; §00 open-questions appended; `phase-23-complete` tag cut.

## Done-when (phase)

- [x] Hybrid query end-to-end works (`QUERY`, `RECALL_HYBRID`, and transparent `RECALL`).
- [x] Router picks reasonable retrievers per query class (5 rules in §13/05).
- [x] RRF fusion correct (score-scale-invariant, deterministic ties).
- [x] EXPLAIN / TRACE useful for debugging (planner sections + per-retriever execution metrics).
- [x] Substrate RECALL transparently uses hybrid when a schema is present.
- [ ] **Performance budgets validated at production scale** — phase 14 acceptance suite. v1 phase-23 benches operate at 10K scale (regression detection).

## Scope cuts

| Cut | Where it goes | Reason |
|---|---|---|
| Streaming hybrid query results (limit > 100) | Post-v1 — see §00 OQ-23-A | v1 returns a single `QueryResponse`; the SUBSCRIBE path for streaming was not in scope. |
| Hybrid recall + transactional read-your-writes | Post-v1 — see §00 OQ-23-B | Lens layering across statements + relations is multi-week work. v1 txn'd RECALL stays on the substrate path; spec §05/08 §5 only covers substrate semantics. |
| Filter-only retriever mode (no text, no anchor) | Post-v1 — see §00 OQ-23-C | The planner returns `NoSignal` when neither text nor anchor is supplied. A "find by filters only" mode requires a new "everything" retriever. |
| Learned router on top of rule-based | Future versions — `OQ-V2-1` + §00 OQ-23-D | Need labeled query traffic to train. Rule-based stays as fallback. |
| Cross-shard hybrid result merging | Post-v1 — see §00 OQ-23-E | v1 routing is per-shard; the connection layer's fan-out lives upstream of the hybrid engine. |
| `MemoryResult.text` population on the hybrid path | Matches existing substrate behaviour (text only when caller requests) | Hybrid projection leaves `text = ""`; substrate had the same default. `include_text` future work covers both paths. |
| Parallel retriever execution | v1 sequential per §23.7 plan | Retriever traits are sync; async-trait migration deferred. §02/02 §2.10 headroom comfortable (3 × 10 ms vs 50 ms p99). |

## Phase exit

- [x] Sub-tasks 23.0–23.12 landed on `feature/phase-23-hybrid-query`.
- [x] All scope cuts documented in this file + ROADMAP.
- [x] Workspace `cargo zigbuild --target x86_64-unknown-linux-gnu --workspace --tests` green at tag time.
- [x] `cargo clippy --target x86_64-unknown-linux-gnu -p brain-planner -p brain-protocol -p brain-ops -p brain-server -p brain-sdk-rust --all-targets -- -D warnings` clean.
- [x] criterion bench harness in place (`crates/brain-planner/benches/hybrid_query.rs`); wall-time capture deferred to phase-14 acceptance suite (matching the 21.7 / 22.8 precedent).
- [x] Tag `phase-23-complete` cut.

## Pitfalls

- Don't over-tune the router; the rule-based router is a sensible-default, not optimal. Learned routing is `OQ-V2-1`.
- Fusion `k=60` is documented in `§13/01 §"Choice of k"`; per-query override rides on `FusionConfig`.
- Push-down filter optimization: temporal goes down into retrievers per the routing decision; everything else applies post-fusion. Don't chase additional push-downs in v1.
- The hybrid path doesn't fetch `MemoryResult.text` inline; clients that need text use the existing `include_text` path on substrate RECALL or `STATEMENT_GET` / `ENTITY_GET` to hydrate.
- `MissingRetriever` from the executor (e.g. operator left the lexical slot empty) falls back to substrate RECALL with a warn log; don't fail the request.
