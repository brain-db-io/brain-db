# Phase 26 — Single-path consistency (kill mode bifurcations)

## Goal

A client calling `RECALL("foo")` against any healthy Brain shard MUST go through one code path, hit one set of retrievers, and produce results whose shape is independent of what's been uploaded to the shard. Same for `ENCODE`. Today the codebase has six mode bifurcations (audited below) that violate this — the shard quietly takes a different path depending on whether a schema is declared, whether a model loaded, whether tantivy initialised. This phase collapses all of them.

Three themes, in priority order:

1. **Tantivy is mandatory.** The lexical retriever is never `None`. If `TantivyShard::open` fails at shard spawn, the shard fails to spawn (a degraded shard is worse than a missing one — operators must know).
2. **Schema is always-on and associative.** Every shard has an internal schema from byte zero (the system namespace `brain` is already seeded). User `SCHEMA_UPLOAD` calls **merge** into this baseline additively; they don't flip a mode. The runtime never branches on "is a schema declared" because the answer is always yes.
3. **Capabilities are hard-required at spawn.** Cross-encoder model, extractor registry, embedding model — if any fail to load and the operator hasn't explicitly opted them out, the shard refuses to start. No silent degradation.

## Prerequisites

- [x] Always-PQ pivot landed (phase 25 commits `2b8de57`…`951ceb9`).
- [ ] Test suite green on the always-PQ shape (the 3 known failures from the post-pivot run are unrelated and tracked separately).

## Reading list

1. `spec/05_operations/03_read_pipeline.md` — current "substrate vs hybrid" framing that must go.
2. `spec/03_schema/00_purpose.md` + `01_grammar.md` — schema model; merge semantics need adding.
3. `spec/04_wire_protocol/03_opcodes.md` §SCHEMA_UPLOAD — wire contract changes (merge, not replace).
4. `spec/11_extractors/00_purpose.md` — extractor pipeline currently runs only when schema declared; needs to always run.
5. `spec/13_retrievers/05_hybrid_query.md` — "hybrid" framing was already redundant; collapse with §13 retriever fan-out.
6. `crates/brain-ops/src/handlers/recall.rs:44–60` — the `substrate_recall` vs `hybrid_recall` branch (the canonical example of the bug).
7. `crates/brain-ops/src/state/schema_gate.rs` — the `bool` toggle to be removed.
8. `crates/brain-server/src/shard/mod.rs:1374–1401` — the tantivy-fallback-to-None branch.

## Outputs

- One code path through `RECALL`, `ENCODE`, `PLAN`, `REASON`, and the typed-graph wire ops.
- `SchemaGate` deleted. Planning inputs no longer carry `has_active_schema`.
- `OpsContext.lexical_retriever`, `.semantic_retriever`, `.graph_retriever`, `.cross_encoder` change from `Option<Arc<dyn _>>` to `Arc<dyn _>` (cross-encoder may keep an explicit `Disabled` variant if the operator opts it out — see T3.2).
- `SCHEMA_UPLOAD` becomes associative-merge with explicit conflict semantics.
- New wire op `GET_CAPABILITIES` returns the shard's enabled features so clients can introspect.
- Updated spec sections (§03, §05, §11, §13).

## Sub-tasks

### Theme 1 — Tantivy mandatory

#### Task 26.1 — `TantivyShard::open` failure becomes shard spawn failure

**Reads:** `crates/brain-server/src/shard/mod.rs:1374–1401`, `crates/brain-index/src/tantivy_shard/mod.rs::open`
**Writes:** `crates/brain-server/src/shard/mod.rs`, `crates/brain-server/src/shard/error.rs`
**What to build:**
- Replace the `match … Err(_) => None` arm with `?` propagation through `ShardError::TantivyInitFailed { source: TantivyShardError }`.
- Tantivy `IndexStatus::NeedsRebuild` is **not** a spawn failure — it's an expected post-recovery state that the maintenance worker handles. The rebuild-required signal stays internal.
- Snapshot-restore failures (`tantivy_recovery::recover_tantivy_on_open` returning `Err`) become spawn failures.

**Done when:**
- A test that nukes `<shard_dir>/memory_text.tantivy/meta.json` mid-shutdown and tries to re-spawn the shard sees a structured `ShardError::TantivyInitFailed` rather than a quietly-degraded shard.
- The "lexical retrieval unavailable" `tracing::error!` log is gone.

#### Task 26.2 — `OpsContext.lexical_retriever: Arc<dyn LexicalRetriever>` (non-optional)

**Reads:** `crates/brain-ops/src/context.rs`, every site that consumes `ctx.lexical_retriever`
**Writes:** `crates/brain-ops/src/context.rs`, `crates/brain-planner/src/hybrid/executor.rs` (HybridExecutorContext.lexical), all call sites
**What to build:**
- Drop `Option` from the three retriever fields in `OpsContext` and `HybridExecutorContext`.
- Constructor takes the retrievers as required arguments; tests pass mock impls.

**Done when:**
- `grep -r 'lexical_retriever:\s*Option' crates/` returns zero results.
- All tests compile against the mandatory shape.

#### Task 26.3 — Delete `substrate_recall`; collapse into one `handle_recall`

**Reads:** `crates/brain-ops/src/handlers/recall.rs`
**Writes:** `crates/brain-ops/src/handlers/recall.rs`
**What to build:**
- Delete `substrate_recall` and `ctx.is_substrate_only()`.
- Rename `hybrid_recall` to be the only path. Drop the `HybridRecallOutcome::Frame(_)` wrapper — return the frame directly.
- Update `crates/brain-ops/src/handlers/plan.rs` and `reason.rs` similarly (any "substrate vs hybrid" branches there).

**Done when:**
- `handle_recall` is a straight line from validate → embed → fan-out → fuse → filter → enrich → response.
- `grep -rn 'substrate_recall\|substrate_plan\|substrate_reason' crates/` returns zero results.

#### Task 26.4 — Test fixture migration

**Reads:** any test that constructs `OpsContext` without a lexical retriever
**Writes:** the affected test files
**What to build:**
- A shared `test_support::tantivy_for_tests()` helper that builds a real `TantivyShard` against a `tempfile::TempDir`. Used by every test fixture instead of "leave lexical = None".
- Delete tests that explicitly asserted the substrate-only path's existence (e.g., "recall works without tantivy").

**Done when:**
- No test relies on `lexical_retriever = None` to exercise a code path.
- The full integration test suite (including the previously-passing 137 `brain-ops` integration tests) still passes.

### Theme 2 — Schema becomes always-on + associative

#### Task 26.5 — Delete `SchemaGate`

**Reads:** `crates/brain-ops/src/state/schema_gate.rs`, every site that calls `ctx.schema_gate.is_declared()`
**Writes:** delete `state/schema_gate.rs`; update every consumer
**What to build:**
- Delete the `SchemaGate` type and the `OpsContext.schema_gate` field.
- Where callers passed `has_active_schema: ctx.schema_gate.is_declared()` into planners, drop the argument entirely. The planner always plans the full pipeline.
- Where wire-op handlers gated on `is_declared()` (e.g., refusing ENTITY_CREATE in schemaless mode), replace with a positive check: "does the schema have an entity type named X?" — if no, return `OpError::EntityTypeNotInSchema { name }`. If yes, proceed.

**Done when:**
- `grep -rn 'schema_gate\|is_declared\|has_active_schema' crates/` returns zero results outside spec docs.
- Typed-graph ops (ENTITY_CREATE, STATEMENT_CREATE, …) work the moment the corresponding type/predicate exists in any namespace — no "schema must be uploaded first" gating, only "this specific entity type must exist."

#### Task 26.6 — Schema merge semantics in `SCHEMA_UPLOAD`

**Reads:** `spec/03_schema/`, `crates/brain-ops/src/handlers/schema.rs::handle_schema_upload`
**Writes:** `crates/brain-ops/src/handlers/schema.rs`, `crates/brain-metadata/src/schema/store.rs`
**What to build:**
- `SCHEMA_UPLOAD { namespace, dsl }` becomes **additive**:
  - Parse the new DSL into entity_types, predicates, relation_types, extractor_defs.
  - For each new declaration, look up the same name in the current active schema for that namespace.
  - **No prior declaration → insert.** Bump namespace version.
  - **Same declaration (byte-equal constraints) → no-op.** Idempotent re-upload.
  - **Conflicting declaration → reject** with `OpError::SchemaConflict { kind, name, namespace, conflict: String }`. The merge is **all-or-nothing** for a single upload (one conflict aborts the whole upload — no half-merged state).
- The whole-replacement semantics moves to a new opcode `SCHEMA_REPLACE { namespace, dsl }` (T-2.7 below) for the rare destructive case.

**Done when:**
- Upload schema-A, then schema-B-additive — both sets of types are queryable.
- Upload schema-A, then schema-B-conflicting — second upload returns `SchemaConflict`, first remains active.
- Re-upload schema-A unchanged — no-op (idempotent), no version bump.

#### Task 26.7 — Wire `SCHEMA_REPLACE` for the destructive case

**Reads:** `spec/04_wire_protocol/03_opcodes.md`, `crates/brain-protocol/src/ops/schema.rs`
**Writes:** new opcode + handler
**What to build:**
- `SchemaReplaceReq = 0x0124`, `SchemaReplaceResp = 0x01A4` (next free in the schema-ops range — verify slot is free).
- Handler tombstones all existing types in the namespace and replaces with the uploaded set. Existing rows that reference removed types stay as orphans (read via `kind = MemoryKind::Episodic`, no enrichment from typed-graph tables).
- Admin-only permission.

**Done when:**
- Round-trip test for the wire shape.
- Integration test: upload schema, encode entities, SCHEMA_REPLACE with disjoint schema, verify old entities are no longer enriched but still recallable as plain memories.

#### Task 26.8 — Always-on extractor pipeline

**Reads:** `spec/11_extractors/`, `crates/brain-ops/src/handlers/encode.rs` (extractor invocation), `crates/brain-extractors/src/extractor.rs`
**Writes:** the same files
**What to build:**
- Drop the `has_active_schema` planning input.
- Extractor pipeline runs on every ENCODE.
- Extracted entities/statements are filtered against the current schema: only entities whose type exists in the schema are persisted to typed-graph tables.
- If no schema types match an extracted entity, it's silently dropped (not an error — extraction is best-effort).

**Done when:**
- An ENCODE on a shard with empty user schema (just the system `brain` namespace) runs the extractor; any matches against Person/Organisation/Place from the system namespace are persisted; matches against undeclared types are dropped.
- An ENCODE on a shard with a schema declaring `Project` extracts a Person and a Project; both persist.

### Theme 3 — Hard-fail capabilities (no silent degradation)

#### Task 26.9 — Cross-encoder: hard error when requested but absent

**Reads:** `crates/brain-ops/src/handlers/recall.rs:610`, `query.rs:349`, `crates/brain-rerank/`
**Writes:** the same + `crates/brain-ops/src/error.rs`
**What to build:**
- Cross-encoder remains optional at the operator level (some deployments don't want it). Make this an **explicit deployment choice**: `Config.rerank.enabled: bool`. Default `true`.
- At shard spawn, if `rerank.enabled == true` and the model fails to load → `ShardError::CrossEncoderInitFailed`.
- At shard spawn, if `rerank.enabled == false` → `OpsContext.cross_encoder = Disabled` (not `None` — explicit sentinel).
- At request time, if `request.rerank == true` and `cross_encoder == Disabled` → `OpError::CapabilityNotEnabled { capability: "rerank" }`. Client gets a clear signal.

**Done when:**
- The previous silent "rerank=true → ignored" behaviour is gone.
- A test exercises both branches: enabled-with-model-load-failure (spawn fails) and disabled-with-request (request fails with a clear error code).

#### Task 26.10 — Extractor registry: same pattern

**Reads:** `crates/brain-extractors/src/registry.rs`, `crates/brain-ops/src/handlers/encode.rs`
**Writes:** the same
**What to build:**
- Operator config gates which extractors are enabled per tier (pattern, classifier, LLM).
- If a tier is enabled in config but the model fails to load → shard spawn failure.
- If a tier is disabled by config → the tier is skipped silently (operator opted out, not a degradation).
- Drop the `has_llm_extractor` planning input; the pipeline always runs whichever tiers are loaded.

**Done when:**
- `grep -rn 'has_llm_extractor\|has_enabled_llm_extractor' crates/` returns zero.
- A bench compares pre-pivot ENCODE latency to post-pivot (extractor always running) — expect a regression on shards that had it disabled by accident; document the cost.

#### Task 26.11 — `GET_CAPABILITIES` wire op

**Reads:** `spec/04_wire_protocol/03_opcodes.md`, capabilities surface design
**Writes:** new opcode + handler + SDK helper
**What to build:**
- `GetCapabilitiesReq = 0x0030` (substrate-control range), `GetCapabilitiesResp = 0x00B0`.
- Response carries `{ rerank: bool, llm_extractor: bool, pattern_extractor: bool, classifier_extractor: bool, schema_namespaces: Vec<String>, ... }`.
- SDK: `client.capabilities() -> Capabilities`.

**Done when:**
- Clients can call `GET_CAPABILITIES` at session start and avoid issuing requests the shard can't serve.
- Round-trip test on the wire shape.

### Theme 4 — Spec sync

#### Task 26.12 — Update §03 (schema), §05 (operations), §11 (extractors), §13 (retrievers)

**Reads:** the named spec sections
**Writes:** the same sections + `spec/01_architecture/07_wedges_and_roadmap.md` (resolves the "schema-gated bifurcation" line if it exists)
**What to build:**
- §03 gains a "Schema Merge Semantics" subsection (matches T2.6).
- §05/03 (read pipeline) drops "substrate vs hybrid" framing.
- §11 reframes "extractors activate on schema declaration" as "extractors always run; persistence is gated on declared types."
- §13 reframes "hybrid query" as just "the query pipeline" — no mode.
- Mark a follow-up in §01.07 wedges that v1.x consolidated mode bifurcations.

**Done when:**
- A reader of the spec can no longer find any reference to "schemaless mode" or "schema must be declared first."

## Verification

Per the project's standard:

1. `cargo check --workspace --lib --tests` clean.
2. `cargo clippy --workspace -- -D warnings` clean.
3. `cargo test --workspace` — same pass count as pre-phase, minus the explicitly-deleted "tests substrate-only path" tests.
4. New tests:
   - Tantivy-init-failure ⇒ shard spawn fails (T1.1).
   - Schema additive merge round-trips (T2.6).
   - Schema conflict aborts whole upload (T2.6).
   - `request.rerank=true` against rerank-disabled shard returns `CapabilityNotEnabled` (T3.1).
   - `GET_CAPABILITIES` returns the right flags for a representative config (T4.1).
5. Manual benchmark: ENCODE p99 on a shard with extractors always-on vs the old gated path. Document the regression in the commit; if > 30% slower, add a phase-26-perf follow-up.

## Risks + stop conditions

| Risk | Mitigation |
|---|---|
| Tantivy mandatory means every shard pays the disk + RAM cost. A shard with 100 memories pays the same overhead as one with 100M. | Acceptable in v1 — keeps the model simple. Spec'd as "lexical retrieval is a core capability." |
| Always-on extractor pipeline regresses ENCODE latency on shards that don't have user schemas. | Bench in T3.2. If regression > 30%, gate the LLM extractor tier behind config (opt-out, but explicit). |
| Schema merge conflict semantics are now load-bearing — operators upload a conflicting schema and get a clear error, but the previous version stays active. Could be confusing. | Document in §03 with examples. Surface in the SDK via a `SchemaConflict` typed error. |
| `SCHEMA_REPLACE` is destructive and may orphan rows. | Admin-only; require an explicit confirm flag (`force_drop_existing: true`). |
| The cross-encoder hard-fail change means deployments that previously "just worked" without the reranker model on disk will now fail to spawn. | Config default `rerank.enabled = true` — operators get a spawn failure with a clear message. Migration note in the changelog. |

**Stop condition:** if T2.6 (schema merge) hits a metadata-layer limitation where the existing `predicates` / `relation_types` tables can't represent additive merges cleanly (e.g., `is_schema_declared: bool` on rows forces a binary state), escalate to the user: this phase's scope grows to include a metadata schema migration.

## Spec edits proposed alongside

(For review before any code; mirrors the pattern from phase 25.)

1. **`spec/05_operations/03_read_pipeline.md`** — rewrite the "Recall has two modes" section as a single flow.
2. **`spec/03_schema/01_grammar.md`** — append "Merge Semantics" subsection: associative additive, conflict-rejects-whole-upload, idempotent re-upload.
3. **`spec/03_schema/04_lifecycle.md`** (new file or section) — covers `SCHEMA_UPLOAD` (merge) vs `SCHEMA_REPLACE` (destructive), versioning, orphan handling.
4. **`spec/11_extractors/00_purpose.md`** — drop "activates when schema declared"; replace with "always runs; persistence is gated on declared types."
5. **`spec/13_retrievers/05_hybrid_query.md`** — rename "hybrid query" → "query pipeline"; drop the "hybrid means typed-graph mode" framing.
6. **`spec/04_wire_protocol/03_opcodes.md`** — register `GetCapabilitiesReq/Resp`, `SchemaReplaceReq/Resp`. Document `SchemaUploadReq` as additive-merge.
7. **`spec/01_architecture/07_wedges_and_roadmap.md`** — mark the "schema-gated bifurcation" item resolved.

## Files to be modified — at a glance

- **Delete**: `crates/brain-ops/src/state/schema_gate.rs` (~170 LOC).
- **Delete**: `substrate_recall` body + helpers in `crates/brain-ops/src/handlers/recall.rs` (~50 LOC).
- **Modify**: ~25 callers of `schema_gate.is_declared()` / `has_active_schema` / `has_llm_extractor`.
- **Modify**: `crates/brain-server/src/shard/mod.rs` (tantivy spawn-failure propagation, ~20 lines).
- **Modify**: `crates/brain-ops/src/handlers/schema.rs` (merge semantics, ~150 LOC added).
- **New**: `crates/brain-protocol/src/ops/admin.rs` — `GetCapabilities` types.
- **New**: `crates/brain-ops/src/handlers/admin/capabilities.rs` — handler (~80 LOC).
- **New**: `crates/brain-ops/src/handlers/schema_replace.rs` — destructive replace (~120 LOC).
- **Modify**: ~10 test fixtures to wire real `TantivyShard` instead of `None`.
- **Modify**: 7 spec files.

Estimated diff: ~2K lines of changes + ~400 lines of new tests. Comparable to phase 25 in scope.

## Order of execution

Recommend landing in this order — each commit leaves the workspace green:

1. **T1.1 + T1.2** (tantivy mandatory + spawn-failure) — smallest change, biggest win, isolates blast radius.
2. **T1.3 + T1.4** (delete substrate_recall + test fixtures) — depends on T1.2.
3. **T2.5** (delete SchemaGate, replace `is_declared` checks with positive schema lookups) — independent.
4. **T2.6 + T2.7** (schema merge + SCHEMA_REPLACE) — depends on T2.5.
5. **T2.8** (always-on extractor pipeline) — depends on T2.5.
6. **T3.1 + T3.2** (capability hard-fail) — independent of T1/T2 but easier with them done.
7. **T3.3** (`GET_CAPABILITIES`) — new wire op, slot in after the above.
8. **T4.1** (spec sync) — last, after the code stabilises.

Total estimated work: ~3-4 focused sessions, comparable to phase 25.
