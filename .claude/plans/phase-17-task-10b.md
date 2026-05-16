# 17.10b — Phase 17 exit (bench + ROADMAP + tag)

Final phase-17 sub-task. Mirrors 16.9.4 + 16.9.5 (entity bench +
exit checklist).

## Spec refs

- `spec/16_benchmarks_acceptance/02_latency_targets.md` §2.3 —
  statement-layer latency targets (already added in 17.1).

## Plan

### Step 1 — `crates/brain-metadata/benches/statement_ops.rs`

Criterion bench covering the four operations the spec calls out
in §2.3:

- `statement_create::fact_3evidence` — corpus of 1M Person entities
  with one Fact each + the `brain:related_to` predicate. Wire-shape
  evidence (no per-entry metadata).
- `statement_get::point_lookup` — random `STATEMENTS_TABLE` reads.
- `statement_supersede::pref` — preference auto-supersede over an
  existing chain of length 1.
- `statement_list::subject_predicate_current` — by-subject index
  point lookup at `(subject, kind, predicate, is_current=1)`.

Each function builds a 1024-statement fixture (smaller than the
spec's 1M for bench-time; spec targets are validated on the
reference 16-core / 64 GB / NVMe rig per §1). Each iteration runs
inside a fresh read or write txn (commit per iteration for writes).

Cargo.toml — add `[[bench]] name = "statement_ops"`.

### Step 2 — `ROADMAP.md` update

Mark Phase 17 ✓ with the same "Delivered" / "Deferred to later
phases" structure as Phase 16.

### Step 3 — `Cargo.lock` follow-up

Capture deps added during the phase (smallvec features, etc.) so
the next phase starts clean.

### Step 4 — Verify

```
cargo zigbuild --target x86_64-unknown-linux-gnu --workspace --tests
cargo test -p brain-core
cargo test -p brain-sdk-rust
cargo clippy --workspace --target x86_64-unknown-linux-gnu --all-targets -- -D warnings
```

### Step 5 — Phase exit + tag

User-authorised tag: `phase-17-complete` (per memory `feedback_commit_authorship`
+ implicit "user authorises tags" rule). I'll surface the proposed
tag command but won't execute it without explicit go.

## Files written

| Path | Change |
|---|---|
| `crates/brain-metadata/benches/statement_ops.rs` | New. 4 criterion benches + fixture. |
| `crates/brain-metadata/Cargo.toml` | Add `[[bench]] name = "statement_ops"`. |
| `ROADMAP.md` | Mark Phase 17 ✓ with delivered/deferred. |
| `Cargo.lock` | Tracking changes from 17.x deps. |

## Risks

- **Bench corpus = 1024 statements vs spec's 1M.** Same trade-off as
  the entity bench; operator-run on the reference rig validates the
  full-scale numbers. Tracked in `spec/16_benchmarks_acceptance/`.
- **Wire-shape evidence has no per-entry metadata** → confidence
  aggregation isn't exercised in the bench (matches the v1
  production path). Phase 22 ADD_EVIDENCE bench will cover it.

## Out of scope

- CI regression thresholds (phase 14).
- Statement HNSW perf bench (phase 21 with the embedding worker).
- Tier-3 / tier-4 resolver benches (phase 21).
- True end-to-end bench through the wire (operator-run).
