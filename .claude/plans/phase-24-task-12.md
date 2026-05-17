# Plan: Phase 24 — Task 12, Documentation polish + phase exit + v1.0.0

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1 (one `chore(...): 24.12 — …` commit) + 2 tags.

---

## 1. Scope

The phase 24 exit + the v1.0.0 release. Three threads:

1. **Documentation polish** (the explicit 24.12 goal):
   - All `[`./0[0-9]_…`]` cross-refs across `spec/` resolve.
   - Every `*_open_questions.md` has version-target tags.
   - The spec is internally consistent (no
     `OQ-V2-N` references that don't exist;
     no §X mentions of files that have moved).
   - At least one end-to-end tutorial exists, walking from
     a blank deployment to a working hybrid query (per
     §31 §"Documentation acceptance" — `docs/tutorials/01-getting-started.md`).
2. **Phase 24 exit metadata** (matching the 21.7 / 22.8 /
   23.12 template):
   - `ROADMAP.md` Phase 24 entry rewritten.
   - `docs/phases/phase-24-acceptance.md` checkboxes flipped.
   - `spec/30_knowledge_open_questions/00_purpose.md`:
     entries for items deferred during 24.x (per-statement
     retention, in-place schema downgrade, secondary indexes
     on audit / LLM-cache, parallel migration workers).
3. **v1.0.0 release**:
   - Tag `phase-24-complete` cut.
   - Tag `v1.0.0` cut at the same commit.
   - `CHANGELOG.md` (new) — v1.0.0 entry summarising all 25
     phases (substrate phases 0–14 + knowledge layer
     15–24). Cross-references the phase docs.
   - `README.md` top — flip status from "in development"
     to "v1.0.0 released"; bump version in `Cargo.toml`.

Concrete deliverables:

1. **`scripts/spec-link-check.sh`** (new) — runs `grep -r '\[`'
   over `spec/` and `docs/`, parses out the relative paths,
   asserts each resolves. Used by 24.11's
   `full-acceptance.sh`.
2. **`docs/tutorials/01-getting-started.md`** (new) —
   end-to-end tutorial: install → encode 3 memories →
   recall → upload schema → backfill → query → inspect
   statements. ~200 lines.
3. **`CHANGELOG.md`** (new) at repo root — v1.0.0 entry +
   v0.9.0-substrate-rc (phase 14) entry +
   pointer to per-phase commit logs.
4. **`Cargo.toml`** workspace bump: `version = "1.0.0"`.
5. **`README.md`** — flip the status badge.
6. **ROADMAP / phase-doc / §30 updates** per template.
7. **`docs/phases/phase-24-acceptance.md`** rewritten to
   match the phase-22 / phase-23 finished-phase template.

## 2. Spec references

- `spec/31_complete_acceptance/00_purpose.md`
  §"Documentation acceptance" — what counts as polished.
- `spec/30_knowledge_open_questions/00_purpose.md` — host
  for OQ-24-X entries.
- `ROADMAP.md` template — established at 21.7 / 22.8 / 23.12.

## 3. External validation

| Item | Source | Status |
|---|---|---|
| Markdown link-check (or equivalent) | rg + a small awk filter | already used in the project |
| `cargo workspace publish` flow | Cargo | standard |
| GitHub Releases / git tags | git | standard |

## 4. Architecture sketch

### `scripts/spec-link-check.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail
broken=0
while IFS= read -r line; do
    file=${line%%:*}
    target=${line#*\[\`}
    target=${target%%\`*}
    if [[ "$target" != /* ]]; then
        target="$(dirname "$file")/$target"
    fi
    if [[ ! -e "$target" ]]; then
        echo "BROKEN: $file → $target"
        broken=$((broken+1))
    fi
done < <(grep -RIn '\[`\./' spec/ docs/ ROADMAP.md)
if (( broken > 0 )); then
    echo "Found $broken broken link(s); failing."
    exit 1
fi
echo "All spec/doc cross-refs resolve."
```

### `docs/tutorials/01-getting-started.md`

Skeleton:

```
# Getting started with Brain v1.0

This tutorial walks you from a blank deployment to a working
hybrid query in ~15 minutes. You'll:

1. Install + start `brain-server`.
2. Encode three memories.
3. Run a substrate recall.
4. Declare a schema.
5. Backfill the existing memories.
6. Run a hybrid query and inspect the result.

## Prerequisites
...

## 1. Install + start
...
## 2. Encode three memories
...
## 3. Substrate recall (no schema yet)
...
## 4. Declare a schema
...
## 5. Backfill
...
## 6. Hybrid query
...

## What's next
- [Operator runbook: schema toggle](../runbooks/schema-toggle.md)
- [Spec map](../../spec/00_master_overview/02_doc_map.md)
- [SDK reference](../../crates/brain-sdk-rust/README.md)
```

### CHANGELOG.md

```markdown
# Changelog

## v1.0.0 — 2026-MM-DD

First stable release of Brain.

### Substrate (phases 0–14)
- Wire protocol, storage (arena + WAL), metadata + graph,
  HNSW index, embedding (BGE-small), planner + executor,
  cognitive primitives (ENCODE / RECALL / PLAN / REASON /
  FORGET), background workers, server, observability,
  SDK + CLI.
- Tag `v0.9.0-substrate-rc` at phase 14 exit.

### Knowledge layer (phases 15–24)
- Knowledge storage (entities, statements, relations,
  LLM cache).
- Entity / statement / relation operations.
- Schema DSL.
- Three-tier extractors (pattern / classifier / LLM).
- Tantivy lexical retrieval.
- Hybrid query engine (semantic + lexical + graph; RRF
  fusion).
- Transparent RECALL routing on schema-declared
  deployments.
- Sweepers + backfill + schema migration + entity GC.
- Schema-toggle runbook + full acceptance suite.
- Tag `phase-24-complete` at the same commit as `v1.0.0`.

### Known limitations
- See `spec/30_knowledge_open_questions/00_purpose.md` for
  deferred work (streaming hybrid results, learned router,
  cross-shard hybrid fusion, etc.).
- See per-phase `*_open_questions.md` files for substrate
  deferrals.
```

### §30 additions

- **OQ-24-A: Per-statement-kind retention** — `Fact` /
  `Preference` / `Event` deserve different retention windows
  but v1 uses one knob across all kinds.
- **OQ-24-B: In-place schema downgrade** — v1 has no
  `SCHEMA_DROP`; the runbook documents a manual revert.
- **OQ-24-C: Secondary indexes on the audit / LLM-cache
  tables** — v1 sweepers scan the full table; secondary
  indexes by timestamp are post-v1 if metrics show pressure.
- **OQ-24-D: Parallel migration workers** — v1 runs one
  migration plan at a time per shard; multi-extractor
  parallelism within a plan is post-v1.

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Single commit + two tags (this plan) | Atomic phase exit + release | Touches many files | ✓ — same shape as 22.8 / 23.12, plus v1.0 release artifacts |
| Separate phase-24-complete + v1.0.0 tags from different commits | Cleaner "phase 24 then release" | Two release moments where one suffices; spec says they co-incide | unify |
| Ship a "phase 24 release candidate" tag before v1.0.0 | Extra QA window | Doesn't add safety beyond what 24.11 already validates | skip |
| Defer CHANGELOG to a follow-up | Less churn now | First release should ship with notes | include |
| Auto-generate CHANGELOG from git log | DRY | Loses the per-phase narrative | hand-written |
| Tutorial in `README.md` | One-stop | README gets bloated | separate file under `docs/tutorials/` |

## 6. Risks / open questions

- **Risk:** A late-breaking spec / code mismatch surfaces during the link-check. **Mitigation:** the link-check is run by 24.11 already; 24.12 just adds the tutorial + version bump on top of a passing acceptance suite.
- **Risk:** `cargo publish` to crates.io fails for one crate (missing license / description). **Mitigation:** workspace dry-run before tagging; the publish step is out of scope for the commit (operator runs separately).
- **Open question:** Should `v1.0.0` be tagged on `main` or `dev` first? **Resolution:** the established convention is FF main from feature → tag on main. Same here.

## 7. Test plan

24.12 is documentation + release. Validated by:

- `bash scripts/spec-link-check.sh` returns 0.
- `bash scripts/full-acceptance.sh` (from 24.11) returns 0.
- `cargo zigbuild --target x86_64-unknown-linux-gnu --workspace --tests` clean.
- The tutorial runs end-to-end against a fresh
  `brain-server` install.

## 8. Commit shape

Single commit:

```
chore(docs,workspace): 24.12 — phase 24 exit + v1.0.0 release

- docs/tutorials/01-getting-started.md (new): end-to-end
  tutorial from install through hybrid query.
- scripts/spec-link-check.sh (new): validates every
  cross-ref in spec/ + docs/ resolves.
- CHANGELOG.md (new): v1.0.0 release notes + v0.9.0
  substrate-rc.
- Cargo.toml: workspace version → 1.0.0.
- README.md: status flipped to "v1.0.0 released".
- ROADMAP.md: Phase 24 entry rewritten in the
  finished-phase template.
- docs/phases/phase-24-acceptance.md: checkboxes flipped;
  phase-exit section added.
- spec/30_knowledge_open_questions/00_purpose.md: OQ-24-A
  through OQ-24-D for items deferred during 24.x.

Verified: cargo zigbuild --target x86_64-unknown-linux-gnu
--workspace --tests (clean); cargo clippy on every touched
crate -- -D warnings; bash scripts/spec-link-check.sh;
bash scripts/full-acceptance.sh on the reference Linux box.
```

Plus two annotated tags:

```
git tag -a phase-24-complete -m "Phase 24 — Sweepers + \
  schema-migration runner + FORGET cascade + LLM cache + \
  audit + supersession + entity GC + acceptance suite."

git tag -a v1.0.0 -m "Brain v1.0.0 — substrate + knowledge \
  layer. See CHANGELOG.md for the full release notes."
```

## 9. Confirmation

1. **Single commit** for the phase exit + release artifacts; **two annotated tags** (`phase-24-complete`, `v1.0.0`) at the same commit.
2. **End-to-end tutorial** at `docs/tutorials/01-getting-started.md`; ~200 lines covering install through hybrid query.
3. **`scripts/spec-link-check.sh`** — validates every `[\`./…\`]` cross-ref resolves; called from 24.11's `full-acceptance.sh`.
4. **Hand-written CHANGELOG** summarising the substrate + knowledge phases; no auto-generation.
5. **`cargo publish` is out of scope** for this commit; the operator runs `cargo publish` after the tag lands.
