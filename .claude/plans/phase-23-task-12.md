# Plan: Phase 23 — Task 12, Phase exit

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1 (one `chore(...): 23.12 — phase 23 exit` commit)

---

## 1. Scope

Close phase 23. Same shape as the 21.7 / 22.8 phase-exit
template — one atomic commit that ships three threads:

1. **Integration test surface** — one focused phase-exit test
   file under `brain-server/tests/` that exercises the full
   hybrid query pipeline end-to-end through the wire-op
   dispatcher (schema → encode → query → assert
   per-retriever outcomes). Complements the per-sub-task
   wire tests already in the tree from 23.9 / 23.11.
2. **Criterion benches** against §16/02 §2.10 — three benches
   in `crates/brain-planner/benches/hybrid_query.rs`:
   - Hybrid 3-retriever end-to-end (the §2.10 headline
     number — 10 ms p50 / 50 ms p99 target).
   - Single-retriever router-degraded path (7 ms p50 / 30 ms
     p99).
   - EXPLAIN plan-only (500 µs p50 / 2 ms p99).
3. **Phase exit metadata**:
   - `ROADMAP.md` Phase 23 entry rewritten in the
     phase-21 / phase-22 template (Exit / Scope cuts /
     Delivered / Deferred / Bench).
   - `docs/phases/phase-23-hybrid-query.md` checkboxes
     flipped; explicit scope-cut callouts; phase-exit
     section added.
   - `spec/30_knowledge_open_questions/00_purpose.md`:
     new entries for deferrals introduced during 23.x
     (streaming results, hybrid-in-txn, filter-only
     retriever mode, learned routing path through 23.3).
   - Tag `phase-23-complete` (annotated) cut after the
     commit lands.

## 2. Spec references

- `spec/16_benchmarks_acceptance/02_latency_targets.md`
  §2.10 — the perf targets the benches validate (10 ms p50
  / 50 ms p99 hybrid; 500 µs p50 / 2 ms p99 EXPLAIN).
- `spec/24_hybrid_query/00_purpose.md` — already at
  implementation depth (landed in 23.0).
- `spec/23_retrievers/03_semantic_retriever.md` +
  `04_graph_retriever.md` — at depth from 23.0.

## 3. External validation

Not applicable. Phase exit is internal; no new libraries.

## 4. Architecture sketch

### Integration test

`crates/brain-server/tests/knowledge_hybrid_phase_exit.rs`
(matching the phase-20 / phase-21 / phase-22 file naming):

```rust
//! Phase 23 exit smoke. Drives the full hybrid pipeline through
//! the live wire-op dispatcher.

#[tokio::test(flavor = "current_thread")]
async fn hybrid_query_returns_indexed_memory() {
    // 1. spawn_shard(1).
    // 2. SCHEMA_UPLOAD trivial schema → gate flips.
    // 3. ENCODE three memories with distinct text.
    // 4. Poll-with-timeout (≤ 500 ms) until lexical retriever
    //    sees the docs (tantivy commit cadence).
    // 5. QUERY text="…term that matches one doc…", limit=10.
    // 6. Assert items.len() ≥ 1, retriever_outcomes covers the
    //    auto-router's picks, total_latency_ms > 0.
}

#[tokio::test(flavor = "current_thread")]
async fn hybrid_explain_renders_plan_text() {
    // 1. spawn_shard, declare schema.
    // 2. QUERY_EXPLAIN text="something".
    // 3. Assert plan_text contains "PLAN:" / "RETRIEVERS:" /
    //    "FUSION:" / "POST_FILTERS:" / "LIMIT:".
    // 4. estimated_cost_ms > 0.
}

#[tokio::test(flavor = "current_thread")]
async fn hybrid_trace_includes_execution_block() {
    // 1. Schema declared + 1 memory indexed.
    // 2. QUERY_TRACE.
    // 3. Assert trace_text contains "EXECUTION:" and per-
    //    retriever lines.
}

#[tokio::test(flavor = "current_thread")]
async fn recall_after_schema_routes_through_hybrid_pipeline() {
    // 1. Schema declared + 1 memory indexed.
    // 2. Substrate RECALL with cue text matching the memory.
    // 3. Assert at least one MemoryResult has non-empty
    //    `contributing_retrievers` and fused_score > 0.
    //    This is the transparent-routing contract from
    //    spec §28/08 §5.
}
```

These reuse the existing `support_harness::start(n_shards)`
+ `complete_handshake` + `round_trip` helpers already
duplicated across the per-sub-task test files. To stay
consistent with the rest of the test crate we copy the
helpers in once (matching the convention in `knowledge_*_wire.rs`).

### Benches

`crates/brain-planner/benches/hybrid_query.rs`. New
`[[bench]]` entry in `brain-planner/Cargo.toml`. Three
groups against §16/02 §2.10:

```rust
fn bench_hybrid_three_retriever(c: &mut Criterion) {
    // Setup (once, batched out of timed loop):
    //   - 10K memories indexed in tantivy + HNSW.
    //   - 1K statements with entity anchors.
    //   - 100 entities.
    // Bench: plan + execute over a text query + entity_anchor.
    // Target check: report p50/p99 against §16/02 §2.10
    //   (10 ms / 50 ms hybrid 3-retriever target).
}

fn bench_hybrid_router_degraded(c: &mut Criterion) {
    // Setup same as above.
    // Bench: text-only query (auto-router picks
    //   Semantic + Lexical, no Graph).
    // Target: §2.10 (7 ms p50 / 30 ms p99).
}

fn bench_explain_only(c: &mut Criterion) {
    // Bench: plan(req) without executor invocation.
    // Target: §2.10 (500 µs p50 / 2 ms p99).
}
```

10K is the same regression-detector scale we used for
phase 22. Spec §16/02 §2.10 explicitly notes 23.12 runs at
10K corpus / 3 retrievers, with production-scale (100K /
1M) deferred to phase-14 acceptance.

Per the 21.7 / 22.8 precedent, **bench wall-time numbers
are optional at the tag** — the harness lives in the tree;
the user can run them when they want concrete numbers, or
defer until phase 14 acceptance. If we follow that
precedent the ROADMAP entry says "captured in phase-14
acceptance".

### ROADMAP entry

Rewrite the existing thin Phase 23 entry (lines 591–599)
in the phase-21 / phase-22 template. Sections:

- One-line.
- Detailed plan link.
- Crates touched.
- Sub-tasks count (13: 23.0 → 23.12) + Exit.
- Scope cuts (see §5 below).
- Delivered (bullet per module / capability).
- Deferred (each item links to its open-question §).
- Bench results placeholder per the 21.7 / 22.8 precedent.

### Phase-doc updates

`docs/phases/phase-23-hybrid-query.md`:
- Status: ✓ tag `phase-23-complete`.
- Each sub-task heading: "Landed in: `.claude/plans/phase-23-task-NN.md`"
  link + `[ ] → [x]`.
- Done-when bullets ticked.
- Phase-exit section added (matching phase-22's layout).
- Scope cuts called out (matching ROADMAP).

### §30 open-questions

Add entries to `spec/30_knowledge_open_questions/00_purpose.md`
for items deferred during 23.x. Proposed entries:

- **OQ-23-A: Streaming hybrid query results** —
  `QUERY.limit > 100` should stream over SUBSCRIBE per spec
  §24/00 §"Streaming results". v1 returns a single
  `QueryResponse` with at most `limit` items; streaming is
  post-v1.
- **OQ-23-B: Hybrid query + transactional read-your-writes** —
  RECALL inside a txn falls back to the substrate path
  (23.11). Hybrid + RYW across statements + relations is
  multi-week work; post-v1.
- **OQ-23-C: Filter-only retriever mode** — the planner
  rejects empty text + no anchor as `NoSignal`. A
  filter-only mode (e.g. "all statements with confidence
  ≥ 0.9 in the last week") would require a new
  "everything" retriever; post-v1.
- **OQ-23-D: Learned router on top of the rule-based one** —
  re-affirms `OQ-V2-1`; phase 23 ships the rule-based
  router as the stable fallback. The labeled-query
  pipeline lands later.
- **OQ-23-E: Hybrid recall result merging across shards** —
  v1 is per-shard; the connection layer's fan-out happens
  upstream of the hybrid engine. Multi-shard fusion is
  post-v1.

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| 10K-scale benches (this plan) | Fast bench run; matches 22.8's regression-detector stance | Doesn't prove §2.10 targets at production scale | ✓ — phase 14 acceptance is the prod gate |
| 100K / 1M-scale benches | Validates spec directly | Bench setup time blows up; CI cost; tantivy 100K is heavyweight | rejected for v1 phase exit |
| Bench harness in brain-ops vs brain-planner | brain-ops sees the full stack | brain-planner owns the hybrid pipeline + accepts the `HybridExecutorContext` directly; smaller dependency surface for benches | `brain-planner` |
| Single mega test file vs split per scenario | Single file is easier to navigate | Mega file becomes a maintenance burden | one `knowledge_hybrid_phase_exit.rs` with 4 tests |
| Defer ROADMAP rewrite to v1.0 cut | Less churn now | Phase-23 entry stays stale; harder to track scope cuts | rewrite now (matches 22.8) |
| Single commit vs split commit | Atomic phase exit | One commit touches ~6 files | ✓ — same shape as 21.7 / 22.8 |
| New `spec/24/01_open_questions.md` file vs adding to §30 | Local context | The §30 file is the canonical home for knowledge-layer open questions; one place to look | append to §30 |
| Force the bench to assert p50 < target | Catches regressions automatically | Numbers fluctuate with hardware; CI would flake | report-only; phase 14 is the gate (21.7 / 22.8 precedent) |

## 6. Risks / open questions

- **Risk:** Integration tests are flaky because the indexer
  commits asynchronously. **Mitigation:** same poll-with-
  timeout pattern phase 22 / 21 used — bounded loop, 25 ms
  intervals, 500 ms cap. Already implemented in the existing
  `support_harness` / wire-test infrastructure.
- **Risk:** Bench corpus generation is slow on CI.
  **Mitigation:** 10K @ embedder + tantivy add ≈ a few
  seconds setup; criterion's `iter_batched` pulls setup
  out of the timed loop. We share one fixture across the
  three bench fns.
- **Risk:** The `bench_hybrid_three_retriever` setup needs
  a schema + entities + statements + memories. The test
  harness (`support_harness::start`) is server-level, but
  the bench wants direct planner access. **Mitigation:**
  build a minimal `HybridExecutorContext` in the bench
  setup using the same per-retriever traits the tests use
  (no TCP layer; saves ~ms per iter and keeps benches
  deterministic).
- **Open question:** Should the phase-23 bench wall-time
  numbers gate the tag? **Resolution:** no — report-only,
  per 21.7 / 22.8. Phase 14's acceptance suite is the
  blocking gate.

## 7. Test plan

The 4 integration tests in §4. Plus:

- `cargo zigbuild --target x86_64-unknown-linux-gnu --workspace --tests`
  clean at tag time.
- `cargo clippy --target x86_64-unknown-linux-gnu --workspace --tests
  -- -D warnings` clean (restricted to phase-23-touched crates if
  pre-existing pedantic warnings elsewhere).
- `cargo bench -p brain-planner --bench hybrid_query --
  --quick` runs green; results captured in ROADMAP or
  deferred to phase-14 acceptance per 22.8 precedent.

## 8. Commit shape

Single commit:

```
chore(planner,server,docs): 23.12 — phase 23 exit (tests + bench + ROADMAP + tag)

Closes phase 23.

- crates/brain-server/tests/knowledge_hybrid_phase_exit.rs
  (new): 4 integration tests driving the hybrid pipeline
  via the wire-op dispatcher — QUERY end-to-end, EXPLAIN,
  TRACE, and the substrate RECALL transparent-routing
  contract (spec §28/08 §5).
- crates/brain-planner/benches/hybrid_query.rs (new):
  three criterion benches against §16/02 §2.10 — hybrid
  3-retriever, router-degraded, EXPLAIN-only.
- crates/brain-planner/Cargo.toml: `[[bench]] name =
  "hybrid_query" harness = false` + criterion dev-dep.
- ROADMAP.md: Phase 23 entry rewritten in the phase-21 /
  phase-22 template (Exit / Scope cut / Delivered /
  Deferred / Bench).
- docs/phases/phase-23-hybrid-query.md: checkboxes flipped
  per sub-task; phase-exit section added.
- spec/30_knowledge_open_questions/00_purpose.md: OQ-23-A
  through OQ-23-E for items deferred during 23.x
  (streaming, hybrid-in-txn, filter-only mode, learned
  router, cross-shard fusion).
```

Plus an annotated tag:

```
git tag -a phase-23-complete -m "Phase 23 — Hybrid query engine: \
  SemanticRetriever + GraphRetriever + RRF fusion + filter chain + \
  rule-based router + planner + executor + EXPLAIN/TRACE + four \
  hybrid-query wire opcodes + fluent SDK query builder + transparent \
  RECALL routing on schema-declared deployments."
```

## 9. Confirmation

Please confirm:

1. **Four integration tests** are the right phase-exit surface (QUERY round-trip, EXPLAIN, TRACE, RECALL transparent routing). Each targets one observable behaviour.
2. **Three 10K-scale benches** in `brain-planner/benches/hybrid_query.rs` against §16/02 §2.10 (hybrid 3-retriever, router-degraded, EXPLAIN-only).
3. **Bench wall-time capture optional at tag** — same call as 21.7 / 22.8. ROADMAP entry can land with "captured in phase-14 acceptance" or with numbers; user's choice.
4. **OQ-23-A through OQ-23-E** appended to `spec/30_knowledge_open_questions/00_purpose.md` for the deferrals introduced during 23.x.
5. **Single commit + tag** `phase-23-complete` (annotated), matching the 21.7 / 22.8 shape.

After approval: implement → verify (workspace zigbuild + targeted clippy) → commit → tag.
