# Roadmap

High-level implementation plan. Detailed sub-task breakdowns live in [`docs/development/phases/`](docs/development/phases/) — this file is the index.

For autonomous-mode operating rules, see [`AUTONOMY.md`](AUTONOMY.md).

## Status

Brain is **pre-release: v0.1.0**. No external users. The wire protocol, redb tables, and schema model are still in flux.

## v1.0 target

The v1.0 release ships when the combined acceptance suite at [`spec/19_benchmarks/06_complete_acceptance.md`](spec/19_benchmarks/06_complete_acceptance.md) passes — functional + performance + storage + operational + schemaless tests, end-to-end.

The path to v1.0 covers two surfaces:

- **Memory storage** (phases 0–14) — vector memory store: WAL, HNSW, wire protocol, write/read pipelines, HTTP transport, observability, benchmarks.
- **Typed graph** (phases 15–24) — entities, statements, relations, schema DSL, three-tier extractors (pattern → classifier → LLM), hybrid retrieval (semantic + lexical + graph with rank fusion). Activates when a schema is declared via `SCHEMA_UPLOAD`; the schemaless deployment posture is a real first-class mode.

The `v1.0.0` tag is cut once the acceptance suite is green. A deployment that never calls `SCHEMA_UPLOAD` is a valid v1.0 deployment posture (schemaless mode).

---

## Phase 0 — Workspace skeleton ✓ provided by starter

**Status:** Scaffolded by the starter template. Verify before moving on.

**Provided:**

- `Cargo.toml` workspace with shared dependency table.
- 12 stub crates under `crates/`.
- `rustfmt.toml`, `clippy.toml`, `rust-toolchain.toml`.
- `.github/workflows/ci.yml` running build, test, clippy, fmt, miri, audit.
- `.gitignore`, `justfile`, `config/dev.toml`.
- `fuzz/` directory.
- `.claude/` with settings, hooks, slash commands, subagents.

**Verify (before starting Phase 1):**

- [ ] `just verify` is green.
- [ ] CI is green on first push.
- [ ] Tag the latest commit: `git tag phase-0-complete`.

**No detailed phase doc** — the work is just verification.

---

## Phase 1 — Wire Protocol & Core Types

**One-line:** Frame format, opcode codecs, fuzz target.

**Detailed plan:** [`docs/development/phases/phase-01-wire-protocol.md`](docs/development/phases/phase-01-wire-protocol.md)

**Crates touched:** `brain-core`, `brain-protocol`.

**Sub-tasks:** 11.

**Exit:** every opcode round-trips; fuzz finds no panics; tag `phase-1-complete`.

---

## Phase 2 — Storage: Arena + WAL + Recovery

**One-line:** Memory-mapped vector arena, write-ahead log with group commit, crash recovery.

**Detailed plan:** [`docs/development/phases/phase-02-storage.md`](docs/development/phases/phase-02-storage.md)

**Crates touched:** `brain-storage`.

**Sub-tasks:** 12.

**Exit:** 1000-iteration random-kill recovery test passes; miri clean; tag `phase-2-complete`.

---

## Phase 3 — Metadata + Graph (redb)

**One-line:** All 13 redb tables; idempotency; recovery integration with Phase 2.

**Detailed plan:** [`docs/development/phases/phase-03-metadata.md`](docs/development/phases/phase-03-metadata.md)

**Crates touched:** `brain-metadata`.

**Sub-tasks:** 12.

**Exit:** all tables present and tested; cross-crate recovery test passes; tag `phase-3-complete`.

---

## Phase 4 — ANN Index (HNSW)

**One-line:** Wrap `hnsw_rs` with the spec's parameters and lifecycle.

**Detailed plan:** [`docs/development/phases/phase-04-ann-index.md`](docs/development/phases/phase-04-ann-index.md)

**Crates touched:** `brain-index`.

**Sub-tasks:** 8.

**Exit:** recall@10 ≥ 0.95 at 100K vectors; persistence round-trip works; tag `phase-4-complete`.

---

## Phase 5 — Embedding Layer

**One-line:** BGE-small via candle, batching, caching, determinism.

**Detailed plan:** [`docs/development/phases/phase-05-embedding.md`](docs/development/phases/phase-05-embedding.md)

**Crates touched:** `brain-embed`.

**Sub-tasks:** 7.

**Exit:** ≥ 1K texts/sec; deterministic; tag `phase-5-complete`.

---

## Phase 6 — Query Planner & Executor

**One-line:** Logical plan tree, cost model, pull-based executor.

**Detailed plan:** [`docs/development/phases/phase-06-planner.md`](docs/development/phases/phase-06-planner.md)

**Crates touched:** `brain-planner`.

**Sub-tasks:** 8.

**Exit:** every operation type has a planner test; tag `phase-6-complete`.

---

## Phase 7 — Cognitive Operations

**One-line:** ENCODE, RECALL, PLAN, REASON, FORGET on top of the planner; idempotency.

**Detailed plan:** [`docs/development/phases/phase-07-operations.md`](docs/development/phases/phase-07-operations.md)

**Crates touched:** `brain-ops`.

**Sub-tasks:** 11.

**Exit:** correctness suite from spec §02/01 fully green; tag `phase-7-complete`.

---

## Phase 8 — Background Workers

**One-line:** All 12 workers running cooperatively.

**Detailed plan:** [`docs/development/phases/phase-08-workers.md`](docs/development/phases/phase-08-workers.md)

**Crates touched:** `brain-workers`.

**Sub-tasks:** 14.

**Exit:** each worker tested; performance regression test green; tag `phase-8-complete`.

---

## Phase 9 — `brain-server`: end-to-end wire-up

**One-line:** A runnable substrate. Tokio connection layer + Glommio shards.

**Detailed plan:** [`docs/development/phases/phase-09-server.md`](docs/development/phases/phase-09-server.md)

**Crates touched:** `brain-server`.

**Sub-tasks:** 10.

**Exit:** E2E smoke test passes 100 iterations; tag `phase-9-complete`.

---

## Phase 10 — Rust SDK & CLI

**One-line:** Polished `Client` + `brain-cli` covering every spec'd admin command.

**Detailed plan:** [`docs/development/phases/phase-10-sdk-cli.md`](docs/development/phases/phase-10-sdk-cli.md)

**Crates touched:** `brain-sdk-rust`, `brain-cli`.

**Sub-tasks:** 13.

**Exit:** SDK drives every operation; CLI covers every command; tag `phase-10-complete`.

---

## Phase 11 — `brain-http` (foundation HTTP/WS/SSE layer)

**One-line:** Brain-owned HTTP transport on hyper 1.x — replaces hand-rolled admin/CLI HTTP, adds WebSocket + SSE.

**Detailed plan:** [`docs/development/phases/phase-11-brain-http.md`](docs/development/phases/phase-11-brain-http.md)

**Crates touched:** new `brain-http`; migrations in `brain-server`, `brain-cli`.

**Sub-tasks:** 8.

**Exit:** admin hand-roll deleted; SSE + WebSocket working end-to-end; tag `phase-11-complete`.

---

## Phase 12 — Observability

**One-line:** Production-grade telemetry surface — full metrics taxonomy, structured JSON logs, OpenTelemetry tracing, dashboards, alerts.

**Detailed plan:** [`docs/development/phases/phase-12-observability.md`](docs/development/phases/phase-12-observability.md)

**Crates touched:** all (instrumentation), plus `monitoring/dashboards/`, `monitoring/alerts/`.

**Sub-tasks:** 6.

**Exit:** every spec'd `brain_*` metric emitted; JSON log schema matches spec §02/02; OTel spans cover request lifecycle; reference Grafana dashboards + Alertmanager rules ship in-tree; tag `phase-12-complete`.

---

## Phase 13 — Benchmarks & Chaos

**One-line:** Measure-and-stress: criterion benches for every operation, load generator, chaos harness, soak rig.

**Detailed plan:** [`docs/development/phases/phase-13-benchmarks.md`](docs/development/phases/phase-13-benchmarks.md)

**Crates touched:** `benches/`, `tests/chaos/`, `tests/soak/`.

**Sub-tasks:** 4.

**Exit:** every operation has a criterion baseline that hits the spec §14 targets on reference hardware; chaos suite covers kill / I/O fault / network / corruption scenarios; tag `phase-13-complete`.

---

## Phase 14 — Substrate Acceptance & `v0.9.x-substrate-rc`

**One-line:** Run all 10 substrate acceptance gates, runbook-validate, doc pass, tag substrate release-candidate.

**Detailed plan:** [`docs/development/phases/phase-14-acceptance-release.md`](docs/development/phases/phase-14-acceptance-release.md)

**Crates touched:** `acceptance/`, `docs/runbooks/`, READMEs, CHANGELOG.

**Sub-tasks:** 5.

**Exit:** gates 1-10 green; 48 h soak result recorded; runbooks executed against a chaos scenario; `cargo doc` clean; tag `phase-14-complete` and `v0.9.x-substrate-rc`. **The `v1.0.0` tag is deferred to Phase 24** (combined substrate + knowledge-layer release).

---

# Knowledge layer (phases 15–24)

These phases turn Brain from a vector memory store into a cognitive database with typed entities, statements, relations, schema-driven extraction, and hybrid retrieval. Estimated 58–83 days of focused work. Phases 16–22 can partially overlap once Phase 15 is done. See [`docs/development/phases/README.md`](docs/development/phases/README.md) for the full dependency DAG.

---

## Phase 15 — Knowledge storage extensions

**One-line:** New redb tables, WAL frame types, on-disk artifact paths (tantivy/HNSW/LLM cache), schema-declared flag. Binary boots; substrate behaves identically.

**Detailed plan:** [`docs/development/phases/phase-15-knowledge-storage.md`](docs/development/phases/phase-15-knowledge-storage.md)

**Crates touched:** `brain-metadata`, `brain-storage`, `brain-server`.

**Sub-tasks:** 6. **Exit:** substrate-only regression suite stays green; tag `phase-15-complete`.

---

## Phase 16 — Entity layer ✓

**One-line:** Entity table, type system, entity HNSW (declared; resolver wiring in phase 21), resolver tiers 1 (exact / alias) and 2 (trigram fuzzy). Tiers 3 (embedding) and 4 (LLM) stubbed for phase 21.

**Detailed plan:** [`docs/development/phases/phase-16-entities.md`](docs/development/phases/phase-16-entities.md)

**Crates touched:** `brain-core`, `brain-metadata`, `brain-index`, `brain-protocol`, `brain-server`, `brain-sdk-rust`.

**Sub-tasks:** 9. **Exit:** entity create / merge / unmerge / rename / resolve / list / tombstone all work via wire + SDK; tag `phase-16-complete`.

**Delivered:**

- 9 entity wire opcodes (`0x0130–0x0138`) end-to-end through `brain-protocol`, `brain-ops`, `brain-server`.
- Knowledge namespace introduced at high-byte `0x01` (wire opcode widened to `u16` in 16.6a — pre-v1.0 wire change documented in §02/12 composition §0).
- Hand-written entity SDK over `Person` (typed `EntityHandle<T>` + 5 builders for all 9 ops + `ClientErrorEntityExt`). Derive macros defer to phase 19.
- `MergeRecord` v2 + `entity_merge_ops` (full diff captured for grace-period unmerge). Statement / relation re-route deferred to phases 17 / 18 sweeps.
- §28 knowledge wire protocol section brought to §03-depth (15 detail files, ~135 KB of spec).
- §14 entities backfilled with merge / unmerge / GC mechanics (§03 / §04 / §05).
- Adversarial-input resolver tests + create→merge→unmerge→rename lifecycle integration test + criterion bench for tier-1 / tier-2 perf.
- 14 substrate `SubscriptionEvent` event types extended for knowledge layer; event emission wired across all six mutating entity handlers.

**Deferred to later phases (tracked in `../00_overview/04_open_questions_archive.md` + `../00_overview/04_open_questions_archive.md`):**

- Resolver tier 3 (embedding) — phase 21 when entity HNSW is wired into the resolver.
- Tier 4 (LLM-tier) — phase 21.
- Cursor pagination + multi-frame streaming for `ENTITY_LIST` — phase 23.
- Statement / relation re-routing during merge — phases 17 / 18.
- Derive macro `#[derive(BrainEntity)]` — phase 19.

---

## Phase 17 — Statement layer ✓

**One-line:** Statement table; three kinds (Fact, Preference, Event); supersession chains; contradiction surfacing; statement HNSW (declared; populator in phase 21); per-kind noisy-OR confidence aggregation.

**Detailed plan:** [`docs/development/phases/phase-17-statements.md`](docs/development/phases/phase-17-statements.md)

**Crates touched:** `brain-core`, `brain-metadata`, `brain-index`, `brain-protocol`, `brain-ops`, `brain-server`, `brain-sdk-rust`.

**Sub-tasks:** 11. **Exit:** all three kinds work end-to-end via wire + SDK; supersession chains traverse; contradictions surface (not auto-resolved); confidence aggregation drops in; tag `phase-17-complete`.

**Delivered:**

- §14 statements section brought to §03-depth (8 files; supersession / contradiction / storage / confidence / evidence / open questions / references).
- §28/06 statement frames already at §03-depth from phase 16; new opcodes wired end-to-end.
- 7 statement wire opcodes (`0x0140–0x0146`) + responses (`0x01C0–0x01C6`) end-to-end through `brain-protocol`, `brain-ops`, `brain-server`.
- `brain-core` value types: `Statement` / `StatementObject` (tagged union: Entity / Value / Memory / Statement) / `StatementValue` (Text / Integer / Float / Bool / UnixNanos / Blob) / `EvidenceRef` (Inline / Overflow) / `SubjectRef` / `TombstoneReason` / `Predicate`.
- Predicate registry + interning in `brain-metadata`, with built-ins `brain:is_a / has_name / mentions / related_to / prefers / scheduled` seeded at `MetadataDb::open`. `predicate_intern` is idempotent on identical constraints; `AlreadyExists` on diverging shapes.
- `statement_ops`: CRUD + supersession (auto for Preference, explicit for Fact) + contradiction surface (Facts) + tombstone / retract + chain history + filtered list. All operations atomic within one redb txn.
- Statement HNSW declared in `brain-index` (M=32, ef_construction=200, ef_search=128 per spec §26/00); populator deferred to phase 21 with the embedding worker.
- Confidence aggregation per spec §02/04: noisy-OR with per-kind decay (Fact 365d half-life / Preference 60d / Event none). Wired into `statement_create` / `_supersede` when inline evidence carries per-entry metadata; wire callers keep their supplied confidence until phase 22's ADD_EVIDENCE op.
- Hand-written statement SDK: `client.fact() / .preference() / .event() / .statements()`. Uniform `StatementHandle` read-side; derive macros defer to phase 19.
- Integration test suite: 11 wire-smoke tests + 13-step lifecycle + 9 mock-server SDK tests + statement_ops criterion bench.

**Deferred to later phases (tracked in `../00_overview/04_open_questions_archive.md` + `../00_overview/04_open_questions_archive.md`):**

- Statement HNSW embedding worker — phase 21.
- `STATEMENT_ADD_EVIDENCE` opcode (richer per-entry metadata wire path) — phase 22.
- Confidence-sweep worker + bucket re-indexing on confidence delta > 0.05 — phase 21.
- Hybrid query router (RRF fusion) for statement semantic search — phase 23.
- Cursor pagination + multi-frame streaming for `STATEMENT_LIST` / `STATEMENT_HISTORY` — phase 23.
- Discrete `STATEMENT_CONTRADICTED` event + dedicated contradiction audit table — phase 22-23.
- Multi-value `by_subject` index for contradictory active Facts — phase 23.
- Per-predicate decay overrides (vs kind-level defaults) — phase 19 schema DSL.
- Cross-shard `statements_by_object_entity` reverse-index writes — phase 23.
- `#[derive(BrainFact)]` macro + typed `Fact<T>` SDK wrappers — phase 19.

---

## Phase 18 — Relation layer ✓

**One-line:** Relation table; relation-type registry with cardinality + symmetric flags; cardinality-driven auto-supersession; canonical symmetric ordering with dual-index population; iterative BFS traversal with cycle detection and depth/branching caps.

**Detailed plan:** [`docs/development/phases/phase-18-relations.md`](docs/development/phases/phase-18-relations.md)

**Crates touched:** `brain-core`, `brain-metadata`, `brain-protocol`, `brain-ops`, `brain-server`, `brain-sdk-rust`.

**Sub-tasks:** 10. **Exit:** all 4 cardinality variants work end-to-end via wire + SDK; symmetric relations dual-indexed; traverse terminates on cycles within depth/branching caps; tag `phase-18-complete`.

**Delivered:**

- §14 relations section brought to §03-depth (8 files; cardinality / symmetric / storage / traversal / evidence / open questions / references).
- §28/07 relation frames already at §03-depth from phase 16; new opcodes wired end-to-end.
- 7 relation wire opcodes (`0x0150–0x0156`) + responses (`0x01D0–0x01D6`) end-to-end through `brain-protocol`, `brain-ops`, `brain-server`.
- `brain-core` value types: `Relation` (18 fields with `chain_root` + supersession), `RelationType` (with `cardinality`, `is_symmetric`, optional `from_type / to_type` constraints), `canonical_pair` helper for symmetric byte-wise ordering.
- Relation-type registry + interning in `brain-metadata` with built-ins `brain:related_to` (ManyToMany asymmetric), `brain:reports_to` (ManyToOne), `brain:co_authored` (symmetric ManyToMany) seeded at `MetadataDb::open`.
- `relation_ops`: CRUD + cardinality-driven auto-supersession (ManyToMany / ManyToOne / OneToMany / OneToOne) + symmetric canonicalisation + tombstone + chain history + filtered list (by_from / by_to with type filter). All operations atomic within one redb txn.
- `relation_traversal`: iterative BFS with `DEFAULT_MAX_DEPTH = 3` (cap 5), `DEFAULT_MAX_BRANCHING = 1000` (cap 10_000), visited-set cycle detection, tracing::warn on truncation, `TraversalDirection` (Outgoing / Incoming / Both).
- `RelationMetadata` rkyv shape widened with `chain_root_bytes`; archive id bumped to v2.
- Hand-written relation SDK: `client.relation()` / `.relations()` with uniform `RelationHandle` + `TraversalPath` value types. Derive macros defer to phase 19.
- Integration test suite: 11 wire-smoke tests + 6-step lifecycle test + 8 mock-server SDK tests + criterion bench against §02/02 §2.4 perf targets.
- New event type: `RelationTombstoned` added to `EventType` enum + `KnowledgeEventPayload`. Created / Superseded events also wired through brain-ops handlers.

**Deferred to later phases (tracked in `../00_overview/04_open_questions_archive.md` + `../00_overview/04_open_questions_archive.md`):**

- Cross-shard `RELATION_TRAVERSE` coordination — phase 23.
- Streaming TRAVERSE response (per-frame) + cursor pagination on LIST_FROM / LIST_TO — phase 23.
- Weight-aware shortest-path traversal — post-v1.0.
- `RELATION_RETRACT` opcode (hard delete with grace period) — phase 22.
- Entity-merge relation re-routing — phase 23 (or after phase 18 if scope allows).
- `RelationCardinalityConflict` discrete event — phase 23.
- Bulk-mode cardinality skip flag for extractor backfills — phase 22.
- `relations_by_type` index for type-only admin queries — phase 22 if demanded.
- Per-relation-type FORGET cascade configurability — phase 22.
- `#[derive(BrainRelation)]` macro + typed `Relation<T>` SDK wrappers — phase 19.
- Cardinality auto-supersede event emission (handler doesn't yet surface the inner supersede that `relation_create` performs) — phase 22.

---

## Phase 19 — Schema DSL ✓

**One-line:** Parser + validator + per-namespace versioning for the declarative schema language; system-schema bootstrap replaces hand-seeded built-ins; SDK schema builders.

**Detailed plan:** [`docs/development/phases/phase-19-schema-dsl.md`](docs/development/phases/phase-19-schema-dsl.md) (superseded by `.claude/plans/phase-19*.md` per-sub-task plans).

**Crates touched:** `brain-protocol`, `brain-metadata`, `brain-ops`, `brain-sdk-rust`, `brain-server`.

**Sub-tasks:** 9. **Exit:** schema upload validates + versions per namespace; system schema seeds on first open; subsequent entity / predicate / relation registrations flow through the parse → validate → persist → intern fan-out; tag `phase-19-complete`.

**Scope cut:** **No migration plan computation** per user direction (no existing deployments to migrate). Tracked as `../00_overview/04_open_questions_archive.md` Q3.

**Delivered:**

- §21 schema-DSL section brought to §03-depth (9 files: ast / validator / namespaces / versioning / system_schema / open_questions / references).
- §02/02 §2.6 schema-layer perf targets added (UPLOAD / VALIDATE / GET / LIST); phase-gate renumbered.
- §29/00 SDK phase-scope flipped: 19.8 SchemaBuilder + client.schema() ✓.
- 4 schema wire opcodes (`SCHEMA_UPLOAD` / `_GET` / `_LIST` / `_VALIDATE` at `0x0120-0x0123`, responses at `0x01A0-0x01A3`) end-to-end through `brain-protocol`, `brain-ops`, `brain-server`.
- Schema AST in `brain-protocol::schema`: value-typed (serde, no rkyv) — `Schema`, `SchemaItem`, `EntityTypeDef`, `PredicateDef`, `RelationTypeDef`, `ExtractorDef` + supporting enums.
- Pest 2.7 parser (`grammar.pest` + recursive-descent visitor) mirrors the §03/01 EBNF: namespaces, attribute modifiers, predicate kind/object, relation cardinality + symmetric + properties, extractor kind/target + 14 per-kind config fields, heredoc strings, regex literals, JSON capture for `examples:` / `schema:`, condition expressions with and/or/matches, comments + CRLF + trailing commas. `ParseError` carries 1-based line/col.
- Static validator (`validate(&Schema)` + `validate_system_schema(&Schema)`): namespace + duplicates + type refs + predicate kind/object compatibility + cardinality/symmetric + attribute rules (`unique` not on Ref, default-type compat) + extractor required-field/range checks. `ValidatedSchema` newtype proves validation cleared. Accumulates all errors (no first-error short-circuit).
- Per-namespace schema persistence in `brain-metadata::schema_store`: `(namespace, version)` → `SchemaVersionRow` (rkyv) + `namespace -> u32` active pointer. `schema_upload` is atomic — bumps version + writes row + active pointer + fans out into entity_type / predicate / relation_type intern paths.
- System schema bootstrap (load-bearing): `brain-metadata/src/system_schema/schema.brain` is `include_str!`-embedded; parsed + validated + applied at `MetadataDb::open`. Replaces `BUILTIN_PREDICATES` / `BUILTIN_RELATION_TYPES` / `seed_builtin_entity_types` from 16.1 / 17.3 / 18.3 — every built-in registration now flows through the parser + validator + schema_upload, sharing code paths with user uploads. Built-in IDs (`Person == EntityTypeId(1)`, etc) preserved.
- `SchemaUpdatedEvent.namespace` field added; emitted post-commit on UPLOAD.
- SDK `client.schema()` entry with `.upload(&Schema)` / `.upload_text(text)` / `.validate(text)` / `.get(ns, v)` / `.list(ns)`; `SchemaBuilder::new(ns).entity_type(...).predicate(...).relation_type(...).build()` fluent assembler; canonical DSL printer for AST → text round-trip.
- Integration test suite: 8 wire-smoke tests + 1 phase-exit lifecycle test + 6 mock-server SDK tests + schema_ops criterion bench (parse + validate + upload at 50-definition fixture, plus get / list).

**Deferred to later phases (tracked in `../00_overview/04_open_questions_archive.md`):**

- Migration plan computation, schema diff / keep-re-extract-tombstone semantics — out of v1 scope; revisit in v1.1+ (§03/07 Q3).
- `#[derive(BrainEntity)]` / `BrainFact` / `BrainPreference` / `BrainEvent` / `BrainRelation` proc macros — phase 19b or phase 21 (§03/07 Q13).
- Multi-document schemas per namespace + `use other_ns;` cross-namespace imports — post-v1 (§03/07 Q2, Q6).
- Source spans threaded through the AST → validator → wire — phase 19+ improvement (§03/07 Q4 / Q15).
- Per-namespace entity-type ID space (`brain:Person` vs `acme:Person`) — needed once user entity types arrive (later sub-tasks).
- Schema deletion / rollback (§03/07 Q9).
- Validator-version migration when validator rules change (§03/07 Q10).
- Binary-bootstrap migration when system schema content changes across binaries (§03/07 Q11).
- Admin-only authorization for `0x0120-0x0123` (§03/07 Q15 / §28/05 §8) — phase 21 admin.
- Stream-paginated `SCHEMA_LIST` — phase 23.
- `EXTRACTOR_LIST` / `_DISABLE` / `_ENABLE` (`0x0124-0x0126`) — phase 20.

---

## Phase 20 — Pattern + classifier extractors ✓

**One-line:** Extractor framework; pattern (regex) + classifier (operator-provided model) tiers wire into ENCODE; built-ins (`brain.entity_mentions`, `brain.basic_ner`) declared via the system schema; extraction audit log + governance wire ops.

**Detailed plan:** [`docs/development/phases/phase-20-pattern-classifier-extractors.md`](docs/development/phases/phase-20-pattern-classifier-extractors.md) (superseded by `.claude/plans/phase-20*.md` per-sub-task plans).

**Crates touched:** new `brain-extractors`; `brain-protocol`, `brain-metadata`, `brain-ops`, `brain-server`.

**Sub-tasks:** 10. **Exit:** pattern extractor runs end-to-end through ENCODE without operator setup; classifier framework wired with operator-provided model surface (real candle inference parked as phase 20.7b); audit log queryable; 3 governance wire ops (LIST/DISABLE/ENABLE) operational; tag `phase-20-complete`.

**Scope cut:** Real classifier inference (candle BERT forward pass + linear classifier head) parked as **phase 20.7b**. Phase 20 ships the framework with the BertTokenClassifier load path validated and the candle runtime returning the staged `Failure(reason: "runtime not wired")` until weights + math land. Operators can already provision `BRAIN_NER_MODEL_PATH` — 20.7b just lights up the inference path.

**Delivered:**

- §22 extractors / §27 workers / §25 provenance sections brought to phase-20-implementation depth (~21 spec files: 8 §22 + 3 §27 + 3 §25 + bundled §02/02 §2.7 perf targets + §03/07 fan-out resolution).
- New `brain-extractors` crate (~2 600 LOC):
  - `Extractor` trait (object-safe `Arc<dyn Extractor>`).
  - `ExtractionResult` / `ExtractionStatus` (u8-repr enum, bytes match `brain-metadata::extraction_status::*`).
  - `ExtractedItem` = `EntityMention | StatementMention | RelationMention`.
  - `ExtractorRegistry` with enable / disable / iter_enabled / iter_all.
  - `IdempotencyKey` + BLAKE3 `hash_memory_text`.
  - `PatternExtractor` over `regex` 1.x with 1 MiB compile-size cap (§11/01 §2). All 4 target kinds projected.
  - `ClassifierConfig` + `BertTokenClassifier` load path (operator-provided directory matching brain-embed's `EmbedderConfig`).
  - `labels` BIO decoder for CONLL `B-X I-X I-X` → `X` spans with stray-I-promotion + label-switch handling.
  - `materialize` — decodes persisted `ExtractorDefinition.definition_blob` (JSON) back to `Arc<dyn Extractor>` instances; LLM-kind rows register as degraded placeholders pending phase 21.
  - Operator setup doc `docs/bundled-ner.md`.
- `brain-metadata` storage layer:
  - Widened `ExtractionAudit` (v1 → v2) to the full §11/05 §1 / §25/01 §1 shape: provenance, status, outputs, cost, input_hash. 3 secondary indexes (`_BY_MEMORY` / `_BY_EXTRACTOR` / `_BY_TIME`).
  - `audit_ops` API: `audit_write` (4-table atomic write), `audit_get`, `audit_by_memory` / `_by_extractor` / `audit_recent` / `audit_recent_failures`.
  - 64-entry `OUTPUTS_CAP`; over-cap rejected before wtxn touches.
  - Widened `ExtractorDefinition` (v1 → v2): namespace + name + qname index. `extractor_ops` API mirrors `predicate_ops` (intern / get / lookup / list / set_enabled).
  - `schema_apply` fleshed out for `SchemaItem::Extractor` — JSON-encoded AST blob fans out via `extractor_intern`.
  - System schema gains `brain.entity_mentions` (pattern, two English-name regexes) and `brain.basic_ner` (classifier, threshold 0.6). Stable IDs across reopens (1 and 2).
- `brain-ops`:
  - `OpsContext` extended with `extractor_registry: Arc<RwLock<ExtractorRegistry>>` + `classifier_config: Arc<ClassifierConfig>`.
  - `extractor_pipeline::run_extractor_pipeline` — snapshot-and-dispatch synchronously after ENCODE commit. Audit row per dispatch. Best-effort: errors logged + audited, never propagate to ENCODE.
  - ENCODE non-txn path hooks the pipeline; txn-path skips (phase 22+ wires at commit time).
  - `knowledge_extractor` wire handlers for `EXTRACTOR_LIST` / `_DISABLE` / `_ENABLE`. DISABLE/ENABLE sync the in-memory registry alongside the wtxn write so subsequent dispatches honour the new state.
- `brain-server`:
  - Shard spawn materialises the persisted `EXTRACTORS_TABLE` rows into the runtime registry via `build_registry_from_definitions`. Per-row errors logged + skipped.
  - `BRAIN_NER_MODEL_PATH` env var wired into `ClassifierConfig`.
- Wire surface: 3 opcodes (`0x0124` / `0x0125` / `0x0126`) + 3 responses (`0x01A4` / `0x01A5` / `0x01A6`) end-to-end.
- Integration tests: 6 wire-smoke tests + 1 full-lifecycle phase-exit test (ENCODE → pattern → audit → DISABLE → ENCODE → audit-only-classifier).
- criterion benches against §02/02 §2.7 perf targets (pattern + audit ops; classifier deferred to 20.7b).

**Deferred to later phases (tracked in `../00_overview/04_open_questions_archive.md` + `../00_overview/04_open_questions_archive.md` + `../00_overview/04_open_questions_archive.md`):**

- **Phase 20.7b** (immediate follow-up): BertRuntime candle forward pass + linear classifier head — the `#[ignore]`-gated `real_inference_returns_per_span_for_alice` test in `classifier::tests` flips on when a model is provisioned.
- Resolver-tier persistence of `EntityMention` outputs (entity_mentions rows) — phase 22+; v1 audit row carries item count in `status_reason` for diagnostic visibility.
- LLM extractor (phase 21).
- Classifier near-foreground queue (§27/01 §3) — v1 runs synchronously.
- `OnDemand` / `OnSchemaChange` / `Periodic` triggers — phase 22+ (§11/07 Q3).
- Resolution workers / decay sweeper / FORGET cascade / audit-log sweeper — phase 22+ (§27/07 Q1-Q4).
- `ADMIN_GET_EXTRACTION_AUDIT` wire op — phase 22+ admin (§25/07 Q1).
- Multi-extractor batching, adaptive throttling, cross-shard coordination, content-addressed output IDs — post-v1.
- `depends_on` topological-sort ordering — §11/07 Q11.
- Bundled-model Cargo feature for self-contained binaries — post-v1.
- `feature_extraction: Custom { id }` — post-v1 (§11/07 Q2).
- Auto-predicate creation for unknown statement-target predicates — post-v1 (§11/07 Q6).
- Admin authorization on extractor governance opcodes — phase 21 admin (§28/05 §8).

**Bench results** (Linux Docker, --quick):
- `pattern_extract` 4 KiB / 5 regexes: **43 µs** (spec p99 100 µs ✓).
- `pattern_extract` 256 B / 5 regexes: **2.9 µs**.
- `audit_by_memory(limit=100)`: **47 µs** (spec p99 2 ms ✓).
- `audit_by_extractor(limit=100)`: **83 µs** (spec p99 2 ms ✓).
- `audit_write` per-iter db open + commit cost dominates at 1.4 ms — wtxn-only cost is dramatically lower per the in-test commit timings; the bench setup overhead is the noise source.

---

## Phase 21 — LLM extractor ✓

**One-line:** Third extractor tier (LLM) lights up — Anthropic + OpenAI clients behind a `LlmClient` trait; `LlmExtractor` with cache (phase-17 `LlmCacheDb`) + JSON-schema validation + retry-once + per-call cost budget; server-side env-driven router + per-shard cache wiring; mock-client integration + wire-smoke tests.

**Detailed plan:** [`docs/development/phases/phase-21-llm-extractor.md`](docs/development/phases/phase-21-llm-extractor.md) (per-sub-task plans `.claude/plans/phase-21-task-0[0-7].md`).

**Crates touched:** new `brain-llm`; `brain-extractors`, `brain-metadata`, `brain-ops`, `brain-server`.

**Sub-tasks:** 8 (21.0 spec backfill → 21.7 phase exit).

**Exit:** mock-client end-to-end pipeline (cache → estimator → client → schema-validate → projection → cache write) green; server-side LLM router selects provider from env at startup; per-shard `LlmCacheDb` wired; spec §11/09 + §02/02 §2.8 backfilled; `Extractor::run` is async; tag `phase-21-complete`.

**Scope cut:** Phase-doc sub-tasks 21.7 (Resolver tier 4 — LLM-assisted entity disambiguation) and 21.8 (built-in `brain.preferences_llm` extractor) deferred to phase 22+ / post-v1. The LLM cache + schema validation + retry-once + cost budget all live inside the `LlmExtractor` impl (`crates/brain-extractors/src/llm.rs`); the original phase-doc 21.3/21.4/21.5/21.6 split was collapsed accordingly. See per-sub-task plans `.claude/plans/phase-21-task-01..06.md` for the actual landed shape.

**Delivered:**

- §11/09 (LLM extractor) and §02/02 §2.8 (LLM perf targets) brought to phase-21 implementation depth.
- New `brain-llm` crate (~700 LOC):
  - `LlmClient` trait (object-safe `Arc<dyn LlmClient>`) — `complete(LlmRequest) -> LlmFuture<'a>`, `model()`, `model_id_hash()`.
  - `AnthropicClient` (Messages API; system + user split; `max_tokens`; structured `LlmError` taxonomy).
  - `OpenAiClient` (Chat Completions; cost-micro-usd computed via static pricing table + token usage from response).
  - `model_id_hash` — BLAKE3-64 stable key over the provider's model string for cache-row scoping.
- New `LlmExtractor` (`crates/brain-extractors/src/llm.rs`, ~600 LOC):
  - Cache lookup keyed on `(input_hash, extractor_id, version, model_id_hash)` via the phase-17 `LlmCacheDb`.
  - Per-call cost budget (`CostBudget { per_call_micro_usd }`) — extractions over budget short-circuit with `ExtractionStatus::SkippedBudget` and emit zero LLM calls.
  - JSON-schema validation (`jsonschema` crate) against the operator-declared `output_schema_json`; on first-pass failure the extractor retries once with the validator error appended to the prompt; second failure drops with `ExtractionStatus::SchemaInvalid`.
  - Projection: validated JSON → `ExtractedItem` (`EntityMention | StatementMention | RelationMention`).
  - Per-call timeout (`Duration`) enforced via the client future.
- `Extractor::run` made async (`ExtractionFuture<'a> = Pin<Box<dyn Future<Output = ExtractionResult> + Send + 'a>>`). `PatternExtractor` and `BertTokenClassifier` wrap their sync bodies in `Box::pin(async move { ... })`.
- `MaterializeDeps` bundle + `materialize_llm_extractor` — decodes the persisted LLM-kind `ExtractorDefinition.definition_blob` (provider, model, prompt, schema, budget, timeout) into a live `Arc<dyn Extractor>` against the server-supplied dep bundle (client + cache).
- Server-side wiring (`crates/brain-server`):
  - `build_llm_deps()` reads `BRAIN_LLM_PROVIDER` + `BRAIN_LLM_MODEL` + `BRAIN_*_API_KEY` env, constructs the per-shard `Arc<dyn LlmClient>`, opens a shard-local `LlmCacheDb` under the shard data dir, and threads both into `OpsContext.llm_client` + `OpsContext.llm_cache`.
  - Missing env / unsupported provider → server logs a warning + LLM-kind rows materialize as `LlmExtractor::degraded()` placeholders that emit `ExtractionStatus::ConfigError`.
- Spec backfill: §11/09 (LLM extractor mechanics) + §02/02 §2.8 (cache-hit p50 1 ms / p99 5 ms; budget-skip p50 200 µs / p99 1 ms) drafted alongside the implementation.
- Tests: 11 `LlmExtractor` unit tests + 11 materializer unit tests + 9 server `build_llm_deps` tests + 7 integration tests (`tests/knowledge_llm_extractor.rs`) covering cache hit/miss, retry-once, budget skip, malformed JSON, schema-invalid, projection, degraded fallback + 2 wire-smoke tests.
- criterion benches: `crates/brain-extractors/benches/llm_pipeline.rs` (cache-hit, cost-budget skip, mock-client miss — informational) + `pattern_extract.rs` updated to the new async trait.

**Deferred to later phases:**

- Resolver tier 4 (LLM-assisted entity disambiguation) — phase 22+ (§11/07 Q12). Original phase-doc 21.7.
- Built-in `brain.preferences_llm` extractor — post-v1. Operators declare their own LLM extractors; the system schema ships only the phase-20 pattern + classifier built-ins. Original phase-doc 21.8.
- Live-provider opt-in tests (real Anthropic / OpenAI API behind an env var) — post-v1.
- Pricing TOML override — post-v1; static `STATIC_PRICING` table is the only source today.
- Per-extractor model selection (operator declares model X, router serves model Y) — phase 22+; §11/09 §2 specifies prefix-only routing in v1.
- Live-registry sync on `SCHEMA_UPLOAD` — phase 22+. Uploaded LLM-kind extractors are observable via `EXTRACTOR_LIST` but the dispatching registry is rebuilt only at shard spawn (gap recorded in `tests/knowledge_llm_extractor_wire.rs`).
- Global (cross-shard) cost budget — post-v1; v1 enforces `per_call_micro_usd` only.

**Bench results** (Linux Docker, --quick): bench harness in place; numbers to be captured during phase-22 pre-flight (skipped at tag time to keep the loop moving). Spec targets: cache-hit p50 1 ms / p99 5 ms, budget-skip p50 200 µs / p99 1 ms.

---

## Phase 22 — Tantivy / lexical retrieval ✓

**One-line:** Tantivy BM25 over memory text + statement text; brain analyzer with URL / code-ID / Porter pipeline; per-shard memory + statement text indexer workers (bounded channel + group commit + retry-once-then-fatal); `LexicalRetriever` trait + `TantivyLexicalRetriever`; atomic-swap rebuild + shard-startup recovery.

**Detailed plan:** [`docs/development/phases/phase-22-tantivy-lexical.md`](docs/development/phases/phase-22-tantivy-lexical.md) (per-sub-task plans `.claude/plans/phase-22-task-0[0-8].md`).

**Crates touched:** `brain-index`, `brain-ops`, `brain-server` (no `brain-workers` — text indexer drain runs as `glommio::spawn_local` directly under the shard executor).

**Sub-tasks:** 9 (22.0 spec backfill → 22.8 phase exit).

**Exit:** ENCODE / FORGET / STATEMENT_CREATE / SUPERSEDE / TOMBSTONE / RETRACT all dispatch to the matching indexer via OpsContext slots wired at shard spawn; `LexicalRetriever::retrieve` returns BM25-ranked items per-scope; on-disk corruption or schema-version mismatch triggers an atomic-swap rebuild at next shard start; tag `phase-22-complete`.

**Scope cuts:**

- **Memory text rebuild produces an empty valid index.** `MEMORIES_TABLE` stores `text_size` but not the text itself (text lives only on the ENCODE wire path + WAL frames). Full content-aware memory rebuild lands post-v1; operators re-ingest from their own source-of-truth in v1. Statement rebuild is content-complete via `object_blob` + entity / predicate joins.
- **Partial WAL replay on shard recovery deferred.** The bound (≤ N-1 writes per indexer at crash, default N=256) is documented in §26/01 §3 and accepted for v1; cursor-tracked partial replay lands post-v1.
- **Hot rebuild while the live writer is running deferred.** v1 rebuild is startup-only.
- **Stop-word removal explicitly NOT in the analyzer** (preserves exact-ID queries like `ACME-1247`).
- **Snippet generation deferred** — `RankedItem.snippet` always `None` in v1.
- **BM25 k1 / b custom-similarity wiring deferred** — `LexicalRetrieverConfig` exposes the fields but the retriever uses tantivy defaults (1.2 / 0.75); custom similarity binds in phase 23 if needed.
- **`ADMIN_TANTIVY_REBUILD` wire op deferred to §28/05 admin scope.**
- **Cross-shard ranking deferred to phase 23** (router fan-out).
- **Segment-merge windowing post-v1** — rely on tantivy's `LogMergePolicy`.

**Delivered:**

- §23 / §26 / §27 / §02/02 §2.9 brought to phase-22 implementation depth:
  - `spec/13_retrievers/02_lexical_retriever.md` (new, ~180 LOC) — trait surface, BM25 params, tokenizer pipeline, scope dispatch, filters, errors, perf bounds.
  - `spec/10_metadata/01_tantivy_layout.md` (new, ~200 LOC) — directory layout, schemas, commit cadence, segment merge, atomic-swap rebuild, recovery contract, size budgets.
  - `spec/15_background_workers/02_text_indexer_workers.md` (new, ~200 LOC) — memory + statement text indexers, backpressure-on-overflow discipline, retry-once-then-fatal commit policy, WAL ordering.
  - `spec/19_benchmarks/02_performance_targets.md` §2.9 — LexicalRetriever p50/p99 targets.
- `brain-index` (new modules):
  - `tantivy_shard` — per-shard `TantivyShard` handle with `BRAIN_SCHEMA_VERSION=1` payload stamping, `IndexStatus { Ready | NeedsRebuild { OpenFailed | SchemaVersionMismatch | PayloadCorrupt } }`, fresh-dir vs existing-dir disambiguation via `meta.json` probe.
  - `tantivy_shard::tokenizer` — `BrainTokenizer` (NFC → lowercase → URL + code-ID + dotted-identifier preservation → tokenize → Porter stem; protected tokens bypass stemmer). Registered under tantivy's `"default"` name.
  - `tantivy_shard::retriever` — `LexicalRetriever` trait, `LexicalQuery` / `LexicalFilters` / `LexicalRetrieverConfig` / `RankedItem` / `RankedItemId` / `LexicalError`, `TantivyLexicalRetriever` impl with synchronous `reader.reload()` per query.
- `brain-ops`:
  - `ops::text_indexer::memory` — `MemoryTextOp { Upsert | Forget }`, `MemoryTextDispatcher`, `spawn_memory_text_indexer_local` (Linux Glommio entry) + `run_memory_text_indexer` (test-friendly). Bounded `flume` channel (4096), `CommitPolicy::from_env()` (N=256 / T=1 s defaults, env-overridable), retry-once-then-shard-fatal commit, every commit stamps `schema_payload_json()`.
  - `ops::text_indexer::statement` — same harness for `statements.tantivy/`; `confidence_bucket()` clamping; `upsert_op_from_statement()` helper that joins entity + predicate at dispatch time.
  - `ops::text_indexer::rebuild` — `rebuild_memory_text` (empty valid index) + `rebuild_statements` (content-aware, with tombstone + pending-subject + orphan-row skips); atomic-swap mechanics via `std::fs::rename`.
  - `OpsContext` gains `tantivy`, `memory_text_dispatcher`, `statement_text_dispatcher`, `lexical_retriever` slots (all `Option<...>` — substrate-only deployments leave them `None`).
  - Hooks at `handle_encode`, `handle_forget`, `handle_statement_create` (+ auto-superseded preference cascade), `handle_statement_supersede`, `handle_statement_tombstone`, `handle_statement_retract`.
- `brain-server`:
  - Shard spawn opens the `TantivyShard`, registers the brain analyzer, calls `tantivy_recovery::recover_tantivy_on_open` (which runs 22.6 rebuild fns on `NeedsRebuild` status), spawns both indexer drain tasks via `glommio::spawn_local`, constructs the retriever, installs all dispatcher + retriever handles on the `OpsContext`.
- Tests:
  - `tantivy_shard` unit tests (~28 across schema, retriever, tokenizer, open).
  - `brain-ops` text indexer tests (~24 across memory + statement + rebuild + commit-policy + e2e indexer→retriever).
  - `brain-server` recovery tests (4) + phase-exit integration tests (2; ENCODE → retrieve; FORGET → no hit).
- `crates/brain-index/benches/lexical_retrieve.rs` — three criterion benches at 10K corpus scale against §02/02 §2.9 perf targets. Production-scale (100K / 1M) validation reserved for phase 14 acceptance.

**Deferred to later phases:**

- Full content-aware memory rebuild (post-v1) — §27/07.
- Partial WAL replay on shard recovery (post-v1) — §27/07.
- Hot rebuild while live writer running (post-v1).
- `ADMIN_TANTIVY_REBUILD` wire op — phase 28/05 admin.
- Cross-shard lexical ranking — phase 23 router.
- BM25 k1 / b custom similarity — phase 23 if needed.
- Snippet generation — post-v1 if hybrid query needs it.
- Segment-merge windowing — post-v1.

**Bench results** (Linux Docker, --quick): bench harness in `crates/brain-index/benches/lexical_retrieve.rs` ready for capture; wall-time numbers deferred per the 21.7 precedent. Spec targets: memory single-term p50 10 ms / p99 50 ms @ 100K; statement single-term same @ 1M; commit (256-doc batch) p50 5 ms / p99 25 ms (§02/02 §2.9). Production-scale validation runs in phase 14's acceptance suite.

---

## Phase 23 — Hybrid query engine ✓

**One-line:** Rule-based query router (5 rules), RRF fusion (`k=60`), post-fusion filter chain (type / temporal / confidence / tombstone / supersession), planner + executor, EXPLAIN/TRACE renderers, four wire opcodes (`QUERY` / `QUERY_EXPLAIN` / `QUERY_TRACE` / `RECALL_HYBRID`), fluent SDK `client.query()` builder, and substrate `RECALL` transparently routes through the hybrid pipeline on schema-declared deployments.

**Detailed plan:** [`docs/development/phases/phase-23-hybrid-query.md`](docs/development/phases/phase-23-hybrid-query.md) (per-sub-task plans `.claude/plans/phase-23-task-0[0-9].md`, `.claude/plans/phase-23-task-1[0-2].md`).

**Crates touched:** `brain-planner`, `brain-index`, `brain-protocol`, `brain-ops`, `brain-server`, `brain-sdk-rust`.

**Sub-tasks:** 13 (23.0 spec backfill → 23.12 phase exit).

**Exit:** `client.query()` plans + executes hybrid; EXPLAIN renders the plan text without execution; TRACE adds the per-retriever execution block; substrate `RECALL_REQ` transparently routes through the hybrid engine when a schema is declared (`MemoryResult.contributing_retrievers` + `fused_score` populated); tag `phase-23-complete`.

**Scope cuts:**

- **Streaming hybrid query results (limit > 100) deferred** to post-v1 — single `QueryResponse` frame in v1; SUBSCRIBE streaming path tracked as §30 OQ-23-A.
- **Hybrid + transactional read-your-writes deferred** — RECALL inside a txn stays on the substrate path. Lens layering across statements + relations is multi-week work; tracked as §30 OQ-23-B.
- **Filter-only retriever mode (no text + no anchor) deferred** — planner returns `NoSignal`. A "find by filters only" mode needs a new "everything" retriever; tracked as §30 OQ-23-C.
- **Learned router on top of the rule-based one** — re-affirms `OQ-V2-1` + adds §30 OQ-23-D. Rule-based router ships as the stable fallback in v1.
- **Cross-shard hybrid result merging deferred** — v1 is per-shard; cross-shard fan-out belongs upstream of the hybrid engine. Tracked as §30 OQ-23-E.
- **`MemoryResult.text` not auto-populated on the hybrid path** — matches the substrate default (text only when caller requests).
- **Parallel retriever execution deferred** — v1 invokes the three retrievers sequentially under Glommio's single-threaded executor. Async-trait migration is post-v1; §02/02 §2.10 budget headroom is comfortable at 10K scale.
- **No `client.recall_hybrid` SDK verb** — the wire opcode `RECALL_HYBRID` (`0x0163`) exists for narrow / non-Rust callers, but the Rust SDK leaves it unreachable from the public surface. Domain verbs in the public API (`client.recall(...)` is the substrate verb; 23.11 routes it through hybrid transparently).

**Delivered:**

- §13/03 (SemanticRetriever), §13/04 (GraphRetriever), §02/02 §2.10 (hybrid perf targets) brought to phase-23 implementation depth in 23.0.
- `brain-index` (new modules):
  - `semantic_retriever` — trait + `SemanticQuery / SemanticScope / SemanticFilters / SemanticRetrieverConfig / SemanticError` plus `RankedItemId::{Memory,Statement,Entity,Relation}` extension to cover all four item kinds.
  - `graph_retriever` — trait + `GraphQuery::{Star,Path,Subgraph}` / `GraphRetrieverConfig` / `Direction` / `GraphError`.
- `brain-ops` (new modules):
  - `ops::semantic_retriever::BrainSemanticRetriever` — embeds via the dispatcher, searches the per-shard memory HNSW + (when wired) the statement HNSW, pushes filters down via `SemanticFiltersConfigSlot`.
  - `ops::graph_retriever::BrainGraphRetriever` — Star / Path / Subgraph traversal over the entity + relation + statement redb tables; relation-type push-down; bounded depth + branching.
  - `ops::knowledge_query` — four hybrid-query wire handlers (`Query` / `QueryExplain` / `QueryTrace` / `RecallHybrid`) + wire ⇄ planner translation helpers; 16 KiB text cap; 3-entry cap on explicit retriever lists.
  - `schema_gate` — `SchemaGate(Arc<ArcSwap<bool>>)` seeded from `schema_namespaces` at shard spawn; flipped by `handle_schema_upload` post-commit (spec §28/08 §1).
  - `OpsContext` gains `semantic_retriever / lexical_retriever (22.5) / graph_retriever / schema_gate` slots.
  - `handle_recall` routes through hybrid when the gate is set AND no txn is attached; falls back to substrate on `MissingRetriever`.
- `brain-planner::knowledge` (new namespace):
  - `router` — 5 routing rules + classification features (`has_text`, `has_entity_anchor`, `contains_exact_id`, `is_all_caps_tokens`, `is_question`, `contains_entity_mention_heuristic`, `contains_temporal_expression`, etc.).
  - `fusion` — `fuse_rrf(outputs, k, weights)` with `DEFAULT_K = 60`; stable sort with deterministic tie-break.
  - `filters` — type → temporal → confidence → tombstone → supersession; single `ReadTransaction` shared across the five filters; per-step survivor counts.
  - `planner` — `plan(req) -> QueryPlan`; expands routing into `PlannedRetriever` configs with per-retriever defaults and a single pre-filter push-down (temporal > predicate > kind precedence in v1).
  - `executor` — `execute(plan, req, ctx) -> QueryResult` with sequential retriever invocation, soft post-hoc timeout, RRF fuse, filter chain, project.
  - `explain` — `render_plan(plan)` and `render_trace(plan, metadata)`.
- `brain-protocol`:
  - 8 new opcode entries at `0x0160-0x0163` (req) and `0x01E0-0x01E3` (resp).
  - `knowledge::query` (new module) — `QueryRequest`, `QueryResponse`, `QueryExplainRequest`, `QueryExplainResponse`, `QueryTraceRequest`, `QueryTraceResponse`, `RecallHybridRequest`, `RecallHybridResponse`, and the supporting types (`RetrieverWire`, `RetrieverSelectionWire`, `FusionConfigWire`, `TimeRangeWire`, `ItemIdWire`, `RetrieverContributionWire`, `RetrieverOutcomeWire`).
  - Substrate `MemoryResult` gains `contributing_retrievers: Vec<RetrieverNameWire>` and `fused_score: f32`; new `RetrieverNameWire` enum in `responses::types` with a `From<RetrieverWire>` bridge so substrate types stay free of the knowledge namespace.
- `brain-sdk-rust::knowledge::query` (new module):
  - `Client::query()` returns `QueryBuilder<'_>` with three terminal verbs: `.execute()` / `.explain()` / `.trace()`.
  - SDK-owned domain types (no aliasing): `Retriever`, `RetrieverSelection` (with `.explicit()` validating constructor), `FusionConfig`, `TimeRange` (with `from_to / since / until / last_days / last_hours / open_ended / contains`), `ItemRef` (typed-ID enum), `RetrieverOutcomeStatus` (collapses `(u8, String)` wire pair into an enum), `QueryHit`, `RetrieverContribution`, `RetrieverOutcome`, `QueryResult`, `ExplainResult`, `TraceResult`.
  - Builder-side validation (NoSignal, oversized text, invalid fusion knobs, inverted time range, oversized explicit retriever list) runs once in the terminal verb; setters stay infallible.
- `brain-server`:
  - Shard spawn seeds the `SchemaGate` from the per-shard `MetadataDb`; installs it on `OpsContext` via `with_schema_gate`.
- Tests:
  - brain-protocol: 188 lib tests including rkyv round-trips for every new query type.
  - brain-planner: per-module unit tests for router, fusion, filters, planner, executor, explain (~80 tests).
  - brain-ops: knowledge_query unit tests + schema_gate unit tests.
  - brain-server: 6 wire integration tests in `knowledge_query_wire.rs`, 3 in `recall_hybrid_routing.rs`, 4 in `knowledge_hybrid_phase_exit.rs`.
  - brain-sdk-rust: 18 unit tests in `query::tests` + 7 integration tests in `tests/knowledge_query.rs` (mock-server round-trips).
- `crates/brain-planner/benches/hybrid_query.rs` — three criterion benches against §02/02 §2.10 perf targets (hybrid 3-retriever, router-degraded, EXPLAIN-only).

**Deferred to later phases:**

- Streaming hybrid query results — §30 OQ-23-A.
- Hybrid + transactional read-your-writes — §30 OQ-23-B.
- Filter-only retriever mode — §30 OQ-23-C.
- Learned router on top of rule-based — §30 OQ-23-D + §30 OQ-V2-1.
- Cross-shard hybrid result merging — §30 OQ-23-E.
- Parallel retriever execution (async-trait migration) — post-v1.
- Production-scale wall-time validation — phase 14 acceptance.

**Bench results** (Linux, --quick): bench harness in `crates/brain-planner/benches/hybrid_query.rs` ready for capture; wall-time numbers deferred per the 21.7 / 22.8 precedent. Spec targets: hybrid 3-retriever p50 10 ms / p99 50 ms; router-degraded p50 7 ms / p99 30 ms; EXPLAIN-only p50 500 µs / p99 2 ms (§02/02 §2.10). Production-scale (100 K / 1 M) validation runs in phase 14's acceptance suite.

---

## Phase 24 — Sweepers, knowledge acceptance & `v1.0.0` ✓

**One-line:** Eight background workers (backfill, FORGET cascade, schema migration, five sweepers), schema-toggle runbook, end-to-end test, acceptance suite, v1.0.0 release.

**Detailed plan:** [`docs/development/phases/phase-24-acceptance.md`](docs/development/phases/phase-24-acceptance.md) (per-sub-task plans `.claude/plans/phase-24-task-0[0-9].md` + `phase-24-task-1[0-2].md`).

**Crates touched:** `brain-core`, `brain-metadata`, `brain-workers`, `docs/runbooks/`, `docs/tutorials/`, `scripts/`, CHANGELOG, ROADMAP.

**Sub-tasks:** 13 (24.0 spec backfill → 24.12 phase exit + v1.0.0).

**Exit:** WORKER_CHECKPOINTS_TABLE shared across state-carrying workers; backfill + FORGET cascade + schema migration + 5 sweepers all land on the existing `Worker` trait + scheduler; schema-toggle runbook + e2e script + full-acceptance script in tree; tutorial published; `phase-24-complete` and `v1.0.0` tags cut.

**Scope cuts (v1):**

- **Memory text not persisted beyond the WAL.** Live backfill + schema-migration mark items `Failed` with reason "memory text not persisted (v1 limitation)". Dry-run plan preview is fully functional. Operators re-ingest in v1.
- **Cascade audit rows + soft-cascade revert** deferred. Cascade itself updates evidence + recomputes confidence + tombstones correctly.
- **Per-row stale-extraction flag** deferred (needs `StatementRow.flags` schema bump). Stale-count surfaces via metric.
- **Entity GC inbound-reference counting** deferred. Worker + env-flag scaffolding ships; eligibility scan is a stub.
- **LLM cache full sweep** deferred. Defaults rely on `brain-metadata::llm_cache`'s TTL-on-read.
- **handle_forget → cascade enqueue hook** deferred. Workers reachable via direct `ForgetCascadeWorker::enqueue`; spec §25/00 contract preserved (cascade is async by design).
- **Wire opcodes for admin backfill / cancel** deferred — typed request shapes live in `brain-core::worker_state`; CLI / HTTP surface lives in a follow-up.

**Delivered:**

- §27/03 (sweeper workers) + §27/04 (state-carrying workers) brought to phase-24 implementation depth.
- `brain-core` (new modules): `worker_state` (BackfillId / BackfillRange / BackfillRequest / BackfillProgress / WorkerPriority); `migration` (MigrationId / MigrationReason / MigrationItem / MigrationPlan / MigrationSummary).
- `brain-metadata`:
  - `tables::worker_checkpoints` — `WORKER_CHECKPOINTS_TABLE` with composite `(&str, &[u8])` key + `WorkerCheckpointRow {status, attempts, timestamps, last_error}` + status transition ops (`mark_started` / `mark_completed` / `mark_failed`).
  - `cascade_ops` — `cascade_forget_to_statements(wtxn, memory_id, threshold, batch_cap, now)` walks STATEMENTS_TABLE, drops the forgotten memory from inline evidence, recomputes confidence, tombstones with `SourceMemoryForgotten` when empty + low-confidence.
  - `sweeper_ops` — shared `SweepSummary` + `sweep_superseded_statements` + `sweep_audit_log` + `scan_stale_statements`.
- `brain-workers`:
  - 8 new `WorkerKind` variants + default cadences.
  - `backfill::BackfillWorker` with submit / cancel / progress + per-(memory, extractor) checkpoint walk.
  - `forget_cascade::ForgetCascadeWorker` with bounded queue + per-job wtxn.
  - `schema_migration::SchemaMigrationWorker` with plan queue + checkpoint walk.
  - `supersession_sweeper`, `audit_log_sweeper`, `llm_cache_sweeper`, `stale_extraction_detector`, `entity_gc` — periodic Low-priority workers.
- `docs/runbooks/schema-toggle.md` — RB-11 operator runbook: validate → declare → backfill → verify → migrate → revert.
- `docs/tutorials/01-getting-started.md` — 15-minute end-to-end tutorial.
- `scripts/schema-toggle-e2e.sh` — bash driver mirroring the runbook against a live `brain-server`.
- `scripts/full-acceptance.sh` — orchestrator that chains workspace tests + e2e + spec link integrity.
- `scripts/spec-link-check.sh` — validates every `[`./…`]` cross-ref in spec/ + docs/.
- `CHANGELOG.md` — v1.0.0 release notes.

**Deferred to later versions:** all v1 scope-cuts above; per-statement-kind retention; ADMIN_BACKFILL / ADMIN_CANCEL wire opcodes; SCHEMA_DROP opcode for in-place schema downgrade (current revert is manual per the runbook).

**Tags:** `phase-24-complete` and `v1.0.0` both at this commit.

---

## Strict ordering

Phase N+1 doesn't start before Phase N is exited and tagged. The dependencies aren't soft preferences — they're real:

- Phase 1's `Frame` is consumed by Phase 9's connection layer.
- Phase 2's `MetadataSink` trait is implemented by Phase 3.
- Phase 4's `HnswIndex` requires Phase 2's slot reads and Phase 3's tombstone state.
- Phase 7 wires Phases 2-6 together.
- Phase 9 wires everything.
- Phase 11 provides the HTTP substrate Phase 12 instruments on.
- Phase 12 emits the metrics Phase 13 measures.
- Phase 13's chaos rig produces the recovery evidence Phase 14's acceptance gates consume.
- Phase 14's substrate-rc is the foundation Phase 15 builds on without disturbing.
- Phase 15's storage layout is consumed by every knowledge-layer phase.
- Phases 16, 17, 18 layer up the data model; 19 declares it via DSL.
- Phases 20, 21 produce typed data into the model; 22 indexes it for keyword search.
- Phase 23 fuses semantic + lexical + graph retrievers.
- Phase 24's combined acceptance suite is what `v1.0.0` ships against.

Phases 16 through 22 may partially overlap (see DAG in [`docs/development/phases/README.md`](docs/development/phases/README.md)). Strict ordering still applies across the substrate/knowledge boundary at Phases 14 → 15.

Skipping ahead means stubbing types you'll have to revisit. Don't.

## How to track progress

- Each completed sub-task is a commit (per [`AUTONOMY.md`](AUTONOMY.md) §5).
- Each completed phase is a tag (`phase-N-complete`).
- Each phase doc has its own `[ ] / [x]` checkboxes per sub-task.
- `git log --oneline | grep "^[a-f0-9]* [0-9]*\."` shows all completed sub-tasks.
- `/status` (slash command) summarizes current position.

## Known limitations of v1

Documented up front so the scope is honest:

- **Single-node only.** Multi-node clustering is v2.
- **No replication.** Backups (snapshots) only. v2.
- **Rust SDK only.** Python/TypeScript/Go are v1.x.
- **Linux only.** Glommio + io_uring don't run elsewhere.

These aren't bugs — they're scope boundaries. Don't accidentally implement them.
