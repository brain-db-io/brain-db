# Brain Improvement Plan — Speed, Accuracy, Context

## Context

Brain is a typed memory-layer database for AI agents (Rust + glommio + redb + HNSW + tantivy + GLiNER + LLM extractors). Most of the substrate layer ships and the typed-graph layer (Entities / Statements / Relations / Hybrid retrieval) is built but populated unevenly. This plan was produced after a three-pronged research pass:

1. **Arc-labs / Recall** — deep architecture read (six-stage write pipeline, five-tier conflict ladder, RRF with adaptive top-K, prompt-cached system blocks, grounding verdict + evidence spans, plugin surface)
2. **Brain spec deep read** — what §00-§31 mandates vs aspires vs implements; identified ~12 spec-promised pieces not yet built
3. **Memory-layer state of the art** — MemGPT/Letta, Mem0, Zep/Graphiti, Cognee, LangMem, Anthropic memory tool, ChatGPT memory; latency benchmarks (LongMemEval, LoCoMo, DMR), token-budget metrics, bi-temporal patterns

The plan covers all three waves at file-level detail. **Defaults are picked for the flagged design questions** (called out inline as `[DEFAULT: ...]` — you can override later).

## Strategic framing — speed, accuracy, context

Three axes drive every item below:

- **Speed** — hit `spec/16_benchmarks_acceptance/02_latency_targets.md`: ENCODE p99 = 25 ms, RECALL p99 = 20 ms, REASON p99 = 35 ms. Mem0 hits 0.708 s p50 / 1.44 s p95 with LLM-everywhere; Brain's tiered extraction is the cost moat.
- **Accuracy** — Recall@10 ≥ 0.95 at 1 M memories (spec §16/05). Zep tops DMR at 94.8 %; Brain should target ≥ 0.92.
- **Context** — surface area of typed-graph features actually usable downstream. Today: classifier returns entities but statement HNSW is empty, so the SemanticRetriever's statement corpus mode never fires.

Brain's seven invariants are non-negotiable (CLAUDE.md §5). No item below violates them.

---

## Wave 1 — Foundation (close spec gaps that block typed-graph usefulness)

Each item ≤ 3 days. Zero wire-protocol exposure. **Must finish before v1.0 makes sense as a memory-layer claim.**

### W1.1 — Populate StatementHnsw via background worker (CRITICAL PATH)

**Why**: `crates/brain-index/src/statement_hnsw.rs` has insert/search APIs but no worker calls `insert_statement`. `SemanticRetriever` returns empty in statement-corpus mode → hybrid recall over statements degenerates to BM25 + graph only. **Single biggest blocker on Recall@10.**

**Files**
- **NEW**: `crates/brain-workers/src/workers/statement_embed.rs` — model on `crates/brain-workers/src/workers/hnsw_maintenance.rs` shape
- Wire into `crates/brain-workers/src/workers/mod.rs`, `crates/brain-workers/src/scheduler.rs`, `crates/brain-workers/src/config.rs`
- Reads from `brain-metadata::statement::statement_list` (active, un-tombstoned set; track via new `embedded_at_unix_nanos: Option<u64>` column)
- Embedder pool: `crates/brain-embed/src/...`
- Per-shard write to `StatementHnswIndex` held in shard state

**Spec**: §06/02 (HNSW params), §19/03 (Statement storage), §16/02 §2.5 (gated as "phase 21" — this lifts it)

**Effort**: M (2-3 d)

**Verification**:
- New `crates/brain-index/benches/statement_hnsw.rs` — search latency at 100k / 1M statements
- Integration test: ENCODE → wait → SEMANTIC retriever returns the statement
- Recall@10 over a seeded fixture ≥ 0.90

### W1.2 — Entity resolver Tier 3 (HNSW embedding tie-break)

**Why**: `crates/brain-extractors/src/resolver.rs:24-32` explicitly notes "Phase E does not consult the entity HNSW (tier-3)". Trigram-only resolution fragments the entity graph ("Acme Inc" vs "Acme, Inc."). Without Tier 3 the graph retriever recall collapses.

**Files**
- `crates/brain-extractors/src/resolver.rs` — add `tier_embedding` between `tier_alias` and fallback create
- `crates/brain-index/src/entity_hnsw.rs` — already has `search`; verify top-K ergonomics
- `crates/brain-workers/src/metrics.rs` — per-tier counter (already partially present per resolver doc)

**Spec**: §18/01 §3 ("Tier 3 — embedding HNSW")

**Effort**: S-M (1-2 d)

**Verification**:
- Unit test in `resolver.rs` mirroring `tier_fuzzy_matches_close_surface_form_and_adds_alias`
- Integration test ingesting 20 docs with "Acme Inc" / "Acme, Inc." resolves to one EntityId

### W1.3 — Classifier real forward pass for statement-kind head (CRITICAL PATH)

**Why**: `crates/brain-extractors/src/classifier.rs` runs in degraded mode for the statement-kind classifier (Fact / Preference / Event). Without it the pipeline falls through to LLM for every statement → blows the 25 ms ENCODE p99 + the LLM-cost moat. GLiNER shipped covers entity heads; statement-kind head unwired.

**Files**
- `crates/brain-extractors/src/classifier.rs` — implement `forward_inference` over loaded model
- Reuse `crates/brain-extractors/src/gliner/{backbone,head,tokenizer}.rs` — architecture is BERT-encoder + linear head; statement-kind head can share GLiNER's encoder (config flag near `classifier.rs:557-580`)
- **No second BERT load** — piggyback on GLiNER's batched encoder

**Spec**: §22/02 (Classifier extractor), §16/02 §2

**Effort**: M (3 d)

**Verification**:
- Smoke test in classifier.rs (model on `real_inference_returns_brain_qnames_for_alice` from `classifier.rs:1414`)
- Accuracy harness on 200 labelled snippets — ≥ 0.92 macro-F1 to enable by default
- Bench ENCODE p99 falls under 25 ms with classifier on

### W1.4 — Confidence sweep worker

**Why**: spec §19/04 promises stored confidence is periodically refreshed (noisy-OR aggregation with kind-specific decay). Long-running deployments show stale (overly-high) confidence on Preferences/Facts whose evidence has aged. Affects ranker weight and supersession decisions.

**Files**
- **NEW**: `crates/brain-workers/src/workers/confidence_sweep.rs` — model on `crates/brain-workers/src/workers/decay.rs` for cadence + chunked-scan
- Wire into `mod.rs`, `scheduler.rs`, `config.rs`
- Update path: `crates/brain-metadata/src/statement/supersede.rs` (or a new `confidence.rs`)
- Decay helpers: `crates/brain-core/src/knowledge/confidence.rs`

**Spec**: §19/04

**Effort**: M (2 d)

**Verification**:
- Integration test seeding a Statement extracted 90 d ago, running sweep, checking confidence dropped per decay curve
- Property test: idempotent under repeated sweeps (WAL replay)

### W1.5 — Re-derivation worker on FORGET cascade

**Why**: §17/03 mandates that hard-FORGET of a Memory re-derives or supersedes-with-null all dependent Statements. `crates/brain-workers/src/workers/forget_cascade.rs` exists — needs audit against §17/03 (handles statement dependence, not just edge removal). Without this, FORGET creates dangling provenance.

**Files**
- Audit `crates/brain-workers/src/workers/forget_cascade.rs` against §17/03
- Hook in `crates/brain-ops/src/apply/memory.rs` (FORGET handler) to enqueue
- Statement update path in `crates/brain-metadata/src/statement/supersede.rs`

**Spec**: §17/03, §25 provenance

**Effort**: S (1 d to audit + close gap)

**Verification**:
- Chaos test (use `brain-chaos-test` skill): ENCODE 100 memories with extracted statements, FORGET 10, kill mid-cascade, restart
- Invariant: all dependent statements have either re-derived from remaining memories or carry supersede-with-null marker; no dangling `cited_memory` refs

### W1.6 — Schema migration backfill worker activation

**Why**: §21 schema DSL says `SCHEMA_UPLOAD` triggers backfill per the `keep` / `re-extract` / `tombstone` action. `crates/brain-workers/src/workers/schema_migration.rs` exists; verify it consumes the action and that `crates/brain-ops/src/apply/schema.rs` enqueues. Without this, type changes silently leave old data on the old schema.

**Files**
- Audit `crates/brain-workers/src/workers/schema_migration.rs` for the three actions
- `crates/brain-ops/src/apply/schema.rs` — ensure enqueue on schema version change
- `crates/brain-metadata/src/storage_version.rs` — migration plumbing

**Spec**: §21 schema DSL §"Backfill semantics"

**Effort**: S-M (1-2 d)

**Verification**:
- Integration test: upload schema v1, ingest, upload v2 with `re-extract`, observe re-extracted statements
- With `tombstone`, observe soft-tombstone of stale ones

**Wave 1 totals**: ~10-13 working days; zero wire bumps; closes the statement-corpus, entity-graph, and provenance gaps that block Brain from being a real memory-layer claim.

---

## Wave 2 — Differentiation (adopt SOTA, slot into existing architecture)

Each item ≤ 1 week.

### W2.1 — Five-tier supersession with LLM judge (arc-labs Tier 0-3)

**Why**: Brain has supersession chains but the decision logic is single-tier. Arc-labs' five-tier ladder is the most-validated dedupe pattern in the field for typed-statement work. Maps cleanly onto Brain's `(subject, predicate, object)` triple + statement embedding.

The ladder:
- **Tier 0** — exact `(subject, predicate)` match → force supersession (stateful) or contradicts (idempotent)
- **Tier 1** — cosine ≥ 0.92 → auto-supersede
- **Tier 2** — cosine 0.82-0.92 → LLM judge (SUPERSEDES vs COEXISTS)
- **Tier 3** — cosine < 0.82 → both coexist (clean)

**Files**
- `crates/brain-metadata/src/statement/supersede.rs` — promote to a tiered decider
- `crates/brain-extractors/src/llm.rs` — add `judge_supersedes(a, b) -> Verdict` with typed structured output; reuse W2.4's prompt-caching wrapper
- `crates/brain-index/src/statement_hnsw.rs` — needed for Tier 1/2 NN lookup (depends on W1.1)
- `crates/brain-workers/src/workers/supersession_sweeper.rs` — wire to the new tiered decider

**Spec**: §19/01 (Supersession), §19/02 (Contradiction)

**Effort**: M-L (4-5 d). Depends on W1.1 + W2.4.

**Verification**:
- Golden file of 200 statement pairs with human-labelled SUPERSEDES/COEXISTS/CONTRADICTS
- Tier 0-3 reaches ≥ 0.93 agreement
- LLM call count per encode ≤ 0.15 (cost moat)

### W2.2 — Score-weighted RRF + adaptive top-K + cross-encoder rerank

**Why**: `crates/brain-planner/src/hybrid/fusion.rs:23` already uses RRF k=60. Three upgrades:
1. **Per-retriever weight** — Semantic > Lexical > Graph > Temporal default; configurable per query
2. **Adaptive top-K** — based on query class (router signal already in `crates/brain-planner/src/hybrid/router.rs`)
3. **Optional cross-encoder rerank** — top-50 → top-10, gated behind `RECALL.rerank=true`

**Files**
- `crates/brain-planner/src/hybrid/fusion.rs` — add weights param
- `crates/brain-planner/src/hybrid/router.rs` — emit top-K hint per query class
- `crates/brain-planner/src/hybrid/executor/...` — invoke reranker
- **NEW**: `crates/brain-embed/src/cross_encoder.rs` or reuse embed pool with `bge-reranker-base` [DEFAULT: bge-reranker-base, 110M params, MIT-licensed]
- Existing `merge_and_rerank` in `crates/brain-ops/src/index/semantic_retriever.rs:153` — fold in

**Spec**: §23/01 (RRF), §24 (Hybrid query)

**Effort**: M (3-4 d)

**Verification**:
- LongMemEval slice (see §D below)
- Expected: Recall@10 +3-6 pts vs unweighted RRF; rerank +1-3 pts at cost of +6-9 ms p99

### W2.3 — Tiered extractor pipeline with bounded context (top-m similar)

**Why**: Pattern → Classifier → LLM-last is already partially there. Missing: **bounded context for the LLM tier** — pass `top_m=10` similar memories + a rolling summary instead of unbounded history. This is what makes Mem0/Zep tractable cost-wise.

**Files**
- `crates/brain-extractors/src/llm.rs` — add `extract_with_context(text, top_m_neighbors, summary)`
- Helper in `crates/brain-ops/src/apply/encode.rs` — fetch top-m via existing SemanticRetriever before extractor invocation
- `crates/brain-workers/src/workers/summarizer.rs` — wire summary in (worker may already exist)

**Spec**: §22/09 (LLM extractor) + adoption of SOTA pattern from Mem0

**Effort**: M (3 d)

**Verification**:
- LongMemEval token-budget metric — record `tokens_per_query` p50/p95
- Target: median LLM call ≤ 2k tokens input
- Recall delta ≥ 0 (must not regress)

### W2.4 — Prompt-cache scaffold for LLM extractor + judge (CRITICAL FOR COST)

**Why**: Anthropic / OpenAI prompt caching of role + schema system blocks gives 30-90 % cost reduction. Brain's LLM client at `crates/brain-llm/src/anthropic.rs` does not show explicit `cache_control` blocks today. **Foundation for W2.1 (judge would be unaffordable without it).**

**Files**
- `crates/brain-llm/src/anthropic.rs` — add `cache_control: ephemeral` on system block in extractor + judge calls (use `claude-api` skill semantics)
- `crates/brain-llm/src/types.rs` — expose `CachedSystemBlock` typed wrapper
- `crates/brain-extractors/src/llm.rs` — split system prompt into role-block + schema-block (mirror arc-labs `extract.rs:266-631`)

**Spec**: not spec'd explicitly; pure SOTA adoption

**Effort**: S (1 d)

**Verification**:
- Log `cache_creation_input_tokens` and `cache_read_input_tokens`
- Ratio ≥ 0.7 in steady state

### W2.5 — Per-tenant scope binding at API key issuance

**Why**: Arc-labs binds `(org_id, user_id, namespace_id, agent_id, permissions)` to the API key — server derives scope from header, eliminating agent-impersonation bugs. Brain's multi-tenant story needs this. No `ApiKey` scope binding found under `crates/brain-server/src/admin/`.

**Files**
- **NEW**: `crates/brain-server/src/admin/api_key.rs` (table-backed via brain-metadata)
- `crates/brain-server/src/network/...` — derive scope from header (today is permissive)
- **NEW** table in `crates/brain-metadata/src/tables/` — `api_keys` (use `brain-redb-schema` skill)

**Spec**: not in spec; pre-v1 security gap

**Effort**: M (3-4 d). **WIRE BUMP FLAG** — adding an authenticated handshake field beyond what `crates/brain-protocol/src/handshake.rs` carries likely triggers `brain-protocol-version-bump`. [DEFAULT: ship as opt-in via env flag in v1.0, deny-by-default in v1.1 to avoid breaking existing deploys.]

**Verification**:
- Integration tests
- Deny-by-default tests for cross-namespace access

### W2.6 — Confidence-banded merge review queue + ambiguity resolver worker

**Why**: §18/03 §4.2 promises [0.7, 0.95) merges go to a review queue; pending statements (`SubjectRef::Pending(audit)`) need a worker that re-runs resolution as context grows.

**Files**
- **NEW** table `merge_review_queue` in `crates/brain-metadata/src/tables/` (entity merge proposals with bands)
- **NEW** worker: `crates/brain-workers/src/workers/ambiguity_resolver.rs` — periodically scan `Pending(audit_id)` rows from `crates/brain-metadata/src/tables/statement.rs:514`, re-run resolver, finalize when confident
- `crates/brain-ops/src/apply/entity.rs` — emit merge proposals into queue when score in band
- Admin op to list/approve — could defer to W3

**Spec**: §18/03 §4.2

**Effort**: M (4 d)

**Verification**:
- End-to-end: ingest doc A with "J. Smith", later ingest doc B identifying as "Jane Smith, CEO of X" — Pending → Resolved automatically

**Wave 2 totals**: ~4-5 weeks; one likely wire bump (W2.5); brings Brain to competitive parity with Zep/arc-labs on accuracy + cost.

---

## Wave 3 — Strategic (1-4 weeks each; design + first impl)

### W3.1 — Procedural memory hook (agent-rewritable system block)

**Why**: LangMem's procedural memory is a real differentiator. Brain's typed graph + statement kinds make it natural: a `Statement{kind: Preference, subject: Agent, predicate: behavior.*, object: prompt_fragment}` *is* procedural memory. Add a recall path that materializes it as a system block.

**Files**
- **NEW**: `crates/brain-ops/src/apply/procedural.rs` — `MATERIALIZE_PROCEDURAL` op (read-only over statements + render)
- New retriever variant in `crates/brain-planner/src/hybrid/...` filtered to procedural predicate set
- Schema additions for procedural predicates in `spec/21_schema_dsl/...`
- `crates/brain-protocol/src/requests/` — new opcode → **wire bump**

**Effort**: L (2 weeks). [DEFAULT: defer to v1.1 — wire freeze risk in v1.0.]

**Verification**: live demo — agent updates own behavior post-failure; behavior persists across sessions.

### W3.2 — Plan/Reason VSA algebra (first impl)

**Why**: §9 cognitive operations. Today `crates/brain-planner/src/lib.rs` is query-only. VSA bundling over typed predicates would enable structural similarity, not just semantic. Spec'd but unbuilt.

**Files**
- `crates/brain-planner/src/planner/`, `crates/brain-planner/src/executor/phase.rs`
- **NEW**: `crates/brain-planner/src/vsa/` with HRR/MAP-encoded operators

**Effort**: XL (3-4 weeks). [DEFAULT: design-only in v1.0; first impl in v1.1.]

**Verification**: relational analogy benchmark on synthetic typed-graph queries.

### W3.3 — Plugin surface (EnricherPlugin + ConnectorPlugin)

**Why**: Arc-labs' pre-dedupe enricher hook + connector plugin is how third parties extend without forking. Maps to Brain's six-stage pipeline: Enricher slots between `extract` and `dedupe`, Connector wraps `pre_filter`.

**Files**
- **NEW crate**: `crates/brain-plugins/` (trait + dynamic loader)
- Hooks in `crates/brain-extractors/src/extractor.rs` + `crates/brain-ops/src/apply/encode.rs`
- Spec: new section `spec/22_extractors/10_plugins.md`

**Effort**: L (2 weeks). Plugins must run on the writer's executor only (single-writer-per-shard invariant). [DEFAULT: ship in v1.1.]

**Verification**: load a tiny enricher plugin (emoji-to-text normalizer); ingest; observe field mutated pre-dedupe; tests for plugin failure isolation (panic-safe).

### W3.4 — Bi-temporal `t'_created` / `t'_expired` separation (Zep four-timestamp model)

**Why**: Zep's four-timestamp model. Brain already has `valid_from / valid_to` (object time) and `extracted_at` (record time). Adding a separate `record_invalidated_at` (`t'_expired`) lets "what did I believe on date X" queries work without resurrecting tombstones.

**Files**
- `crates/brain-core/src/knowledge/statement.rs` — add `record_invalidated_at_unix_nanos: Option<u64>`
- `crates/brain-metadata/src/tables/statement.rs` — schema bump (use `brain-redb-schema` skill)
- `crates/brain-planner/src/hybrid/filters.rs` — `as_of(record_time)` filter

**Effort**: L (1-2 weeks) — touches storage migration + query planner. [DEFAULT: v1.1 migration. Brain's `valid_from/valid_to` covers most asks; record-time travel is enterprise-grade.] **No wire bump if kept server-internal.**

**Verification**: time-travel query test — ingest at t0, supersede at t1, query `as_of=t0.5` returns the original.

---

## Cross-cutting decisions

### Defaults picked (override if needed)

| # | Decision | Default | Rationale |
|---|---|---|---|
| 1 | W3.4 bi-temporal `t'_expired` ship now or v1.1? | **v1.1** | `valid_from/valid_to` covers most asks |
| 2 | W3.1 Procedural Memory v1.0 (wire bump) or v1.1? | **v1.1** | Wire freeze risk in v1.0 |
| 3 | W2.5 Auth scope: in-process trust vs full multi-tenant v1.0? | **Opt-in v1.0, default v1.1** | Avoid breaking existing deploys |
| 4 | W2.2 Cross-encoder model? | **bge-reranker-base** (110 M, MIT) | Best precision/memory trade in OSS |
| 5 | W2.1 Tier-2 LLM judge — Haiku vs local Qwen? | **Claude Haiku** | Existing `brain-llm/anthropic.rs` wire |

### Patterns rejected (and why)

| Pattern | Rejected because |
|---|---|
| Postgres + pgvector backend | Violates "no storage backend swap" + breaks the sharded/embedded posture |
| Mem0 LLM-every-write | Blows ENCODE p99 25 ms target + cost; we use tiered extraction |
| Letta agent-owns-layout | Breaks single-writer-per-shard invariant |
| ChatGPT concat-everything | Brain doesn't own inference; not our role |
| Schemaless statements | Brain's typed graph is the differentiator |
| Write-back deferred WAL | Violates **WAL-before-ack** invariant; non-negotiable |

### What we won't do in v1.0

- Multi-region replication (v2.0 — enterprise differentiator)
- Memory-as-files agent tool surface for Anthropic SDK interop (v1.1)
- Open-source plugin marketplace
- Tier-4 LLM entity resolver (deferred per `spec/16_benchmarks_acceptance/02_latency_targets.md:90`)
- W3.2 VSA algebra production impl (design only in v1.0)

---

## Verification + benchmark plan

### Per-wave bench gates

Use the `bench` skill, then `brain-perf-target` skill to compare against spec.

| Gate | Bench | Target |
|---|---|---|
| Pre-W1 baseline | `crates/brain-planner/benches`, `crates/brain-index/benches` | Record current Recall@10 + p99 |
| Post-W1 | + new statement-HNSW bench | Recall@10 ≥ 0.85 (statement corpus now non-empty); ENCODE p99 ≤ 25 ms (classifier on) |
| Post-W2 | + LongMemEval slice runner | Recall@10 ≥ 0.95 (spec target); RECALL p99 ≤ 20 ms without rerank, ≤ 30 ms with |
| Post-W3 | + LoCoMo + DMR slices | DMR ≥ 0.92 (vs Zep 0.948); LoCoMo per-task within 10 % of Zep |

### External evals to add

**NEW crate**: `crates/brain-evals/` (or under `crates/brain-cli/`) running:

1. **LongMemEval** — 500-conversation subset; measure Recall@k, token-budget, latency
2. **LoCoMo** — long-conversation memory eval; subset of 50 dialogues
3. **DMR** (Dialogue Memory Recall) — Zep's headline benchmark, fast to iterate

Run all three nightly via `justfile` target `just eval-nightly`.

### Rough timeline to v1.0

Assuming 1 engineer @ full time on database core:

| Phase | Calendar |
|---|---|
| Wave 1 (6 items × M avg) | ~3 weeks |
| Bench gate + perf-target sweep | 1 week |
| Wave 2 (6 items × M-L avg) | ~4-5 weeks |
| LongMemEval + LoCoMo harness | 1 week (parallel) |
| Wave 3 (pick 1-2 for v1.0; rest v1.1) | 2-4 weeks |
| Hardening + chaos-test passes (1000-iter kill, `brain-chaos-test`) | 1 week |
| **v1.0 tag** | **~11-13 weeks** |

**Critical path** (must finish before bench gate is meaningful): **W1.1 (statement HNSW worker)** and **W1.3 (classifier inference)**. Start both day 1 in parallel.

---

## Critical files referenced in this plan

- `crates/brain-workers/src/workers/mod.rs`
- `crates/brain-extractors/src/resolver.rs`
- `crates/brain-extractors/src/classifier.rs`
- `crates/brain-extractors/src/llm.rs`
- `crates/brain-llm/src/anthropic.rs`
- `crates/brain-index/src/statement_hnsw.rs`
- `crates/brain-index/src/entity_hnsw.rs`
- `crates/brain-planner/src/hybrid/fusion.rs`
- `crates/brain-planner/src/hybrid/router.rs`
- `crates/brain-metadata/src/statement/supersede.rs`
- `crates/brain-metadata/src/tables/statement.rs`
- `crates/brain-ops/src/apply/memory.rs`
- `crates/brain-ops/src/apply/schema.rs`
- `spec/16_benchmarks_acceptance/02_latency_targets.md`
- `spec/17_knowledge_model/03_composition.md`
- `spec/18_entities/01_resolution.md`
- `spec/19_statements/01_supersession.md`
- `spec/19_statements/04_confidence.md`
- `spec/21_schema_dsl/00_purpose.md`
- `spec/22_extractors/00_purpose.md`
- `spec/23_retrievers/01_rrf_fusion.md`
- `spec/24_hybrid_query/00_purpose.md`

---

# Part II — How the five cognitive primitives work (deep walk-through)

Each subsection follows the same shape: **wire contract → code path → worked example with timeline → workers that fire post-commit → spec refs → edge cases**. The worked examples reference real file paths in the Brain codebase so you can step through with a debugger.

The five primitives are: `ENCODE` (write a memory), `RECALL` (read), `PLAN` (find a path between memories), `REASON` (derive inferences), `FORGET` (tombstone). They're defined in `spec/09_cognitive_operations/`.

---

## ENCODE — "store a memory"

### Wire contract (`spec/09_cognitive_operations/02_encode.md`)

```
ENCODE(text, agent_id, context, kind, metadata, edges, request_id, deduplicate)
  → EncodeResponse {
      memory_id,
      was_deduplicated,
      salience,
      auto_edges_added,
      edge_results,
      persisted_at,
      fingerprint,
      pending_stages,        // [auto_edge, temporal_edge, extractor]
      has_active_schema,
      has_llm_extractor,
    }
```

### Code path (request → ack)

1. **brain-shell** parses `encode "..." --wait` and builds `EncodeRequest`.
2. **brain-sdk-rust / proto** frames it (opcode `0x0103`) and sends over TCP.
3. **brain-server / network::connection** routes by shard (hash(agent_id) mod N).
4. **brain-server / dispatch** matches opcode → `handle_encode`.
5. **brain-ops / handlers/encode.rs** validates: text length, edge count ≤ 64, kind ≠ Consolidated, request_id present.
6. **brain-embed** embeds the text → 384-d vector (BGE-small; cached if seen recently).
7. **brain-ops / apply/encode.rs** builds the `Phase::UpsertMemory` and `Phase::Link` (one per edge).
8. **brain-ops / writer/submit.rs**:
   a. Idempotency check — look up `WriteId` in `IDEMPOTENCY_TABLE`; if hit with same `request_hash`, return cached ack.
   b. Allocate slot — `brain-storage / arena` reserves slot N, stamps `slot_version`.
   c. **WAL append** — `wal_map.rs::phase_to_wal_payload` → `WalPayload::Encode`. Single phase = direct append. fsync via `pwritev2(RWF_DSYNC)` group commit.
   d. **Apply** — `apply/dispatch.rs` routes `Phase::UpsertMemory` → `apply/memory.rs::apply_upsert_memory` which writes the `memories`, `texts`, `timeline` redb tables in one wtxn. Then `Phase::Link` → `apply/edge.rs::apply_link` writes the `edges_out` + `edges_in` rows.
   e. **HNSW insert** — `brain-index` stamps the new vector into the memory HNSW.
   f. **Stamp idempotency row** — same wtxn as the apply (post-fix from earlier slice).
   g. **Commit**.
9. **submit.rs::try_enqueue_auto_edge / try_enqueue_temporal_edge / try_enqueue_extractor** — post-commit hooks. Each returns `true` if enqueued, populates `pending_stages`.
10. **publish_events_for** — `MemoryEncoded` envelope to SubscribeRegistry.
11. **Response** returned to caller.

### Worked example: `encode "Alice Wong works at Acme Corp." --wait`

| T (ms) | Where | What happens |
|---|---|---|
| 0 | shell | parse args, build `EncodeRequest`, mint `request_id=uuid7()` |
| 2 | sdk | frame + send |
| 4 | server | connection layer routes to shard 2 (hash(agent) mod 4 = 2) |
| 5 | dispatch | opcode 0x0103 → `handle_encode` |
| 6 | encode.rs handler | validate (text 30 chars OK, 0 edges OK) |
| 8 | brain-embed | tokenize + forward BGE-small → 384-d vector (~3 ms on CPU) |
| 8 | submit | idempotency lookup — miss |
| 9 | arena | reserve slot 1, slot_version=0 → `MemoryId = pack(shard=2, slot=1, ver=0)` |
| 11 | WAL | append `WalPayload::Encode`, fsync via pwritev2 RWF_DSYNC |
| 13 | apply | write `memories`, `texts`, `timeline` rows in redb wtxn |
| 14 | HNSW | insert vector into shard's memory HNSW |
| 15 | idempotency | stamp `IDEMPOTENCY_TABLE` row in same wtxn |
| 16 | commit | redb wtxn commits |
| 17 | post-commit | enqueue auto_edge / temporal_edge / extractor (all 3 succeed → `pending_stages=[AutoEdge, TemporalEdge, Extractor]`) |
| 18 | publish | `MemoryEncoded` envelope on SubscribeRegistry |
| 19 | response | sent back |
| 21 | shell | receives response, shows "ENCODED LSN 1 · s2/m1/v1 · 21 ms ago" |
| 21 | shell wait | opens subscribe stream for stage events |
| ~70 | auto_edge worker | wakeup-on-enqueue, drain, knn over HNSW, 0 neighbours → publish `StageCompleted{kind:AutoEdge, edges:0}` |
| ~80 | temporal_edge worker | same, 0 predecessor → publish empty |
| ~120 | extractor worker | wakeup, drain `(memory_id, text)` from queue |
| ~125 | classifier tier | GLiNER batched predict, returns spans for "Alice Wong"→Person 0.96, "Acme Corp"→Organization 0.94 |
| ~5125 | LLM tier (if API key set) | call Claude / OpenAI, prompt includes prior entities as anchors, returns `{statements: [{subject:"Alice Wong", predicate:"works_at", object:"Acme Corp", is_stateful:true, confidence:0.92}]}` |
| ~5140 | extractor publish | `StageCompleted{kind:Extractor, entities:2, statements:1, relations:0, status:Succeeded}` |
| ~5141 | shell wait | sees all 3 stage events, prints summary |

### Workers that fire post-commit

- **`auto_edge`**: HNSW k-NN of new vector; for neighbours with cosine ≥ 0.75 (per Wave 1 default), write `SimilarTo` edges through `submit(Write)`.
- **`temporal_edge`**: find predecessor in same context window; if topical similarity ≥ 0.4, write `FollowedBy` edge.
- **`extractor`**: pattern + classifier + LLM tiers in fixed order; output goes to entities, statements, relations tables.
- **`statement_embed`** (Wave 1 W1.1, not yet built): embeds new statements into Statement HNSW.

### Edge cases

- **`deduplicate=true`** → fingerprint lookup in per-(shard, agent, context) BLAKE3 index. Hit → return existing `MemoryId`, `was_deduplicated=true`, no slot allocated, no WAL record.
- **Idempotency replay** → same `request_id` returns cached `EncodeResponse` from `IDEMPOTENCY_TABLE`. Transparent to caller; `was_deduplicated` reports the original value.
- **Embedding failure** → encode fails entirely; no memory created.
- **Bad edge target** (non-existent MemoryId) → encode succeeds; that edge logged + dropped; `edge_results` records the rejection.
- **Crash mid-encode** → WAL replay on shard restart reconstructs the memory; HNSW rebuilds from disk.

### Spec refs

- `spec/09_cognitive_operations/02_encode.md`
- `spec/07_metadata_graph/06_idempotency.md`
- `spec/07_metadata_graph/07_fingerprint_dedup.md`
- `spec/05_storage_arena_wal/`
- `spec/22_extractors/00_purpose.md` (post-commit extractor enqueue)

---

## RECALL — "find memories similar to a cue"

### Wire contract (`spec/09_cognitive_operations/03_recall.md`)

```
RECALL(cue_text, agent_id, context_filter, kind_filter, top_k, include_text,
       include_edges, consistency, request_id)
  → RecallResponse {
      hits: Vec<Hit { memory_id, score, text?, kind, edges?, contributing_retrievers }>,
      total_candidates: u32,
      pre_filter_dropped: u32,
      latency_ms: u32,
    }
```

When a schema is declared (`has_active_schema=true`), RECALL transparently routes through the **hybrid query** path. Otherwise it falls back to memory-only ANN search.

### Code path (request → response)

**Hybrid path (schema declared):**

1. **handler** validates, embeds cue text.
2. **brain-planner / hybrid/router.rs** classifies the query:
   - Entity-anchored ("who is X?") → graph 2.0, semantic 1.0
   - Exact term ("error code 500") → lexical 2.0, semantic 1.0
   - Paraphrase ("how do I...") → semantic 1.5, lexical 1.0, graph 0.7
3. **brain-planner / hybrid/planner.rs** builds an `ExecutionPlan` DAG: routing → pre-filters → retrievers (parallel) → fusion → post-filters → limit.
4. **brain-planner / hybrid/executor.rs** runs the plan:
   a. Pre-filters (type, temporal, confidence) push down to retrievers where possible.
   b. **Semantic retriever** — HNSW k-NN search; corpus is memory + statement HNSW (statement corpus empty until W1.1 lands).
   c. **Lexical retriever** — tantivy BM25 over memory_text + statement_text indexes (k1=1.2, b=0.75).
   d. **Graph retriever** — Star (entity → neighbours), Path (memory_id_a → memory_id_b), or Subgraph mode.
5. **brain-planner / hybrid/fusion.rs** — RRF (k=60): `score(d) = Σ weight_i / (k + rank_i(d))`. Top-N capped at 100 per retriever.
6. **Post-filter chain**: Type → Temporal → Confidence → Tombstone → Supersession → Limit (top_k).
7. Response built with `contributing_retrievers` per hit (for explainability).

**Substrate-only path (no schema):**

Steps 2-6 collapse to a single HNSW search over the memory corpus. Simpler, ~5 ms p99.

### Worked example: `recall "who is Alice?" --top-k 5 --include-text`

| T (ms) | Where | What happens |
|---|---|---|
| 0 | handler | validate, classify cue |
| 2 | brain-embed | embed "who is Alice?" → 384-d vector |
| 3 | router | classify as "entity-anchored" → graph weight 2.0, semantic 1.0 |
| 4 | planner | build DAG: 3 retrievers, fusion, top-K=5 |
| 4 | executor | launch 3 retrievers via tokio::join! |
| 6 | semantic | HNSW search ef=64, returns top-100 memory candidates |
| 7 | lexical | tantivy BM25 on "alice" returns top-100 (most have "Alice" in text) |
| 9 | graph | look up entity "Alice Wong" via `entity_lookup_by_canonical_name` → EntityId → walk `mentioned_in` predicate → memories that mention her |
| 11 | fusion | RRF k=60 with weights {semantic:1.0, lexical:1.0, graph:2.0} → unified rank |
| 12 | post-filter | drop tombstoned, drop superseded statements (none here) → top-5 |
| 13 | text fetch | for each of 5 hits, read text from `texts` table |
| 14 | response | hits + `contributing_retrievers=["semantic","lexical","graph"]` per hit |

### Consistency modes

- **`Eventual`** (default) — return whatever's currently in HNSW. Memory encoded in last ~10 ms may not appear.
- **`ReadAfterWrite`** — wait until the agent's most recent encode's HNSW publication completes (via barrier on per-agent epoch). Adds ~5-10 ms.

### Edge cases

- **Empty corpus** → 0 hits, graceful.
- **All retrievers timeout** → return partial results with `degraded=true` in response.
- **Tombstoned memory** → filtered out of results; if a hit appears mid-fusion, the post-filter chain drops it (atomic with the tombstone txn).
- **Score ties** → secondary sort by `created_at_unix_nanos` DESC.

### Spec refs

- `spec/09_cognitive_operations/03_recall.md`
- `spec/23_retrievers/01_rrf_fusion.md`
- `spec/24_hybrid_query/00_purpose.md`
- `spec/16_benchmarks_acceptance/05_recall_quality.md`

---

## PLAN — "find a path between two memories"

### Wire contract (`spec/09_cognitive_operations/04_plan.md`)

```
PLAN(start_memory_id, goal_memory_id, depth_max, edge_kinds_allowed, request_id)
  → PlanResponse {
      path: Vec<PathStep { memory_id, edge_kind, weight }>,
      cost: f32,
      depth: u8,
      explored_nodes: u32,
    }
```

### Code path

1. **handler** validates: both memories exist, depth ≤ 5 (hard cap), kinds non-empty.
2. **brain-planner / planner/path.rs** builds a `PathPlan` (BFS-shaped DAG over edge tables).
3. **brain-planner / executor/path.rs** runs:
   a. Open redb rtxn.
   b. BFS from `start_memory_id`. At each node, walk outgoing edges via `EDGES_OUT_TABLE`, filter by `edge_kinds_allowed`.
   c. Cost function: `edge.weight × type_weight(edge.kind)` (e.g., Caused=1.5, FollowedBy=1.0, SimilarTo=0.7).
   d. Early-terminate when `goal_memory_id` reached or depth_max exceeded.
   e. Reconstruct path by back-tracking parent pointers.
4. Response includes the full step list + total cost.

### Worked example: `plan 0x000200...01 0x000200...05 --depth-max 3`

Setup (encoded earlier):
- M1: "auth-rewrite epic kicked off"
- M2: "auth-rewrite design review held" (`FollowedBy` M1)
- M3: "auth-rewrite implementation 60%" (`FollowedBy` M2)
- M4: "auth-rewrite caused incident" (`Caused` M3)
- M5: "auth-rewrite epic shipped" (`FollowedBy` M3)

| T (ms) | Where | What happens |
|---|---|---|
| 0 | handler | validate, both exist, depth=3 OK |
| 1 | planner | build PathPlan |
| 2 | executor | open rtxn |
| 3 | BFS | from M1, walk `edges_out`: → M2 (FollowedBy, w=1.0) |
| 4 | BFS | from M2, → M3 (FollowedBy, w=1.0) |
| 5 | BFS | from M3, → M4 (Caused, w=1.5×1.5=2.25), → M5 (FollowedBy, w=1.0); both explored |
| 6 | BFS | goal M5 reached — terminate |
| 7 | reconstruct | path = [M1→M2→M3→M5] |
| 8 | cost | 1.0 + 1.0 + 1.0 = 3.0 |
| 8 | response | path, cost=3.0, depth=3, explored_nodes=4 |

### Edge cases

- **No path within depth** → empty path, `cost=inf`, `depth_max_reached=true`.
- **Cycle in graph** → visited-set in BFS prevents revisiting (each MemoryId visited once).
- **Symmetric edge** (e.g. `SimilarTo`) → traversed in both directions if both in `edge_kinds_allowed`.

### Spec refs

- `spec/09_cognitive_operations/04_plan.md`
- `spec/16_benchmarks_acceptance/02_latency_targets.md` (PLAN p99=18 ms)

---

## REASON — "derive inferences from an observation"

### Wire contract (`spec/09_cognitive_operations/05_reason.md`)

```
REASON(observation, depth, top_k, kind_filter, request_id)
  → ReasonResponse {
      inferences: Vec<Inference { memory_id, score, derivation_path, kind }>,
      explored_nodes: u32,
    }
```

REASON is a step beyond RECALL: instead of just finding similar memories, it follows edges from the cue's near-neighbours to surface **derived** inferences. The cost model rewards short, high-weight paths.

### Code path

1. **handler** embeds observation, runs initial RECALL to find top-`m` (default 5) anchor memories.
2. **brain-planner / planner/reason.rs** for each anchor: traverse edges up to `depth`, collecting "reachable" memories with weighted distance.
3. **executor**:
   a. Score each reachable memory as `cosine(observation, mem) × Π edge.weight`.
   b. Rank, take top-`k`.
4. Each inference carries its derivation path so the caller can explain the result.

### Worked example: `reason "is the auth-rewrite stable?" --depth 2 --top-k 3`

Setup: same memories M1-M5 as PLAN example, plus M6: "auth-rewrite tests passing" (`FollowedBy` M3).

| T (ms) | Where | What happens |
|---|---|---|
| 0 | handler | embed observation |
| 4 | initial recall | top-m=5 anchors: M3 (impl 60%), M4 (caused incident), M5 (shipped), M6 (tests passing), M2 (design review) |
| 5 | reason | for each anchor, BFS depth-2 collecting neighbours + derivation path |
| 6 | from M3 | →M4 (Caused, w=1.5), →M5 (FollowedBy, w=1.0), →M6 (FollowedBy, w=1.0) |
| 6 | from M4 | (no outgoing — incident is a sink) |
| 7 | from M5 | (shipped — no outgoing) |
| 8 | scoring | rank by `cosine × Π weights`: M4 score=0.85 (incident contradicts stable), M5 score=0.78 (shipped supports stable), M6 score=0.72 (tests pass supports stable) |
| 9 | top-K | M4, M5, M6 with derivation paths |
| 10 | response | 3 inferences + paths |

The caller now sees: "the agent observed M4 (caused incident) closest to your question, then M5 (shipped) and M6 (tests passing) — mixed signal."

### Spec refs

- `spec/09_cognitive_operations/05_reason.md`
- `spec/16_benchmarks_acceptance/02_latency_targets.md` (REASON p99=35 ms)

---

## FORGET — "tombstone or hard-delete a memory"

### Wire contract (`spec/09_cognitive_operations/06_forget.md`)

```
FORGET(memory_id, mode, reason, request_id)
  → ForgetResponse {
      memory_id,
      now_state: Tombstoned | Reclaimed,
      cascade_count: u32,        // dependent statements affected
      committed_at: u64,
    }
```

`mode = Soft` (default): mark tombstoned, 7-day grace period before reclamation. `mode = Hard`: zero the bytes immediately, fail-stop on any reference.

### Code path

1. **handler** validates memory exists, not already tombstoned.
2. **brain-ops / apply/memory.rs::apply_tombstone_memory** builds `Phase::Tombstone { target: Memory{id, mode} }`.
3. **writer/submit.rs** path:
   a. WAL append (`WalPayload::Forget`).
   b. apply: set `tombstoned=true, tombstoned_at_unix_nanos=now` in `memories` table; mode=Hard also zeros the arena slot.
   c. **HNSW**: mark node as "deleted" (lazy — node stays until next compaction); RECALL filters tombstoned out.
   d. Edge tables: edges stay; queries skip them when either endpoint is tombstoned.
4. **Post-commit cascade**:
   a. `forget_cascade` worker enqueued.
   b. Worker re-derives or supersedes-with-null any statement that cited this memory as evidence (Wave 1 W1.5).
5. Response with cascade count.

### Worked example: `forget 0x000200000000000400000001 --mode soft --reason "duplicate"`

Setup: M4 is "auth-rewrite caused incident" with two derived statements: S1 ("auth-rewrite, brain:caused, incident-321"), S2 ("incident-321, brain:occurred_at, 2026-04-15").

| T (ms) | Where | What happens |
|---|---|---|
| 0 | handler | validate, build Phase::Tombstone |
| 3 | WAL | append Forget payload, fsync |
| 5 | apply | set memories.tombstoned=true |
| 6 | idempotency | stamp |
| 7 | commit | wtxn commits |
| 8 | post-commit | enqueue forget_cascade with `memory_id=M4` |
| 9 | response | now_state=Tombstoned, cascade_count=0 (cascade async) |
| 10 | shell | shows "FORGOT 0x...04 (soft, will reclaim in 7 days)" |
| ~100 | forget_cascade worker | drain, find dependent statements via `statement_list(filter=evidence_includes(M4))` → S1, S2 |
| ~102 | re-derive S1 | if other evidence remains, keep; if M4 was sole evidence, supersede-with-null (write new version with `confidence_floor=0`, mark `evidence_orphaned`) |
| ~104 | re-derive S2 | same |
| ~105 | publish | `StatementsCascadedFromForget{memory_id=M4, statements_affected:2}` event |
| 7 days | reclamation worker | M4 still tombstoned and past grace → arena slot zeroed, `memories` row removed, `slot_versions` bumps |

### Mode differences

| | Soft | Hard |
|---|---|---|
| Arena bytes | preserved until reclamation | zeroed immediately |
| Grace | 7 days | none |
| Recoverable | yes (until reclamation) | no |
| Cascade | async via forget_cascade worker | synchronous (must complete before ack) |
| Use case | normal user-driven forget | GDPR / sensitive data removal |

### Edge cases

- **Memory has incoming edges** → edges remain; queries skip them.
- **Memory already tombstoned** → idempotent; returns same response.
- **Cascade fails mid-flight** → WAL replay reconstructs; cascade re-enqueued.
- **Hard FORGET of memory used as evidence in active statement** → statement supersedes-with-null synchronously; statement chain audit row records the loss.

### Spec refs

- `spec/09_cognitive_operations/06_forget.md`
- `spec/17_knowledge_model/03_composition.md` Rule 3 (re-derivation on FORGET)
- `spec/25_provenance/` (provenance + audit)

---

## Cross-primitive lifecycle: how a memory ages

```
T+0       ENCODE("Alice works at Stripe", deduplicate=false)
                  ↓
              [memory M1 created, vector in HNSW, "Alice"→Person +
               "Stripe"→Organization extracted, statement S1 (works_at) extracted]

T+1h      ENCODE("Alice now at OpenAI")
                  ↓
              [memory M2, extractor proposes statement S2 (works_at),
               supersedes S1 because `works_at` is stateful=true;
               S1.is_current=false, S1.valid_to=now]

T+30d     auto_edge worker firing on M2 spotted similarity to M1 → SimilarTo edge added.
          confidence_sweep worker (W1.4) decays S1.confidence from 0.92→0.78 (90-day half-life).

T+90d     User: FORGET(M1, mode=Hard, reason="GDPR request")
                  ↓
              [WAL Forget, M1 zeroed, S1's evidence ref to M1 dropped; S1 supersede-with-null
               written (already superseded by S2 anyway, so cosmetic).]

T+90d     RECALL("where does Alice work?")
                  ↓
              [hybrid query → graph retriever finds Alice EntityId →
               walks works_at predicate → S2 (current, valid) → response: "OpenAI"]
```

This is the full Brain agent-memory loop: encode → extract → relate → supersede → forget → recall, with the typed graph persisting "what's true now" across the whole cycle.

---

# Part III — How this plan ties back to your repo state

The Brain workspace at `/Users/dodo/Desktop/brain/` is roughly:

```
crates/
├── brain-core         — shared types (MemoryId, Statement, Entity, EdgeKind, …)
├── brain-protocol     — wire format + DSL parser
├── brain-storage      — arena + WAL + recovery
├── brain-metadata     — redb tables (memory + entity + statement + relation + audit)
├── brain-index        — HNSW (memory + entity + statement) + tantivy
├── brain-embed        — BGE-small embedding service
├── brain-extractors   — pattern + GLiNER + LLM extractors
├── brain-llm          — Anthropic + OpenAI clients + router + cache
├── brain-planner      — query planner + hybrid retrieval
├── brain-ops          — handlers/apply/writer (one write path)
├── brain-workers      — 16 background workers per shard
├── brain-http         — HTTP/WS transport
├── brain-server       — binary wiring it all together
├── brain-sdk-rust     — async Rust SDK
├── brain-shell        — interactive REPL
└── brain-cli          — admin CLI
```

The plan in Part I + the walk-through in Part II are scoped to make this workspace ship a credible v1.0 memory-layer database in 11-13 weeks of focused work on the database core.

---

## Sources

- Arc-labs / Recall — `/Users/dodo/Desktop/arc-labs/` (deep architecture read)
- Brain spec — `/Users/dodo/Desktop/brain/spec/` (§00-§31)
- MemGPT/Letta — https://arxiv.org/pdf/2310.08560
- Mem0 — https://arxiv.org/html/2504.19413v1
- Zep / Graphiti — https://arxiv.org/html/2501.13956v1
- Anthropic memory tool — https://www.anthropic.com/news/context-management
- Anthropic context engineering — https://www.anthropic.com/engineering/effective-context-engineering-for-agents
- LangMem — https://langchain-ai.github.io/langmem/
- Cognee — https://www.cognee.ai/blog/fundamentals/ai-memory-in-five-scenes
- RRF / hybrid retrieval — https://opensearch.org/blog/introducing-reciprocal-rank-fusion-hybrid-search/
- Prompt caching — https://genta.dev/resources/prompt-caching-llm-guide
- Anthropic contextual retrieval (49% retrieval-failure cut) — https://www.datacamp.com/tutorial/contextual-retrieval-anthropic
- MemoryBank — https://arxiv.org/pdf/2305.10250
- OpenAI temporal agents cookbook — https://cookbook.openai.com/examples/partners/temporal_agents_with_knowledge_graphs/temporal_agents
- Mem0/Zep benchmark dispute — https://blog.getzep.com/lies-damn-lies-statistics-is-mem0-really-sota-in-agent-memory/
