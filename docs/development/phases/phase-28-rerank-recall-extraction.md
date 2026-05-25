# Phase 28 — Rerank always-on; extraction productive on the first-class merged schema

> **Status:** proposed — awaiting approval. Revised after design direction: **rerank is a first-class always-on feature (no request flag)**, and **schema is a first-class merge-based state (system default + user expansions), never a flag**. The system default already declares a rich type vocabulary, so extraction returning 0 is a pipeline bug, not a seeding gap.

## Design corrections driving this revision

1. **Rerank is first-class, always-on.** It must work without any argument. There is no `--rerank` flag and no `request.rerank` bool. Rerank runs on every read whenever the cross-encoder is loaded. The *only* knob is the deploy-time `config.rerank.enabled`, which controls whether the model loads at all (operator opt-out → graceful RRF-only). Clients never toggle it per-request.

2. **Schema is first-class, merge-based, always active.** Brain boots with the system-default `brain:` namespace and every user `SCHEMA_UPLOAD` merges additively on top — the active schema only ever expands. Schema is not a flag or a mode. (This is already the phase-26 behaviour; this phase reinforces it with a first-class upload verb and by making extraction actually use it.)

3. **The system default is already productive.** `crates/brain-metadata/src/system_schema/schema.brain` declares `Person`, `Organization`, `Project`, `Event`, `Place`, `Concept` + ~25 predicates + relation types. So `entities=0` on encode is **not** a missing-schema problem — the extraction pipeline isn't promoting candidates against a vocabulary that already exists. That's the bug to fix.

## Pre-work — Step 0 (commit the pending fix)

The XLM-RoBERTa loader fix + env-race hardening are validated but uncommitted (5 files):
- `brain-rerank: load bge-reranker-base as XLM-RoBERTa (fix BERT mismatch)` — `model.rs` + `lib.rs`.
- `server+scripts: rerank on by default, bootstrap provisions it` — `bootstrap-model.sh` + `shard/mod.rs` + `shard/adapters.rs`.

---

## Theme A — Rerank becomes first-class (remove the flag, always run)

### WS1 — Delete the per-request rerank flag from the wire
- **Read / write:**
  - `brain-protocol/src/ops/memory.rs:108` — remove `pub rerank: bool` from `RecallRequest`.
  - `brain-planner/src/hybrid/router.rs:73` — remove `rerank` from the planner `QueryRequest`; the executor decides by capability, not by request.
  - `brain-protocol/src/ops/capabilities.rs:33-36` — keep a `rerank` *availability* bit in `GET_CAPABILITIES` (so clients know whether results are reranked) but drop the "clients should drop `RecallRequest.rerank`" framing.
  - `envelope/request.rs:484`, `envelope/response.rs:625,637` — drop the field from constructors/round-trips.
- **Done when:** `rerank` no longer appears as a request field anywhere; the wire carries no rerank toggle.

### WS2 — Read pipeline always reranks when the model is loaded
- **Read / write:**
  - `brain-ops/src/handlers/recall.rs:51` — delete the `req.rerank && !enabled → CapabilityNotEnabled` hard-fail (no flag to conflict with).
  - `recall.rs:489` + `query.rs:221,281` — stop passing a per-request `rerank`; the executor runs the rerank stage iff `cross_encoder` is `Enabled`.
  - `brain-planner/src/hybrid/executor.rs` — the rerank stage fires whenever `ctx.cross_encoder.is_some()`; when `None` (operator disabled the load), RRF-only, no error.
- **Result:** RECALL **and** QUERY both rerank by default on any shard with the model loaded. Operator opt-out (`[rerank] enabled = false`) → graceful RRF-only everywhere.
- **Done when:** a bare `recall "<exact phrase>"` (no flags) floats the exact match to #1 via the cross-encoder.

### WS3 — Honest recall card (rerank always reflected)
- **Read / write:** `brain-shell/src/commands/recall.rs` + the shared `MemoryResult` renderer.
- Show the **fused score** (the actual rank key, from `confidence`) as the primary column, semantic cosine as a secondary `sem=`, and the rerank score as `rr=` when rerank ran. Header marks `⟳ reranked` vs `RRF-only` so it's clear which ordering you're looking at.
- No rerank flag added to shell or SDK (the opposite of the discarded approach).
- **Decision point:** column layout — confirm `score`(fused) + `sem`(cosine) + `rr`(rerank) before implementing.

### WS4 — Spec + CLAUDE.md alignment (user-directed spec change)
- `spec/13_retrievers/06_post_processing.md`, `spec/13_retrievers/00_purpose.md`, `spec/13_retrievers/05_hybrid_query.md`, and CLAUDE.md §4 currently say rerank is "opt-in per RECALL call." Rewrite to: **rerank always runs when the cross-encoder is loaded; the only control is the deploy-time `config.rerank.enabled` load gate.**
- Spec is normally read-only — these edits are explicitly user-directed; I'll surface the deltas for confirmation before committing.

---

## Theme B — Extraction produces entities on the merged schema

The system schema already declares the types; the pipeline isn't yielding them. This is diagnosis-first.

### WS5 — Diagnose & fix `entities=0`
- **Diagnose (step 1, no code):** with GLiNER loaded and `registry_size=3`, trace why `do_extractor_cycle` emits `entities=0` for text full of obvious entities ("Priya Sharma joined Stripe"). Candidate root causes to rule out, in order:
  1. Classifier tier registered but **not actually invoking GLiNER** (only the pattern tier runs).
  2. GLiNER invoked but **labels not derived** from the active `brain:` entity-type qnames (zero labels → zero spans).
  3. Candidates produced but **dropped at the write stage** (resolution, schema-version check, or confidence gate).
  4. A tier-enable/config mismatch (`extractors.classifier.enabled`).
- **Fix** the identified break so `encode` against the system default yields persisted entities/statements with **no manual schema step**.
- **Done when:** fresh shard → `encode "Priya Sharma joined Stripe as a Senior Engineer in San Francisco"` → `recall --include-graph` shows `Person:Priya Sharma`, `Organization:Stripe`, `Place:San Francisco` (or the subset GLiNER + the schema support), with audit `entities>0`.

### WS6 — First-class schema-expansion verb (shell)
Schema is first-class, but the shell has **no `schema` verb** (verb enum in `command.rs` has entity/statement/relation/mention, not schema), and `brain-cli` has none either. Users can't expand the merged schema today.
- **Write:** add a `schema` verb mirroring the `entity`/`statement` verb structure — `schema upload --from-file <PATH>`, `schema get <ns>`, `schema list`, `schema validate --from-file <PATH>` — wired to the existing `SCHEMA_UPLOAD/GET/LIST/VALIDATE` opcodes (and `SCHEMA_REPLACE` behind an explicit `--force-drop` for the destructive case). Confirm the SDK schema client exists (`brain-sdk-rust/src/ops/schema.rs`) and reuse it.
- **Framing:** the verb *expands* the active merged schema (additive); it is not a mode toggle. `schema list` shows system default + every user namespace merged in.
- **Done when:** `schema upload --from-file my.brain` adds a user type that then extracts on the next encode, sitting alongside the system `brain:` types.

---

## Sequencing
1. **Step 0** — commit the pending loader fix.
2. **Theme A (WS1–WS4)** — one worktree: wire-flag removal + always-on rerank + card + spec. Cohesive; land together.
3. **Theme B (WS5–WS6)** — second worktree: extraction diagnosis/fix + schema verb. Independent of A (different files), can run in parallel.

## Constraints (carried)
- No `config/dev.toml` edits. No `Co-Authored-By: Claude` trailers. No `pub use X as Y` aliases. No `// Spec §X/Y` ref comments.
- Spec edits (WS4) are user-directed; surface deltas before committing.
- Verify in the Linux devcontainer (`CARGO_BUILD_JOBS=2`); **release build** for any latency/extraction-quality claim (debug candle is ~10–50× slower — that's why the Semantic retriever logged 250 ms).

## Verification (end state)
- Bare `recall "Alice merged the auth-rewrite branch"` (no flags) → exact match ranks #1 (rerank ran automatically); card shows fused + `rr` scores, header `⟳ reranked`.
- Shard with `[rerank] enabled = false` → same recall returns RRF-only, header `RRF-only`, no error.
- Fresh shard `encode` of entity-rich text → `recall --include-graph` shows extracted entities/statements with no manual schema step.
- `schema upload --from-file <user.brain>` → a custom type extracts on the next encode.
- `just verify` green; touched crates clippy `-D warnings` clean.

## Open questions for the user
1. **WS3 card layout** — `score`(fused) + `sem`(cosine) + `rr`(rerank), header `⟳ reranked`/`RRF-only`? Or a layout you prefer?
2. **WS6** — confirm a full shell `schema` verb (upload/get/list/validate) is wanted as the first-class expansion surface.
3. **Latency posture** — rerank now runs on *every* recall. On the target hardware (release build) the spec budgets ~6–9 ms for 50 pairs. Confirm always-on is acceptable as the default even though it widens every RECALL's tail; the deploy-time `enabled=false` remains the escape hatch.
