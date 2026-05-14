# Phase 14 — Acceptance & v1.0.0 Release

## Goal

Run every acceptance gate, validate every runbook, finish the docs
pass, and cut `v1.0.0`. Everything before this phase produced
components; this phase declares them, together, production-ready.

## Prerequisites

- [ ] Phase 13 complete (`phase-13-complete` tag). The acceptance suite
      consumes Phase 13's benchmark baselines and chaos results.

## Reading list

1. [`spec/16_benchmarks_acceptance/08_acceptance_test_suite.md`](../../spec/16_benchmarks_acceptance/08_acceptance_test_suite.md) — the 10 gates.
2. [`spec/14_observability_ops/07_runbooks.md`](../../spec/14_observability_ops/07_runbooks.md) — runbooks to validate.

## Outputs

- `acceptance/run.sh` — single entry point that runs gates 1-10 and
  reports pass/fail.
- `docs/runbooks/*.md` — one per runbook in spec §14/07, each tested
  against a chaos scenario produced in Phase 13.
- README + getting-started overhaul; operator guide covers install,
  config, monitor, recover.
- `CHANGELOG.md` covering v0.1 → v1.0.0.
- Tags: `phase-14-complete`, `v1.0.0`.

## Sub-tasks

### Task 14.1 — Acceptance suite runner
**Reads:** spec §16/08.
**Writes:** `acceptance/run.sh` + per-gate test files.
**Done when:** `bash acceptance/run.sh` exits 0 on the reference
environment; output is a clear pass/fail per gate.

### Task 14.2 — Runbook validation
**Reads:** spec §14/07.
**Writes:** `docs/runbooks/{disk-full,wal-corruption,shard-down,hnsw-rebuild,oom,latency-spike,etc}.md`.
**Done when:** each runbook is a working procedure executed against the
corresponding Phase 13 chaos scenario; recovery time recorded.

### Task 14.3 — Documentation pass
**Writes:** README rewrite, `docs/guides/{install,configure,operate,upgrade}.md`, `cargo doc` cleanup.
**Done when:** `cargo doc --workspace` is warning-free; every public
API in the SDK has at least one example; getting-started works from a
clean machine in under 15 minutes.

### Task 14.4 — Release notes
**Writes:** `CHANGELOG.md` covering every feature shipped Phase 1 → Phase 13; `RELEASE-NOTES-v1.0.0.md`; known-limitations section pulled from `spec/*/open_questions.md`.
**Done when:** changelog references every tagged phase; release notes
are written for an operator (not a developer) audience.

### Task 14.5 — Release cut
**Writes:** version bumps across all `Cargo.toml`s; final `v1.0.0` tag.
**Done when:** all 10 acceptance gates green, soak result attached,
release notes published, tag pushed, README points at the release.

## Phase exit checklist

- [ ] Sub-tasks 14.1–14.5 complete.
- [ ] Gates 1-10 green on reference infra.
- [ ] Every runbook executed once against a chaos scenario.
- [ ] `cargo doc --workspace` clean.
- [ ] CHANGELOG + release notes published.
- [ ] Tag `phase-14-complete` and `v1.0.0`.

## Notes

This is the last phase. Treat it as a checklist, not a feature stream —
the goal is "everything we already built works together", not "build
more". Resist the urge to fix-by-feature in this phase. Real fixes go
in a follow-up minor release.
