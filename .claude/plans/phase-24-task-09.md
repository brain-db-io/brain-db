# Plan: Phase 24 — Task 09, Schema-toggle runbook

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1
**Depends on:** 24.1 (backfill worker), 24.8 (schema
                migration runner).

---

## 1. Scope

A step-by-step operations document at
`docs/runbooks/schema-toggle.md` for operators who want to:

1. **Declare a schema** on an existing deployment that's been
   running in substrate-only mode.
2. **Run the backfill** to extract knowledge-layer statements
   from existing memories.
3. **Verify** the resulting hybrid query path works.
4. **Revert to substrate-only mode** (the substrate keeps
   serving; knowledge-layer data is preserved but unused).

24.9 is **documentation-only**. No code. The runbook
references the workers + opcodes the previous sub-tasks
shipped; nothing new is built here.

Concrete deliverables (one commit):

1. **`docs/runbooks/schema-toggle.md`** (new) — the runbook.
2. **`docs/runbooks/README.md`** (new or updated) — index
   for ops runbooks under `docs/runbooks/`.
3. Cross-refs from `ROADMAP.md` ("Operational runbooks") and
   the phase doc.

## 2. Spec references

- `spec/28_knowledge_wire_protocol/08_schema_optional_mode.md`
  — schema-declaration state machine + gate behaviour.
- `spec/31_complete_acceptance/00_purpose.md` §"Schema-on /
  schema-off transitions acceptance" — what 24.10's e2e
  test validates.
- `spec/21_schema_dsl/00_purpose.md` — schema document
  shape.

## 3. External validation

| Item | Source | Status |
|---|---|---|
| `SCHEMA_UPLOAD` wire flow | phase 19 + 24.0 | shipped |
| Backfill request shape | 24.1 | new |
| Migration plan response | 24.8 | new |
| `RECALL_REQ` transparent hybrid routing | 23.11 | shipped |

## 4. Architecture sketch

```
docs/runbooks/schema-toggle.md

# Schema toggle runbook

## When to use this

You're running Brain in substrate-only mode (no schema
declared). You want to:
  a) Declare a knowledge-layer schema, OR
  b) Migrate an existing schema, OR
  c) Revert a schema-declared deployment to substrate-only.

## Prerequisites

- Brain server reachable.
- Schema file (`my-schema.brain`) reviewed.
- LLM API keys configured if extractors include LLM tier.
- A maintenance window (the backfill consumes shard time).

## Pre-flight checklist

- [ ] Backup the data directory (`tar` snapshot).
- [ ] `brain-cli admin stats` healthy.
- [ ] LLM cost budget reviewed.

## Step 1 — Validate the schema (dry-run)

  brain-cli schema validate --file my-schema.brain

Expected: 0 errors. Address each before proceeding.

## Step 2 — Declare the schema

  brain-cli schema upload --file my-schema.brain

Expected response:
- `schema_version: 1`
- `validation_errors: []`
- `migration_summary: { total_items: 0 }`  (no prior schema)

What happens server-side (per §28/08 §1):
- The per-shard `SchemaGate` flips from `false` to `true`.
- Substrate RECALL now routes through the hybrid pipeline
  (spec §28/08 §5).

## Step 3 — Backfill existing memories

  brain-cli admin backfill --extractors all --memories all

Progress is streamed:
  Backfill: 1235/100000 (1.2%), failed: 0, eta: 14m

Tunables:
- `--extractor pattern,classifier` — limit tiers.
- `--memory-range from..to` — partial backfill.
- `--dry-run` — print the plan without extracting.

If interrupted: rerun the same command; the worker resumes
from the last completed item (24.1 §"Resume semantics").

## Step 4 — Verify

  brain-cli query "test cue"
    # Returns hits with `contributing_retrievers` populated.

  brain-cli statement list --subject <entity-id>
    # Returns extracted statements from the backfill.

## Step 5 — Reverting (substrate-only fallback)

There is no `SCHEMA_DROP` opcode in v1 (spec §28/08 §1). To
return to substrate-only behaviour:

  Option A — stop using knowledge-layer ops.
  Knowledge data stays on disk; substrate primitives
  unchanged.

  Option B — empty-out the active-schema pointer.
  Stop the server. Manually remove the namespace row from
  `schema_active_versions` (redb CLI). Restart. The
  `SchemaGate` re-seeds from metadata at startup and reads
  empty → `false`. Hybrid path disabled; substrate RECALL
  is plain semantic.

Option B is destructive of the schema pointer (not the data).

## Pitfalls

- A backfill against millions of memories can cost real LLM
  money. Always `--dry-run` first.
- The first RECALL after gate flip incurs a one-time
  retriever initialisation. Subsequent calls are fast.
- Migration runs cooperatively under the Background lane
  (20% shard time). Heavy migrations are slow but don't
  starve RECALL.

## Recovery

  - Backfill stuck: `brain-cli admin backfill cancel`; rerun.
  - Migration stuck: `brain-cli admin schema migration cancel`.
  - Schema upload failed mid-flight: redb is transactional;
    either the upload landed or it didn't. Check
    `schema list` to see the active version.
```

### `docs/runbooks/README.md`

Short index listing this runbook + any future ones. Anchored
from ROADMAP under "Operational runbooks".

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Single Markdown runbook (this plan) | One file to update; matches industry norm | Less testable than scripts | ✓ |
| Bash scripts in `scripts/` only | Executable | Operators want prose explanation alongside commands | scripts live in 24.10; this is the human-readable accompaniment |
| Spec section instead of runbook | Co-located with design | Runbooks have a different audience (ops vs developers) | runbook in `docs/runbooks/` |
| Include reversal via `SCHEMA_DROP` opcode | Cleaner | Spec §28/08 §1 explicitly says v1 has no SCHEMA_DROP | document the manual approach (option B) |

## 6. Risks / open questions

- **Risk:** Runbook drifts from implementation as CLI flags change. **Mitigation:** Step 5 references the `schema list` shape, not exact CLI internals; the runbook is reviewed at phase 24 exit.
- **Open question:** Should this runbook live under `docs/` or in the repo root as `RUNBOOK-schema-toggle.md`? **Resolution:** `docs/runbooks/` — keeps the root clean; mirrors industry conventions.

## 7. Test plan

24.9 is docs-only. No code; no tests. Verified by:

- `grep -r '\[`./0[0-9]_' docs/runbooks/` returns no broken refs.
- Markdown link-check passes.
- The corresponding e2e test in 24.10 executes the steps in this runbook (smoke validation).

## 8. Commit shape

```
docs(runbooks): 24.9 — schema toggle operator runbook

- docs/runbooks/schema-toggle.md (new): step-by-step ops
  document covering schema declaration, backfill, verification,
  and the manual revert path.
- docs/runbooks/README.md (new): index for ops runbooks.
- ROADMAP.md: link "Operational runbooks" section to
  docs/runbooks/.

No code; no test gates.
```

## 9. Confirmation

1. **Single Markdown runbook** at `docs/runbooks/schema-toggle.md` — matches industry norm.
2. **5 numbered steps**: validate → declare → backfill → verify → revert.
3. **Revert covered manually** (no `SCHEMA_DROP` opcode in v1) — option A (stop using) + option B (manual redb edit).
4. **Cross-referenced** from ROADMAP "Operational runbooks" section.
5. **24.10's e2e test mirrors the steps** — runbook stays evergreen because the test breaks first.
