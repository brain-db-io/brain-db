# Spec Restructure — Brain as a Modern Memory Database for AI Agents

**Status:** Proposed. Deleted after execution completes.

## Why

Brain is **a memory database for AI agents**. Not a "cognitive substrate." Not a "knowledge layer on top of a substrate." One database that stores typed memories (facts, preferences, events, entities, relations) with explicit provenance, hybrid retrieval (semantic + lexical + graph + temporal), and bi-temporal validity. The spec needs to read that way.

The current spec carries three structural problems:

1. **Defunct framing leftovers.** §03 wire_protocol has 8 `knowledge_*`-named duplicate files (knowledge_design_choices, knowledge_payload_encoding, knowledge_errors, knowledge_validation, knowledge_open_questions, knowledge_references, knowledge_README, knowledge_opcodes_overview) — each a parallel-named sibling of the substrate counterpart. §11 has 3 similar duplicates. §14 still says "the knowledge layer organizes data in three layers" — that vocabulary is dead per CLAUDE.md §2.

2. **Ornamental naming.** "hybrid query router" tells you nothing — every modern retrieval system uses multiple retrievers. "ANN index" is engineering jargon. "Cognitive operations" / "cognitive substrate" sound abstract; "write pipeline" / "read pipeline" describe what the system actually does. arc-labs gets this right ([recall/docs/spec](file:///Users/dodo/Desktop/arc-labs/docs/spec)) — every chapter is one-or-two functional words.

3. **Reading whiplash.** Current order: subsystem internals (§04-§08) → cognitive ops (§09) → cross-cutting (§10-§12) → SDK (§13) → ops (§10-§14) → knowledge concepts (§14-§24). A new reader hits HNSW parameters before being told what Brain stores. An SDK implementer reads `01 → 02 → 03 → 09 → 13`, skipping ⅔ of the spec.

The fix: **rethink as one database for agentic AI**. Sections grouped by what a reader needs in order; names that say what the thing does; the typed-graph framing baked into the data model itself.

## Target structure — hybrid (chapter + deep-dive)

20 chapter files at the top level (one per concept). Each chapter has a matching subdir holding the dense reference material. New readers read the 20 chapters end-to-end. Implementers reading one concern dive into the subdir.

```
spec/
├── 00-overview.md                ← chapter
│   00-overview/
│       glossary.md
│       doc-map.md
│       versioning.md
│       open-questions-archive.md
├── 01-architecture.md            ← chapter
│   01-architecture/
│       problem.md
│       background.md
│       primitives.md
│       layers.md
│       hardware.md
│       targets.md
│       non-goals.md
│       comparison.md             ← Brain vs. Mem0 / Zep / Letta / Pinecone / Weaviate
│       design-wedges.md          ← NEW: competitive differentiators
│       invariants.md             ← the 7 engineering invariants
├── 02-data-model.md
│   02-data-model/
│       memory.md
│       entity.md                 ← (folded from old §14)
│       statement.md              ← (folded from old §14)
│       relation.md               ← (folded from old §14)
│       three-statement-kinds.md  ← (folded from old §14)
│       composition.md            ← (folded from old §14)
│       identifiers.md
│       edges.md
│       context.md
│       salience.md
│       memory-kinds.md
│       lifecycle.md
├── 03-schema.md                  ← renamed from "schema_dsl"
│   03-schema/
│       grammar.md, ast.md, validator.md, namespaces.md,
│       versioning.md, system-schema.md
├── 04-wire-protocol.md
│   04-wire-protocol/
│       design-choices.md         ← ONE file (8 dupes merged)
│       transport.md, frame-header.md, payload-encoding.md, opcodes.md,
│       handshake.md, request-frames.md, response-frames.md, streaming.md,
│       errors.md, validation.md, versioning.md, subscribe-events.md,
│       schema-optional-mode.md,
│       entity-frames.md, schema-frames.md, statement-frames.md,
│       relation-frames.md, query-frames.md, admin-frames.md
├── 05-write-pipeline.md          ← renamed from "cognitive ops" write side
│   05-write-pipeline/             (ENCODE + LINK + FORGET + TXN + extraction)
│       encode.md, link.md, forget.md, txn.md, materialize-procedural.md,
│       extraction-trigger.md
├── 06-read-pipeline.md           ← renamed from "cognitive ops" read side
│   06-read-pipeline/              (RECALL + PLAN + REASON + SUBSCRIBE + query)
│       recall.md, plan.md, reason.md, subscribe.md, query.md
├── 07-sdk.md
│   07-sdk/
│       core-api.md, connection.md, retries.md, streams.md,
│       idiomatic-languages.md, observability.md, testing.md,
│       typed-graph-builders.md, query-builder.md
├── 08-embedding.md               ← renamed from "embedding_layer"
│   08-embedding/
│       model-choice.md, tokenization.md, inference.md, normalization.md,
│       caching.md, batching-gpu.md, fingerprinting.md, migration.md
├── 09-storage.md                 ← renamed from "storage_arena_wal"
│   09-storage/
│       arena-layout.md, arena-growth.md, wal-overview.md, wal-records.md,
│       wal-durability.md, write-path.md, recovery.md, checkpointing.md,
│       snapshots.md
├── 10-indexing.md                ← renamed from "ann_index"
│   10-indexing/
│       hnsw-primer.md, parameters.md, insertion.md, search.md, deletion.md,
│       persistence.md, maintenance.md, concurrency.md, filtering.md
├── 11-metadata.md                ← renamed from "metadata_graph"
│   11-metadata/
│       redb-choice.md, table-layout.md, memory-table.md, edge-storage.md,
│       context-table.md, idempotency.md, text-storage.md, transactions.md,
│       concurrency.md, tantivy-layout.md, knowledge-tables.md, llm-cache.md
├── 12-extractors.md
│   12-extractors/
│       pattern.md, classifier.md, llm.md, triggers.md, resolver.md,
│       audit.md, idempotency.md, prompt-caching.md, plugins.md
├── 13-query-optimizer.md         ← renamed from "query_planner"
│   13-query-optimizer/
│       cost-model.md, plan-cache.md, ann-executor.md, attractor-executor.md,
│       graph-executor.md, vsa-algebra.md
├── 14-retrievers.md              ← drops "hybrid query" framing
│   14-retrievers/
│       rrf-fusion.md, lexical.md, semantic.md, graph.md,
│       query-router.md, per-hit-enrichment.md, rerank.md
├── 15-provenance.md
│   15-provenance/
│       audit-tables.md, derivation-chains.md, re-extraction.md, retention.md
├── 16-concurrency.md
│   16-concurrency/
│       (current §10 contents)
├── 17-background-workers.md
│   17-background-workers/
│       (unified worker set — current §11 with 3 dupes merged)
├── 18-sharding.md                ← renamed from "sharding_clustering"
│   18-sharding/
│       (current §12 contents)
├── 19-observability.md           ← renamed from "observability_ops"
│   19-observability/
│       metrics.md, logs.md, tracing.md, dashboards.md, alerts.md,
│       admin-ops.md, runbooks.md, capacity-planning.md,
│       security-scope.md
├── 20-failure-recovery.md
│   20-failure-recovery/
│       (current §14 contents)
└── 21-benchmarks.md              ← renamed from "benchmarks_acceptance"
    21-benchmarks/
        latency-targets.md, throughput-targets.md, resource-targets.md,
        recall-quality.md, durability-criteria.md, benchmark-methodology.md,
        acceptance-test-suite.md, complete-acceptance.md
```

**Net: 22 chapter files + ~200 deep-dive files** (vs. current 310 files in 25 nested dirs).

## Naming sweep

Drop ornamental adjectives. Use names that say what the thing does.

| Current | New | Reason |
|---|---|---|
| `hybrid query router` | **query router** | "hybrid" = "uses multiple retrievers"; every modern retrieval system does this — adjective adds no signal |
| `hybrid query` | **query** | same |
| `hybrid query engine` | **query engine** | same |
| `cognitive substrate` | **memory database for AI agents** | concrete + searchable + matches positioning |
| `cognitive operations` | **operations** or split into **write pipeline** + **read pipeline** | "cognitive" adds nothing; pipelines are concrete |
| `knowledge layer` | (delete the phrase entirely) | there is no separate layer |
| `substrate` (the noun, when describing Brain itself) | (delete) | Brain *is* the DB |
| `substrate-only mode` | **schemaless mode** | describes what's enabled, not what's absent |
| `three-layer model` | (delete) | one model with four record types |
| `typed memory layer` | **typed memory** | drop "layer" |
| `ANN index` | **vector index** or **indexing** | "ANN" is engineering jargon |
| `HNSW maintenance` | **index maintenance** | "HNSW" is implementation |
| `RRF fusion` | **rank fusion** | RRF is the algorithm name; "rank fusion" is the role |
| `idempotency cleanup` | **idempotency sweep** | "cleanup" is vague |
| `slot reclamation` | **arena GC** or **slot reclaim** | reclaim is the action |
| `cross-encoder rerank` | **reranker** | the noun, not the tier-tag |
| `knowledge tables` | **typed-graph tables** (or just **graph tables**) | describes shape, not "extra layer" |
| `knowledge workers` | (use the worker names) | extractor, sweeper, indexer, etc. |
| `knowledge-layer storage` | (use the artifact names) | tantivy, llm cache, redb tables |

Sweep every spec file. Sweep CLAUDE.md, ROADMAP.md, AUTONOMY.md. Leave external library names alone (HNSW is fine when discussing the algorithm; "the HNSW parameters" stays; "the HNSW maintenance worker" becomes "the index maintenance worker").

## Reframe — concrete positioning

Brain's current opening (in `00_master_overview/README.md`):
> Brain — A Cognitive Substrate for AI Agents

Replace with:
> **Brain — a memory database for AI agents.**
>
> Agents need a place to put what they learn. Brain stores typed memories — facts, preferences, events, entities, and relations — with explicit provenance, confidence, and bi-temporal validity. Retrieval is hybrid (semantic + lexical + entity-graph + temporal) fused with weighted rank fusion. One Rust core, one wire protocol, one schema. Apache 2.0.

Open `01-architecture.md` with concrete problems agentic-memory systems have today, with numbers from public sources (arc-labs's pattern):

- **Extraction produces junk.** Mem0's production audit showed 97.8% of stored memories were junk (hash duplicates, hallucinated profiles, feedback-loop contamination). Brain's three-tier pipeline (pattern → GLiNER classifier → LLM with prompt cache) rejects ~80-90% of candidates before storage.
- **Flat text loses signal.** LongMemEval (ICLR 2025) shows existing systems hit 5-65% on temporal reasoning. Brain stores typed records with bi-temporal validity (`valid_from / valid_to / extracted_at / record_invalidated_at`).
- **Context-window stuffing wastes tokens.** At GPT-4o's $2.50/M input tokens and 10k tokens of prior context per turn × 100k calls/day = ~$912k/year of wasted tokens. Brain's hybrid retrieval returns top-k relevant memories, not the full history.
- **Existing systems lock you in.** Zep needs Neo4j. Pinecone serverless charges per-segment. Mem0 gates graph features at $249/mo. Brain ships embedded / self-hosted / cluster from one codebase, Apache 2.0.

## Design Wedges

Add `01-architecture/design-wedges.md` stating Brain's five competitive differentiators (a higher-level companion to the seven invariants, which remain as engineering rules):

1. **Typed graph + bi-temporal** — every memory is a typed record with four timestamps. Not flat vectors. Time-travel queries work.
2. **Owned embedding model** — Brain runs BGE-small itself; clients send text. No external API on the write path. Latency budget under operator control.
3. **WAL-first durability** — single-writer-per-shard, fsync-before-ack, CRC everywhere. No "eventually consistent" cliff edges.
4. **One codebase across deployment modes** — embedded, self-hosted, cluster. Same crates, same wire, same schema. No "cloud edition" with extra features.
5. **Apache 2.0, no SaaS lock-in** — including the typed graph, extractor pipeline, and reranker.

## What dissolves (file-by-file)

### Sections that disappear

- **§14 knowledge_model** (5 files) — content into `02-data-model.md` chapter + `02-data-model/three-statement-kinds.md` + `02-data-model/composition.md`. Drop "three-layer model" prose.
- **§14 entities** (8 files) — record-type content into `02-data-model/entity.md`; resolution logic into `12-extractors/resolver.md`; merge review queue into `12-extractors/audit.md`.
- **§14 statements** (8 files) — record-type content into `02-data-model/statement.md`; supersession ladder into `14-retrievers/` or `02-data-model/supersession.md`; bi-temporal into `02-data-model/statement.md`.
- **§14 relations** (8 files) — into `02-data-model/relation.md`.

### Dedups inside §03 (8 → 0)

| Substrate file (kept) | Knowledge-suffix dupe (folded in) |
|---|---|
| `01_design_choices.md` | `26_knowledge_design_choices.md` |
| `04_payload_encoding.md` | `27_knowledge_payload_encoding.md` |
| `05_opcodes.md` | `15_knowledge_opcodes_overview.md` |
| `10_errors.md` | `18_knowledge_errors.md` |
| `11_validation.md` | `19_knowledge_validation.md` |
| `../00_overview/04_open_questions_archive.md` | `../00_overview/04_open_questions_archive.md` |
| `../00_overview/05_external_references.md` | `../00_overview/05_external_references.md` |
| `README.md` | `30_knowledge_README.md` |

### Dedups inside §11 (3 → 0)

| Substrate file (kept) | Knowledge-suffix dupe (folded in) |
|---|---|
| `00_purpose.md` | `12_knowledge_workers_overview.md` |
| `../00_overview/04_open_questions_archive.md` | `../00_overview/04_open_questions_archive.md` |
| `../00_overview/05_external_references.md` | `../00_overview/05_external_references.md` |

### §02/14 "knowledge_layer_storage" title → drop

Content already lives in §07 sibling files. Title "Storage — Knowledge Layer" drops.

### §02/09 entity_lifecycle schema_evolution → §03 schema/versioning

1-page redirect. Fold.

### §24 numbering tightening

`00, 01, 07, 08` → `00, 01, 02, 03`. Trivial rename.

## Renumbering map (old → new chapter index)

| Old | New | What |
|---|---|---|
| §00 master_overview | **00** overview | rename |
| §01 system_architecture | **01** architecture + design-wedges | add wedges |
| §02 data_model | **02** data-model | + folded §17/§18/§19/§14 record-type chapters |
| §03 wire_protocol | **04** wire-protocol | 8 dupes merged |
| §04 embedding_layer | **08** embedding | renamed |
| §05 storage_arena_wal | **09** storage | renamed |
| §06 ann_index | **10** indexing | renamed |
| §07 metadata_graph | **11** metadata | renamed + knowledge_layer_storage folded |
| §08 query_planner | **13** query-optimizer | renamed (matches arc-labs convention) |
| §09 cognitive_operations | **05** write-pipeline + **06** read-pipeline | split by direction |
| §10 concurrency_epochs | **16** concurrency | renamed |
| §11 background_workers | **17** background-workers | 3 dupes merged |
| §12 sharding_clustering | **18** sharding | renamed |
| §13 sdk_design | **07** sdk | renamed + moved up to public surface |
| §10 observability_ops | **19** observability | renamed |
| §14 failure_recovery | **20** failure-recovery | unchanged role |
| §14 benchmarks_acceptance | **21** benchmarks | renamed |
| §14 knowledge_model | **(dissolved into §02)** | |
| §14 entities | **(dissolved into §02 + §12)** | |
| §14 statements | **(dissolved into §02 + §10)** | |
| §14 relations | **(dissolved into §02)** | |
| §21 schema_dsl | **03** schema | renamed + moved up |
| §22 extractors | **12** extractors | renumber |
| §23 retrievers | **14** retrievers | drops "hybrid query" framing |
| §24 provenance_versioning | **15** provenance | renamed + numbering tightened |

## Execution order

| Step | Action | Risk |
|---|---|---|
| 1 | Draft this plan ← (✓ this file) | none |
| 2 | Add `docs/architecture-learnings.md` (separate doc) capturing what to borrow from Neo4j/Pinecone/Weaviate/CockroachDB/TigerBeetle/FoundationDB/arc-labs | none |
| 3 | Dedup the §03 knowledge_* dupes (8 file merges, paths unchanged yet) | low |
| 4 | Dedup the §11 knowledge_* dupes (3 file merges) | low |
| 5 | Tighten §24 numbering (00, 01, 07, 08 → 00, 01, 02, 03) | trivial |
| 6 | Drop §02/14 knowledge_layer_storage title; merge content into §02/00 + §02/02 | low |
| 7 | Fold §02/09 entity_lifecycle into §21 (will become §03) | low |
| 8 | Fold §14 knowledge_model into §02 (4 new chapters) | medium |
| 9 | Fold §18/§19/§14 contents into §02 + §11/§13 destinations | medium-high |
| 10 | **Naming sweep** — replace every term in the naming table across all spec files | medium-high |
| 11 | **Reframe** — rewrite `00_index.md` + `01_architecture/00_purpose.md` with new positioning + Brain-vs-X comparison | medium |
| 12 | **Add Design Wedges** — write `01_architecture/design-wedges.md` (new file) | low |
| 13 | Section renames + renumber — bulk `git mv` per the renumbering map | high (path refs) |
| 14 | Write chapter overview files (one per section, ~22 new files at spec/ root) | medium (new prose) |
| 15 | Update every `spec/N_M/...` path reference in CLAUDE.md, ROADMAP.md, AUTONOMY.md, docs/development/phases/* | high |
| 16 | Update intra-spec `§N/M` shorthand citations | high |
| 17 | Rewrite `00-overview/doc-map.md` to reflect 22 sections + new order + design wedges | low |
| 18 | Final verification: no `knowledge_*` files, no "hybrid query" / "cognitive substrate" / "three-layer" prose, all links resolve | gate |

Each step is a commit-worthy unit. Steps 3-12 are content edits with stable paths. Step 13 is the path-rename pass. Step 14 adds new top-level chapter files. Steps 15-16 are bulk perl substitutions.

## Verification

```bash
# After full migration:
find spec -name "*knowledge*"                             # → 0 files
find spec -maxdepth 1 -name "*.md" | wc -l                # → 22 chapter files + README
find spec -maxdepth 1 -type d | wc -l                     # → 22 subdirs + spec/

grep -rln "knowledge layer\|three.layer model\|substrate layer\|cognitive substrate" spec/   # → 0
grep -rln "hybrid query router\|hybrid query engine" spec/   # → 0
grep -rln "ANN index" spec/ | grep -v "HNSW algorithm"    # → 0 (the role is "vector index")
grep -rln "phase [0-9]" spec/                             # → 0 (or near-0)

grep -rln "spec/2[1-4]_\|spec/1[7-9]_\|spec/20_" spec/ CLAUDE.md ROADMAP.md AUTONOMY.md docs/   # → 0 stale paths
```

Plus an mdlint pass to catch broken relative links.

## Precedent

- **[Neo4j docs](https://neo4j.com/docs/)** — Getting Started / Cypher / Operations / Drivers / Graph Data Science. No engine-layer split. Labeled-property graph is the data model from page 1.
- **[Pinecone docs](https://docs.pinecone.io/)** — Introduction / Architecture / Indexes / API Reference / Integrations. Concepts → architecture → API.
- **[Weaviate docs](https://docs.weaviate.io/)** — Concepts (each concept a self-contained chapter) / Quickstart / Configuration / Manage Data / Search.
- **[CockroachDB design.md](https://github.com/cockroachdb/cockroach/blob/master/docs/design.md)** — one document, 19 top-level sections.
- **[FoundationDB docs](https://apple.github.io/foundationdb/contents.html)** — Why → Technical Overview → Client Design → Design Recipes → API → Admin → Storage Engine.
- **[TigerBeetle ARCHITECTURE.md](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/ARCHITECTURE.md)** — Problem Statement → Overview → Design Decisions → Conclusion → References.
- **[arc-labs Recall spec](file:///Users/dodo/Desktop/arc-labs/docs/spec/)** — 19 flat files, names like `04-write-pipeline.md`, `13-query-optimizer.md`. Direct sibling project; closest pattern to what we want.

None of them splits the spec by layer. All of them name things by what they do, not by where they sit in some hierarchy.

## What this is NOT

- Not a code refactor. Crates, modules, types, wire format unchanged.
- Not a v1.1 plan. v1.0 acceptance gate untouched.
- Not a content rewrite (mostly). Prose inside files mostly stays; titles, frontmatter, and post-consolidation residue change.

Just: the spec layout matches what the system is — *a modern memory database for agentic AI*.
