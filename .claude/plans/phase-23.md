# Plan: Phase 23 — Hybrid Query Engine

**Status:** awaiting-confirmation (master plan; per-sub-task plans land 23.1+)
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 13 (one per sub-task 23.0–23.12)

---

## 1. Scope

Light up the hybrid query path of the knowledge layer:

- `SemanticRetriever` over memory + statement HNSW (23.1).
- `GraphRetriever` over the entity-relation-statement graph
  (23.2).
- Rule-based router with 5 routing rules over the §24/00
  classification features (23.3).
- RRF fusion (`k = 60`) over multi-retriever ranked lists
  (23.4).
- Filter chain (type / temporal / confidence / tombstone /
  supersession) with push-down (23.5).
- Query planner + executor producing a parallel DAG, with
  EXPLAIN / TRACE for debugging (23.6 + 23.7 + 23.8).
- Wire opcodes 0x60–0x63 + SDK query builder (23.9 + 23.10).
- RECALL transparent hybrid mode when a schema is declared
  (23.11).
- Integration tests + criterion benches + phase exit + tag
  `phase-23-complete` (23.12).

Phase 23 does NOT:

- Implement learned routing (a small classifier trained on
  labeled queries) — explicit post-v1 cut documented inline
  in §24/00.
- Implement streaming hybrid queries for `limit > 100` — the
  §24/00 spec calls it out, but v1 batches the response;
  streaming is post-v1 (§29 SDK plan owns the surface).
- Implement cross-shard fan-out — per-shard hybrid query in
  v1; multi-shard router lives in the connection layer and is
  parked behind a feature flag for phase 24+.

## 2. Spec anchors (post-23.0)

| Section | Status after 23.0 |
|---|---|
| `spec/23_retrievers/03_semantic_retriever.md` | ✓ written |
| `spec/23_retrievers/04_graph_retriever.md` | ✓ written |
| `spec/16_benchmarks_acceptance/02_latency_targets.md` §2.10 | ✓ amended |
| `spec/24_hybrid_query/00_purpose.md` | ✓ already at depth (no backfill needed) |
| `spec/23_retrievers/01_rrf_fusion.md` | ✓ already at depth |

## 3. Sub-tasks

| # | Title | Plan | Crates |
|---|---|---|---|
| 23.0 | §23/03 + §23/04 + perf-targets spec backfill | `phase-23-task-00.md` (✓ awaiting approval) | spec + plans only |
| 23.1 | `SemanticRetriever` trait + tantivy-style impl over memory + statement HNSW | `phase-23-task-01.md` | `brain-index` (+ statement embedding worker bring-up if corpus is empty) |
| 23.2 | `GraphRetriever` trait + impl over redb entity / relation / statement tables | `phase-23-task-02.md` | `brain-index` (or `brain-planner`; deferred to plan) |
| 23.3 | Query router (5 rules over §24/00 features; NER + temporal-expression detection) | `phase-23-task-03.md` | `brain-planner` |
| 23.4 | RRF fusion (`fuse_rrf(outputs, k, weights) -> Vec<FusedItem>`) | `phase-23-task-04.md` | `brain-planner` |
| 23.5 | Filter chain (5 filter classes; push-down where supported) | `phase-23-task-05.md` | `brain-planner` |
| 23.6 | Query planner (request → `QueryPlan` DAG; pre-filter compute; cost estimate) | `phase-23-task-06.md` | `brain-planner` |
| 23.7 | Query executor (parallel retriever fan-out + timeout + fusion + filters → `QueryResult`) | `phase-23-task-07.md` | `brain-planner` |
| 23.8 | EXPLAIN + TRACE (plan-only + plan + execution metadata) | `phase-23-task-08.md` | `brain-planner` + `brain-protocol` |
| 23.9 | Wire opcodes 0x60–0x63 (QUERY / QUERY_EXPLAIN / QUERY_TRACE / RECALL_HYBRID) | `phase-23-task-09.md` | `brain-protocol`, `brain-ops`, `brain-server` |
| 23.10 | SDK query builder (`client.query()...execute()`) | `phase-23-task-10.md` | `brain-sdk-rust` |
| 23.11 | RECALL transparent hybrid mode | `phase-23-task-11.md` | `brain-ops` |
| 23.12 | Integration tests + criterion benches + phase exit + tag `phase-23-complete` | `phase-23-task-12.md` | tests + ROADMAP + tag |

## 4. Scope cuts

| Cut | Where it goes | Reason |
|---|---|---|
| Learned routing (small classifier over labeled queries) | Post-v1 (§24/00 §"Learned routing" already documents) | Rule-based router suffices for the v1 query shape; learned routing needs labeled data we don't yet have. |
| Streaming hybrid results (limit > 100) | Post-v1 | §24/00 §"Streaming results" describes the SUBSCRIBE-based path; v1 batches. |
| Cross-shard fan-out | Phase 24+ | v1 hybrid query is per-shard; the connection layer's existing per-shard dispatch is the v1 boundary. |
| Statement HNSW corpus backfill via background worker | Lands in 23.1 plan if needed; may be punted to 23.12 if the worker bring-up is non-trivial | The statement embedding worker is referenced in §27/00 but the impl hasn't landed. |
| Filter push-down through statement-by-predicate redb index | Phase 23.5 polish if simple; otherwise post-v1 | §24/00 §"Filter as retriever vs filter" hints at it; not blocking for v1 correctness. |
| Per-retriever weight tuning data | Post-v1 | Equal weights default; tunable via config. §23/01 §"Per-retriever weights" documents the path. |
| Bidirectional BFS for `GraphQuery::Path` | Post-v1 | §23/04 §3 specifies single-source BFS with early termination; bidirectional is the optimisation. |

## 5. Risks

| Risk | Mitigation |
|---|---|
| `hnsw_rs` filter callback API doesn't match §23/03 §5's push-down model | §23/03 §5 explicitly allows fall-back to post-search filtering; 23.1 plan picks the impl based on actual API. |
| Statement HNSW corpus is empty at phase-23 start | §23/03 §9 documents the v1 limitation; 23.1 plan decides whether to wire the embedding worker now or stage. |
| Router NER quality is poor on short queries | The §24/00 features list classifies "Text contains entity names" as one signal of many; even with imperfect NER, the router degrades gracefully (Default rule covers free-text). |
| Filter push-down complexity (5 filter classes × N retrievers) | 23.5 plan grades push-down per filter; the safe fallback is post-fusion filter (still correct, slightly slower). §24/00 §"Filter chain" already documents this. |
| Cross-cutting cost-estimate accuracy in the planner | 23.6 plan defines a simple linear cost model; production-tuned cost models are post-v1. |

## 6. Verification gate (phase exit, 23.12)

- All 23.0–23.11 commits land on `feature/phase-23-hybrid-query`.
- `cargo zigbuild --target x86_64-unknown-linux-gnu --workspace --tests` clean.
- `cargo clippy -p brain-planner -p brain-index -p brain-ops -p brain-server --tests -- -D warnings` clean.
- `cargo bench -p brain-planner --bench hybrid_query -- --quick` meets §16/02 §2.10 targets at 10K corpus scale (or the gap is explicitly recorded in ROADMAP).
- Integration test surface in `crates/brain-server/tests/knowledge_hybrid_query_phase_exit.rs` covers each of the 5 routing rules.
- Tag `phase-23-complete` (annotated).

## 7. Tagging discipline

Each sub-task: one commit, descriptive message. No squashing,
no merge commits, no `Co-Authored-By` trailer. Phase-exit tag
is annotated and points to the 23.12 commit.

---

After 23.0 is approved and committed, I draft
`phase-23-task-01.md` for the SemanticRetriever sub-task.
