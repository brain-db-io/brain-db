# Phase 15 — Spec ↔ implementation audit

**Task:** Final pre-release pass. For each of the 17 spec sections,
verify that the implementation either honors every MUST or has a
tracked deferral / SD entry.

**Reads:** `spec/` (218 files, ~42 K lines); existing
`docs/spec-deviations.md`; phase docs; runtime crates.

---

## 1. Why

We're at "v1.0.0 release-ready, awaiting reference-hardware soak".
Before the release cut, do a systematic spec ↔ impl pass. The
substrate has been built phase by phase, so each phase doc has its
own exit checklist — but no single document confirms that the
sum of those checklists is **spec-faithful**. This is that
document.

The existing surfaces this rolls up:

- `docs/spec-deviations.md` — 19 conscious deviations recorded
  during Phases 2-4 (storage, embedding, index). Mostly spec
  typos + design-clarification choices. No new entries from
  Phases 5-14.
- Inline `phase-NN/<slug>` markers — every deferred surface in
  the runtime carries a tracker (e.g. `phase-12/hnsw-sampling`,
  `phase-11/audit-log`). The audit cross-references these.
- Per-phase exit checklists — each completed phase ticked its
  spec-mandated outputs.

This phase pulls these strands into one auditable record per
spec section.

## 2. Scope

Full audit of the 17 spec sections is ~500-700 individual MUST
clauses. That's not feasible in a single autonomous loop. The
plan splits the audit into three tiers:

**Tier A — audit at depth in this loop (3 sections):**

- §03 Wire protocol (12 files) — stability surface for v1.0;
  every frame field is part of the wire ABI.
- §05 Storage / arena / WAL (12 files) — durability is THE
  invariant; 9 deviations already recorded; deserves a
  consolidated audit.
- §14 Observability (12 files) — just shipped; deferred metrics
  are well-documented; fresh in mind.

**Tier B — worklist + light scan in this loop (the other 14):**

For each remaining section: count MUST clauses, identify owning
crate(s), set audit priority. Each gets a stub page in
`docs/spec-audit/` ready for a deeper pass.

**Tier C — operator-run later:**

Tier B audits driven by the operator on cadence. Each audit
follows the same template (see §3).

## 3. Methodology

For each spec section:

1. **Enumerate MUSTs.** Spec text uses "must", "MUST", "required",
   "always", "never" as the normative keywords. Grep for them.
2. **Find evidence.** For each MUST clause, locate the
   implementation file + line range that satisfies it. The output
   is a table: `clause → file:lines → status`.
3. **Classify status:**
   - **Matched** — impl honors the clause.
   - **Deferred** — clause not yet implemented; carries a
     `phase-NN/<slug>` tracker.
   - **Deviated** — impl differs; has an SD entry in
     `docs/spec-deviations.md`.
   - **Drift** — impl differs and there's *no* tracker / SD
     entry. **These are bugs to surface.**
4. **Write up.** One file per section at
   `docs/spec-audit/s<NN>-<name>.md`.

The index at `docs/spec-audit/README.md` rolls up:

- Per-section coverage (X / Y MUSTs audited).
- Drift count (should be 0 for a clean release).
- Deferred count (acceptable; trackers documented).

## 4. Output shape

Per-section audit file template:

```markdown
# Spec audit — §NN <name>

**Spec files:** spec/NN_<name>/*.md (count)
**Implementation:** crates/<crate>/...
**MUSTs scanned:** N
**Status:** all-matched / N deferred / M drift

## Summary
<one-paragraph rollup>

## Findings

| # | Clause | Spec ref | Impl evidence | Status |
|---|---|---|---|---|
| 1 | "<verbatim must clause>" | §NN/03 §2 | crates/brain-x/src/y.rs:42 | matched |
| 2 | ... | ... | ... | deferred (phase-NN/<slug>) |
| 3 | ... | ... | ... | deviation (SD-N.M-K) |
```

## 5. Done when

- [ ] `docs/spec-audit/README.md` framework + status table.
- [ ] Three Tier-A audit pages complete.
- [ ] Fourteen Tier-B stub pages with MUST-count + priority.
- [ ] Zero **drift** findings in Tier-A. Any drift becomes a new
      SD entry or a tracker.
- [ ] Commit per the standard message shape.

## 6. Risks

- **MUST-grep false positives.** The word "must" appears in
  non-normative contexts (e.g. "you must understand X to follow").
  Filter visually before counting.
- **Implementation evidence drift.** Line numbers change. Cite
  file + function name + a verbatim snippet anchor rather than a
  bare line range when possible.
- **Scope creep.** The temptation is to fix every minor drift
  inline. Don't. Surface drift as findings; fixes go in follow-up
  PRs.

## 7. Commit shape

```
docs(audit): phase 15 — spec ↔ impl pass

Adds docs/spec-audit/ — per-section audit pages following the
methodology in .claude/plans/phase-15-spec-audit.md.

Tier A (full audit):
- s03-wire-protocol.md   — N MUSTs scanned, 0 drift
- s05-storage.md         — N MUSTs scanned, 0 drift (9 SDs cross-ref'd)
- s14-observability.md   — N MUSTs scanned, M deferred (all trackered)

Tier B (worklist):
- s01..., s02..., ...    — MUST-count + priority for future passes

The index README rolls up per-section coverage. Drift count: 0.

Refs: docs/spec-deviations.md (cross-referenced),
       crates/brain-server/src/metrics/mod.rs (tracker slugs).
```
