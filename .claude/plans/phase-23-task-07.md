# Plan: Phase 23 — Task 07, Query executor

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1

---

## 1. Scope

Implement the executor side of the hybrid query pipeline.
Takes a `QueryPlan` (23.6) and a context carrying the three
retriever handles, invokes each retriever per its `PlannedRetriever`
config, fuses (23.4), applies the post-fusion filter chain
(23.5), truncates to `limit`, and returns a `QueryResult`
with per-retriever latency + result-count metadata for
EXPLAIN/TRACE (23.8).

Concrete deliverables:

1. New module `crates/brain-planner/src/knowledge/executor.rs`:
   - `HybridExecutorContext { semantic, lexical, graph,
     metadata }` — bundle of retriever handles + the
     metadata reader the filter chain needs.
   - `QueryResult { items: Vec<FusedItem>, metadata:
     QueryMetadata }`.
   - `QueryMetadata { plan_summary, retriever_latencies_ms,
     retriever_total_results, filter_stats,
     total_latency_ms }`.
   - `execute(plan, request, ctx) -> Result<QueryResult,
     ExecutionError>`.
2. **Sequential per-retriever invocation in v1**. §24/00
   §"Execution" specifies parallel; the retriever traits are
   sync (no async `retrieve` method), and brain-planner's
   per-shard Glommio executor is single-threaded — so v1
   runs the three retrievers in sequence. Wall-time at the
   §16/02 §2.10 target scale: max ~30 ms across three
   retrievers, well under the 50 ms p99 budget for hybrid
   3-retriever queries. Parallel execution lands post-v1
   (requires async-trait migration of retrievers).
3. **Per-retriever timeout** — each retriever call wrapped in
   a wall-time check; if it exceeds `config.timeout_ms`, the
   executor logs a warn + records `Err::Timeout` in
   `retriever_latencies_ms` for that retriever and proceeds
   with an empty result for it (matches §24/00 §"Execution"
   "Err(TimeoutError) → partial; proceed with what we have").
4. **Per-retriever query construction**:
   - Semantic: `SemanticQuery::Text(req.text)` (or
     `Vector(req.vector)` post-v1); `SemanticScope::Both`
     when both `text` and `entity_anchor` set, else Memory
     scope.
   - Lexical: `LexicalQuery { terms: tokenize(text),
     phrase_clauses: vec![], filters: from_pre_filter }`;
     scope = `LexicalScope::MemoryText` (statement-text
     scope when an entity anchor is present — the router
     hasn't differentiated yet, so v1 sticks to MemoryText).
   - Graph: `GraphQuery::Star { anchor: req.entity_anchor,
     depth: config.max_depth, direction, relation_types,
     include_statements }` — only if `entity_anchor` is set.
     If not set, the graph retriever is skipped (the
     `PreFilter::AgentId` pre-filter doesn't help without an
     anchor).
5. **Pre-filter translation** — `PreFilter::Temporal(range)`
   maps to the retriever-native filter field
   (`SemanticFilters.created_at_ms` / `LexicalFilters.created_at_ms` /
   no equivalent in `GraphQuery`). `PreFilter::PredicateId(_)`
   maps to `SemanticFilters.predicate_id` (single value;
   if multiple, the first; remaining filtered post-fusion).
   `PreFilter::StatementKind(_)` similarly.
6. **Empty retriever results** — if a retriever returns
   `Ok(vec![])` (e.g. statement HNSW empty in v1), the fused
   list gets contributions only from the other two
   retrievers.
7. **Filter chain application** — calls
   `apply_filter_chain(items, &plan.post_filters, &ctx.metadata,
   plan.limit)`. Stats land in `QueryMetadata.filter_stats`.
8. Unit tests against a `HybridExecutorContext` with mock
   retriever handles (small trait impls in the test that
   return canned `Vec<RankedItem>`).

NOT in scope:
- Parallel retriever execution — v1 sequential.
- Streaming results (limit > 100) — post-v1.
- Per-query cost budget enforcement — the planner emits an
  estimate; v1 logs if execution exceeds the estimate but
  doesn't reject.
- EXPLAIN/TRACE rendering — 23.8.
- Cross-shard fan-out — router scope.

## 2. Spec references

- `spec/24_hybrid_query/00_purpose.md` §"Execution" + §"Result
  shape" — binding for the executor flow and `QueryResult`
  shape.
- `spec/24_hybrid_query/00_purpose.md` §"Streaming results"
  — explicit post-v1 cut.
- `spec/23_retrievers/01_rrf_fusion.md` — fusion contract.

## 3. External validation

| Item | Source | Confirmed |
|---|---|---|
| Retriever trait signatures | `brain-index::{SemanticRetriever, LexicalRetriever, GraphRetriever}` | All `retrieve(&self, query, config) -> Result<Vec<RankedItem>, Error>` — sync. |
| `apply_filter_chain` returns `(Vec<FusedItem>, FilterChainStats)` | `brain-planner::knowledge::filters` (23.5) | Yes. |
| `fuse_rrf` signature | `brain-planner::knowledge::fusion` (23.4) | `fuse_rrf(&[(Retriever, Vec<RankedItem>)], k, weights) -> Vec<FusedItem>`. |
| `MetadataDb` access via `Arc<Mutex<MetadataDb>>` | `brain-ops::OpsContext.executor.metadata` (already used by 23.5's tests) | Yes. |

## 4. Architecture sketch

### Types

```rust
// crates/brain-planner/src/knowledge/executor.rs

use std::sync::Arc;
use std::time::Instant;

use brain_index::{
    GraphRetriever, LexicalQuery as IxLexicalQuery, LexicalRetriever, LexicalScope,
    RankedItem, SemanticQuery as IxSemanticQuery, SemanticRetriever, SemanticScope,
};
use brain_metadata::MetadataDb;
use parking_lot::Mutex;

use super::filters::{apply_filter_chain, FilterChainStats};
use super::fusion::{fuse_rrf, FusedItem};
use super::planner::{PreFilter, QueryPlan, Retriever, RetrieverConfig};
use super::router::QueryRequest;

/// Context the executor needs in addition to a `QueryPlan`.
/// Built from `OpsContext`'s retriever slots in the caller.
#[derive(Clone)]
pub struct HybridExecutorContext {
    pub semantic: Option<Arc<dyn SemanticRetriever>>,
    pub lexical: Option<Arc<dyn LexicalRetriever>>,
    pub graph: Option<Arc<dyn GraphRetriever>>,
    pub metadata: Arc<Mutex<MetadataDb>>,
}

#[derive(Debug, Clone)]
pub struct QueryResult {
    pub items: Vec<FusedItem>,
    pub metadata: QueryMetadata,
}

#[derive(Debug, Clone, Default)]
pub struct QueryMetadata {
    pub retriever_latencies_ms: Vec<(Retriever, f64)>,
    pub retriever_outcomes: Vec<RetrieverOutcome>,
    pub retriever_total_results: Vec<(Retriever, usize)>,
    pub filter_stats: FilterChainStats,
    pub total_latency_ms: f64,
}

#[derive(Debug, Clone)]
pub struct RetrieverOutcome {
    pub retriever: Retriever,
    pub status: RetrieverStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetrieverStatus {
    Success,
    Skipped(&'static str),  // e.g. "no anchor for Graph"
    Timeout,
    Failure(String),
}

#[derive(Debug, thiserror::Error)]
pub enum ExecutionError {
    #[error("missing retriever handle: {0:?}")]
    MissingRetriever(Retriever),
    #[error("filter chain: {0}")]
    Filter(#[from] super::filters::FilterError),
    #[error("internal: {0}")]
    Internal(String),
}
```

### `execute` entry point

```rust
pub fn execute(
    plan: &QueryPlan,
    request: &QueryRequest,
    ctx: &HybridExecutorContext,
) -> Result<QueryResult, ExecutionError> {
    let total_started = Instant::now();
    let mut outputs: Vec<(Retriever, Vec<RankedItem>)> = Vec::new();
    let mut latencies: Vec<(Retriever, f64)> = Vec::new();
    let mut outcomes: Vec<RetrieverOutcome> = Vec::new();
    let mut totals: Vec<(Retriever, usize)> = Vec::new();

    for planned in &plan.retrievers {
        let started = Instant::now();
        let result = invoke_retriever(planned, request, ctx);
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        latencies.push((planned.retriever, elapsed_ms));

        match result {
            Ok(items) => {
                totals.push((planned.retriever, items.len()));
                outcomes.push(RetrieverOutcome {
                    retriever: planned.retriever,
                    status: RetrieverStatus::Success,
                });
                outputs.push((planned.retriever, items));
            }
            Err(RetrieverInvocationError::Skipped(reason)) => {
                totals.push((planned.retriever, 0));
                outcomes.push(RetrieverOutcome {
                    retriever: planned.retriever,
                    status: RetrieverStatus::Skipped(reason),
                });
            }
            Err(RetrieverInvocationError::Failure(msg)) => {
                tracing::warn!(
                    target: "brain_planner::executor",
                    ?planned.retriever, error = %msg,
                    "retriever failed; continuing with partial results",
                );
                totals.push((planned.retriever, 0));
                outcomes.push(RetrieverOutcome {
                    retriever: planned.retriever,
                    status: RetrieverStatus::Failure(msg),
                });
            }
        }

        // Per-retriever timeout check — soft. If we blew the
        // budget on this retriever, record it but proceed.
        if let RetrieverConfig::Semantic { timeout_ms, .. }
        |  RetrieverConfig::Lexical  { timeout_ms, .. }
        |  RetrieverConfig::Graph    { timeout_ms, .. } = &planned.config
        {
            if elapsed_ms > f64::from(*timeout_ms) {
                if let Some(last) = outcomes.last_mut() {
                    if matches!(last.status, RetrieverStatus::Success) {
                        last.status = RetrieverStatus::Timeout;
                    }
                }
            }
        }
    }

    let fused = fuse_rrf(&outputs, plan.fusion.k, &plan.fusion.weights);
    let (items, filter_stats) = {
        let metadata_guard = ctx.metadata.lock();
        apply_filter_chain(fused, &plan.post_filters, &metadata_guard, plan.limit)?
    };

    let total_latency_ms = total_started.elapsed().as_secs_f64() * 1000.0;
    Ok(QueryResult {
        items,
        metadata: QueryMetadata {
            retriever_latencies_ms: latencies,
            retriever_outcomes: outcomes,
            retriever_total_results: totals,
            filter_stats,
            total_latency_ms,
        },
    })
}
```

### Per-retriever invocation

```rust
enum RetrieverInvocationError {
    Skipped(&'static str),
    Failure(String),
}

fn invoke_retriever(
    planned: &PlannedRetriever,
    req: &QueryRequest,
    ctx: &HybridExecutorContext,
) -> Result<Vec<RankedItem>, RetrieverInvocationError> {
    match planned.retriever {
        Retriever::Semantic => invoke_semantic(planned, req, ctx),
        Retriever::Lexical => invoke_lexical(planned, req, ctx),
        Retriever::Graph => invoke_graph(planned, req, ctx),
    }
}
```

Each `invoke_*` builds the per-retriever query from the
request + the planner's config + pre-filter, calls the
trait, maps the error.

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Sequential per-retriever invocation (this plan) | Sync retriever traits stay sync; no executor-runtime gymnastics; 3-retriever total stays < 50 ms target | Doesn't match §24/00 §"Execution" parallel intent | ✓ for v1 — parallel post-v1 |
| Make retriever traits async; use `futures::join_all` | Matches spec | Touches every retriever impl + every test; v1 perf doesn't need it | rejected — defer |
| `tokio::task::spawn_blocking` for parallelism | Bypasses async-trait migration | brain-planner doesn't depend on tokio; runs in Glommio | rejected |
| Hard timeout via `tokio::time::timeout` | Strict | Same dep problem; mid-call cancellation needs cooperative retrievers | rejected — v1 records timeout post-hoc |
| `RetrieverStatus::Skipped(&'static str)` (this plan) | Static reasons (no anchor / no text); cheap | Less ergonomic if reasons grow | ✓ — small reason set in v1 |
| Hold `ctx.metadata` lock across retrievers + filter | Single lock acquisition | Locks the metadata DB across retriever calls — retrievers don't always need it; better to lock just at filter time | rejected — lock per filter chain call (already 23.5's pattern) |

## 6. Risks / open questions

- **Risk:** Soft timeout — a runaway retriever can blow well past its `timeout_ms`. **Mitigation:** record post-hoc, log warn, proceed with partial. Hard timeout requires cancellation infrastructure (cooperative `Stop` flag in retrievers); deferred.
- **Risk:** Graph retriever requires `entity_anchor`; if not present, skipping silently is correct but operators may wonder. **Mitigation:** `RetrieverStatus::Skipped("no anchor")` records the skip explicitly; surfaces in TRACE.
- **Open question:** Lexical scope choice. §22.5 `LexicalRetriever` supports both `MemoryText` and `StatementText`. V1 always queries `MemoryText` — statement-text lexical retrieval is conceptually parallel and would need a separate `PlannedRetriever` entry. v1 keeps it simple; phase 24+ adds StatementText routing.
- **Open question:** Semantic scope choice. Same as lexical — v1 picks `SemanticScope::Both` when both text and anchor present; Memory-only otherwise. The 23.6 plan doesn't currently encode scope in `RetrieverConfig::Semantic`; we add it inline as a v1 default.
- **Open question:** Pre-filter to `LexicalFilters` translation for multi-value `predicate_filter`. v1 takes the first value; rest go to post-fusion. Documented.

## 7. Test plan

Unit tests in `crates/brain-planner/src/knowledge/executor/tests.rs`:

- `executes_single_semantic_retriever` — plan with one
  retriever (mock returns 3 hits), runs end-to-end, returns
  3 items + metadata.
- `executes_three_retrievers_fuses_results` — three mock
  retrievers, partially overlapping ids → fused result
  ranked by RRF score.
- `skips_graph_when_no_anchor` — plan has Graph but
  request lacks `entity_anchor` → status Skipped("no anchor")
  with 0 results, executor proceeds.
- `failing_retriever_returns_partial_results` — mock returns
  Err → status Failure(...), other retrievers still
  contribute.
- `timeout_records_status` — mock that sleeps > timeout →
  status Timeout (post-hoc).
- `filter_chain_applied` — pass items that include a
  tombstoned statement; mock metadata; filter chain drops it.
- `limit_truncates_after_filters` — 10 fused items, limit 3
  → exactly 3 items returned; filter_stats.after_supersession
  reflects the pre-limit count.
- `query_metadata_records_per_retriever_latency` — each
  retriever entry in `metadata.retriever_latencies_ms`
  carries a non-negative ms value.
- `total_latency_ms_is_sum_or_greater` — total ≥ sum of
  per-retriever latencies (we measure inclusive of fusion +
  filter).
- `missing_retriever_handle_errors` — plan calls for Semantic
  but `ctx.semantic = None` → ExecutionError::MissingRetriever.

The mock retrievers are minimal `impl SemanticRetriever for
MockSem { ... }` etc., constructed inline in each test.

## 8. Commit shape

Single commit:

```
feat(planner): 23.7 — hybrid query executor

- crates/brain-planner/src/knowledge/executor.rs (new):
  HybridExecutorContext (Arc handles to the three
  retrievers + MetadataDb), QueryResult { items: Vec<FusedItem>,
  metadata: QueryMetadata }, QueryMetadata (per-retriever
  latencies + outcomes + result counts; filter_stats;
  total_latency_ms), RetrieverStatus { Success | Skipped |
  Timeout | Failure }, ExecutionError.
  `execute(plan, request, ctx) -> Result<QueryResult,
  ExecutionError>`: sequential per-retriever invocation,
  RRF fusion (23.4), post-fusion filter chain (23.5),
  limit truncation. Soft timeout via post-hoc latency
  check.
- crates/brain-planner/src/knowledge/executor/tests.rs (new):
  ~10 unit tests with mock retriever trait impls covering
  the happy path, skips/failures/timeouts, filter chain
  integration, limit, missing-handle error.
- crates/brain-planner/src/knowledge/mod.rs: pub mod executor.
```

## 9. Confirmation

Please confirm:

1. **Sequential per-retriever invocation in v1** (vs parallel) — defers the async-trait migration of retrievers; perf budget at §16/02 §2.10 has headroom.
2. **Soft timeout** (post-hoc latency check + `RetrieverStatus::Timeout`) — vs hard cancellation that needs cooperative retrievers.
3. **Graph retriever silently skipped** when no anchor; surfaces in `RetrieverStatus::Skipped("no anchor")`.
4. **`LexicalScope::MemoryText` always** in v1 (statement-text lexical routing post-v1).
5. **Pre-filter single-value mapping** for multi-value filters (first value used; rest post-fusion).

After approval: implement + tests + commit.
