# Sub-task 8.4 — Consolidation worker

**Spec:** `spec/11_background_workers/03_consolidation.md`
**Phase doc:** `docs/phases/phase-08-workers.md` §8.4
**Done when:** Episodic memories meeting consolidation criteria become Consolidated; original episodics retained per spec.

---

## 1. Honest scope

Spec §6 — *"Brain doesn't ship with an LLM; it integrates with an external service. For deployments without an LLM, consolidation is disabled."* — defines the v1 ceiling. We can't generate real summaries. What we **can** build:

1. The **plumbing** that turns a cluster of source memories into a Consolidated memory + DERIVED_FROM edges + stamped sources.
2. A **`Summarizer` trait** as the seam where production deployments plug an LLM adapter.
3. A **`DisabledSummarizer`** default that makes the worker a no-op (matches spec §6/§16).
4. A **simple connected-components clustering** on cosine similarity within a `(context, kind=Episodic, recent)` window. Spec §4 calls for DBSCAN; single-linkage is correct enough for v1 (graph-of-pairs above threshold → connected components). Documented deviation.
5. **Idempotency** via deterministic `RequestId` derived from sorted source-id digest, so partial-crash retries don't duplicate Consolidated memories.

Out of scope:
- Real LLM integration → operators inject their own `Summarizer` later.
- Full DBSCAN → single-linkage is the v1 stand-in.
- Threshold trigger (spec §5 — context >50 episodics) → needs the event channel (Phase 9).
- Approval workflow (§15) → opt-in mode; defer.
- Recursive consolidation (§12) → cap at 1 level in v1; sources must be Episodic.

---

## 2. The Summarizer seam

```rust
// crates/brain-workers/src/summarizer.rs (NEW)

#[derive(Debug, thiserror::Error)]
pub enum SummarizerError {
    /// No LLM configured. Spec §16 — consolidation becomes a no-op.
    #[error("summarizer disabled")]
    Disabled,
    /// LLM call failed.
    #[error("summarizer call failed: {0}")]
    Failed(String),
}

#[async_trait::async_trait]   // OR Pin<Box<Future>> to avoid the dep
pub trait Summarizer: Send + Sync + 'static {
    async fn summarize(&self, memories: &[&str]) -> Result<String, SummarizerError>;
}
```

We already use the `Pin<Box<Future>>` pattern in `WriterHandle` to avoid `async-trait`. Do the same here:

```rust
pub trait Summarizer: Send + Sync + 'static {
    fn summarize<'a>(
        &'a self,
        memories: &'a [&'a str],
    ) -> Pin<Box<dyn Future<Output = Result<String, SummarizerError>> + Send + 'a>>;
}

pub struct DisabledSummarizer;
impl Summarizer for DisabledSummarizer {
    fn summarize<'a>(...) -> ... {
        Box::pin(async { Err(SummarizerError::Disabled) })
    }
}
```

No `async-trait` dependency added.

---

## 3. Clustering

```rust
// crates/brain-workers/src/consolidation.rs (clustering helpers)

pub struct ClusterCandidate {
    pub memory_id: MemoryId,
    pub vector: [f32; brain_embed::VECTOR_DIM],
    pub created_at_unix_nanos: u64,
}

/// Single-linkage clustering on cosine similarity. Returns clusters
/// of size ≥ `min_size`; singletons and small groups are dropped.
/// Spec §4 calls for DBSCAN; this is the v1 stand-in — every pair
/// above the threshold links into the same component, like
/// DBSCAN with min_pts=1 (so all "core" points are reachable from
/// each other).
pub fn cluster_by_similarity(
    candidates: &[ClusterCandidate],
    similarity_threshold: f32,    // default 0.6 (spec §4)
    min_cluster_size: usize,      // default 5 (spec §4)
) -> Vec<Vec<MemoryId>> { ... }

fn cosine(a: &[f32], b: &[f32]) -> f32 { ... }   // dot / (||a|| ||b||)
```

Algorithm: union-find. For each pair (i, j), if `cosine(v_i, v_j) >= threshold`, union(i, j). Return components with size ≥ `min_cluster_size`. O(n²) cosines per cycle — fine because `batch_size=100` (spec §11/01 §11 default).

---

## 4. Idempotent encode of the Consolidated memory

```rust
fn deterministic_request_id(source_ids: &[MemoryId]) -> RequestId {
    let mut sorted = source_ids.to_vec();
    sorted.sort();
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"brain/consolidation/v1");
    for id in &sorted {
        hasher.update(&id.to_be_bytes());
    }
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    // We could mint a v8 UUID for proper UUID semantics; for the
    // request_id field it just has to be a stable 16-byte value.
    RequestId(Uuid::from_bytes(bytes))
}
```

Encoded with `kind = Consolidated`. The writer's existing idempotency table (spec §07/06) makes retries a no-op: same `request_id` → same `MemoryId` → DERIVED_FROM edges already exist or get inserted via overwrite-weight semantics.

---

## 5. The cycle

```rust
async fn do_consolidation_cycle(
    worker: &ConsolidationWorker,
    ctx: &WorkerContext,
) -> Result<usize, WorkerError> {
    // Skip cleanly when no summarizer is configured.
    if worker.summarizer.summarize(&["probe"]).await
        .err().map_or(false, |e| matches!(e, SummarizerError::Disabled))
    {
        return Ok(0);
    }

    let cfg = worker.config();

    // 1. Find candidate contexts. v1: take all distinct contexts
    //    appearing in the recent-episodic window. Phase 9 will
    //    pre-index this.
    let candidates_by_ctx = collect_recent_episodics(
        &ctx.ops.executor.metadata,
        now_unix_nanos(),
        worker.recency_window_nanos,   // default 24h
    )?;

    let mut consolidated_total = 0usize;
    let started = Instant::now();
    for (context_id, candidates) in candidates_by_ctx {
        if started.elapsed() >= cfg.max_runtime { break; }
        if consolidated_total >= cfg.batch_size { break; }
        if ctx.is_shutdown() { break; }

        let clusters = cluster_by_similarity(
            &candidates,
            worker.similarity_threshold,
            worker.min_cluster_size,
        );

        for cluster in clusters {
            // Skip if any source is already consolidated (spec §11).
            if any_already_consolidated(&ctx.ops.executor.metadata, &cluster)? {
                continue;
            }

            // 2. Fetch texts for the cluster (one read txn).
            let texts = fetch_texts(&ctx.ops.executor.metadata, &cluster)?;
            let summary = worker.summarizer
                .summarize(&texts.iter().map(String::as_str).collect::<Vec<_>>())
                .await
                .map_err(|e| WorkerError::Ops(format!("summarize: {e}")))?;

            // 3. Embed the summary.
            let vec = ctx.ops.executor.embedder.embed(&summary)
                .map_err(|e| WorkerError::Ops(format!("embed: {e:?}")))?;

            // 4. EncodeOp with deterministic request_id and
            //    DERIVED_FROM edges to each source.
            let request_id = deterministic_request_id(&cluster);
            let edges: Vec<EncodeOpEdge> = cluster.iter().map(|id| EncodeOpEdge {
                target: *id,
                kind: EdgeKind::DerivedFrom,
                weight: 1.0,
            }).collect();
            let op = EncodeOp {
                request_id,
                context_id,
                kind: MemoryKind::Consolidated,
                text: summary,
                vector: vec,
                salience_initial: 0.7,    // spec §11/02 §10: Consolidated half-life is 90d
                fingerprint: ctx.ops.executor.embedder.fingerprint(),
                edges,
            };
            let ack = ctx.ops.executor.writer.submit_encode(op).await
                .map_err(|e| WorkerError::Ops(format!("submit_encode: {e}")))?;

            // 5. Stamp sources with consolidated_at_unix_nanos.
            //    Separate write txn; idempotent across retries since
            //    the consolidated memory_id is stable (request_id).
            stamp_sources(&ctx.ops.executor.metadata, &cluster, now_unix_nanos())?;

            consolidated_total += 1;
            let _ = ack; // memory_id available if needed for tracing
        }
    }
    Ok(consolidated_total)
}
```

Atomicity note: spec §8 wants the encode + edges + source stamps in one txn. v1 achieves *idempotency* instead: a partial crash leaves the system in a state where the next cycle re-runs the same encode (same request_id → cached) and re-stamps the sources (idempotent). No duplicate Consolidated memories.

---

## 6. `ConsolidationWorker`

```rust
pub struct ConsolidationWorker {
    config: WorkerConfig,
    summarizer: Arc<dyn Summarizer>,
    similarity_threshold: f32,     // default 0.6
    min_cluster_size: usize,       // default 5
    recency_window_nanos: u64,     // default 24h
    initial_salience: f32,         // default 0.7
}

impl ConsolidationWorker {
    pub fn new(summarizer: Arc<dyn Summarizer>) -> Self;
    pub fn with_config(self, cfg: WorkerConfig) -> Self;
    pub fn with_similarity_threshold(self, t: f32) -> Self;
    pub fn with_min_cluster_size(self, n: usize) -> Self;
    pub fn with_recency_window(self, d: Duration) -> Self;
    pub fn with_initial_salience(self, s: f32) -> Self;
}

impl Worker for ConsolidationWorker { ... }
```

`with_*` builders match the existing pattern from `DecayWorker` / `AccessBoostWorker`.

---

## 7. File-by-file plan

| File | Action | Notes |
| ---- | ------ | ----- |
| `crates/brain-workers/Cargo.toml` | Edit | Add `blake3.workspace = true`, `brain-embed = { path = "../brain-embed" }`, `brain-core = { path = ... }` already there, `uuid.workspace = true` already (via brain-core re-exports? — confirm) |
| `crates/brain-workers/src/summarizer.rs` | NEW | Summarizer trait + DisabledSummarizer |
| `crates/brain-workers/src/consolidation.rs` | NEW | clustering, deterministic_request_id, ConsolidationWorker |
| `crates/brain-workers/src/lib.rs` | Edit | Re-exports |
| `crates/brain-workers/tests/consolidation.rs` | NEW | ~18 tests |

No spec changes. No brain-ops changes. No wire changes.

---

## 8. Tests (`crates/brain-workers/tests/consolidation.rs`)

### Summarizer (2)
1. `disabled_summarizer_returns_disabled_error`.
2. `echo_summarizer_returns_joined_input` (test helper used by the cycle tests).

### Clustering (5)
3. `two_high_similarity_memories_form_one_cluster` — pair above threshold + min_size=2 → 1 cluster of 2.
4. `low_similarity_memories_do_not_cluster` — pair below threshold → 0 clusters.
5. `transitive_chain_merges_into_one_cluster` — A↔B and B↔C above threshold; A↔C below → still one cluster of 3 via single linkage.
6. `cluster_below_min_size_is_dropped` — group of 4 with min_size=5 → 0 clusters.
7. `isolated_memory_is_dropped` — single member can never reach min_size → dropped.

### Idempotent request_id (2)
8. `same_source_set_produces_same_request_id`.
9. `different_source_sets_produce_different_request_ids`.

### Cycle (7)
10. `disabled_summarizer_produces_no_consolidations` — register w/ DisabledSummarizer; seed cluster; cycle → 0 created.
11. `cluster_of_five_episodics_produces_one_consolidated` — seed 5 high-similarity Episodics with the same context; cycle → 1 Consolidated created.
12. `consolidated_memory_has_derived_from_edges_to_each_source` — same setup as 11; assert EDGES_OUT from new memory to all 5 sources, EdgeKind::DerivedFrom.
13. `sources_are_stamped_with_consolidated_at` — same setup; assert each source row has `consolidated_at_unix_nanos.is_some()`.
14. `already_consolidated_sources_are_skipped` — seed cluster with one source already stamped; cycle → 0 created.
15. `cross_context_memories_do_not_cluster` — two contexts each with 5 similar memories; cycle → 2 consolidations max (one per context), never cross-context.
16. `tombstoned_memories_are_not_candidates` — seed cluster of 5 with one tombstoned (via writer.submit_forget); cycle → cluster has only 4 → below min_size → 0 created (with default min_size=5).

### Worker integration (2)
17. `worker_registers_with_correct_kind_and_default_cadence` — default 5m interval, kind=Consolidation.
18. `disabled_worker_via_config_does_not_run` — `enabled=false` worker registered; no consolidations after sleep.

---

## 9. Test fixture nuances

- Use a stub `EchoSummarizer` for tests: returns `format!("[{}]", texts.join("|"))`. Deterministic, lets us assert the consolidated text.
- Seed memories via direct `MemoryMetadata::new_active` table writes + HNSW inserts. Real MockDispatcher gives deterministic vectors so we can pick texts that yield high or low cosine.
- For the "DERIVED_FROM edges" test: open EDGES_OUT_TABLE and iterate over `(consolidated_id, EdgeKind::DerivedFrom as u8, *)` range.

---

## 10. Risks

| Risk | Mitigation |
| ---- | ---------- |
| Clustering O(n²) on every cycle | batch_size=100 default keeps n small; spec §11/01 §11 honored |
| LLM latency dominates the cycle | Spec §10: 500-2000 ms per LLM call expected; max_runtime=10s (spec §11/01 §11) bounds total |
| Partial-crash duplication | Deterministic request_id → idempotent encode replay |
| MockDispatcher's cosine semantics in tests | Pick texts whose byte-wise overlap drives the cosine above/below 0.6 — test fixture has explicit text choices |
| Adding blake3 to brain-workers | Already a workspace dep used by brain-ops; trivial cargo add |

---

## 11. Done criteria

- [ ] `Summarizer` trait + `DisabledSummarizer` shipped.
- [ ] Clustering helper + `cluster_by_similarity()` + `deterministic_request_id()`.
- [ ] `ConsolidationWorker` implementing `Worker`; default cadence 5m.
- [ ] DERIVED_FROM edges + source stamping + idempotency working end-to-end.
- [ ] 18 tests pass first run.
- [ ] `cargo test --workspace` green; clippy + fmt clean.
- [ ] Commit subject: `feat(brain-workers): consolidation worker (sub-task 8.4)`.

~700 LOC impl + ~800 LOC tests. Larger than 8.2/8.3 but still one commit.
