# 17.9 — Confidence aggregation

Pure-function implementation of the noisy-OR confidence formula per
spec §19/04. Lives in `brain-core` (no I/O), tested standalone,
optionally wired into `statement_ops::statement_create /
_supersede` so callers passing per-evidence metadata get
aggregation; callers passing the wire-shape "no-metadata" inline
evidence keep their caller-supplied `confidence` verbatim.

```
confidence(S, now) = 1 - Π (1 - c_i · decay(age_i, kind))
```

## Spec refs

- `spec/19_statements/04_confidence.md` §1–§8 — formula, decay,
  recomputation triggers, bucketing, edge cases, exact test list.
- `spec/19_statements/05_evidence.md` — evidence shape backing the
  aggregation.
- `spec/19_statements/06_open_questions.md` Q3 + Q4 — what's
  deferred (read-time recompute; lazy sweep worker).

## Reads-only files

- `crates/brain-core/src/knowledge/statement.rs` — `EvidenceEntry`,
  `EvidenceRef`, `StatementKind` already there from 17.2.
- `crates/brain-metadata/src/statement_ops.rs` — `statement_create`
  call sites that 17.9 lightly extends.
- `crates/brain-metadata/src/tables/knowledge/statement.rs` —
  `confidence_bucket(c)` helper already lands here (17.4).

## Key design decisions

### D1 — Pure function in `brain-core`

`aggregate_confidence` is I/O-free, deterministic, stateless. Lives
in `crates/brain-core/src/knowledge/confidence.rs`. No dep on
`brain-metadata` (no clock; caller passes `now_unix_nanos`).

### D2 — `ConfidenceConfig` defaults match spec §3

```rust
pub struct ConfidenceConfig {
    pub fact_half_life_seconds: u64,    // default 31_536_000 (365 days)
    pub pref_half_life_seconds: u64,    // default 5_184_000  (60 days)
    pub event_decay_disabled: bool,     // default true
}
```

`Default` impl + `const fn default_v1()` for compile-time
construction.

### D3 — Hookup into `statement_ops::statement_create / _supersede`

The v1 wire shape drops per-evidence metadata (confidence /
timestamp / extractor) in `EvidenceRefWire::Inline`; callers via
the SDK get `EvidenceEntry { confidence_milli: 0, .. }` entries
on the server side.

Rule: **if any inline evidence has `confidence_milli > 0`,
recompute the statement's `confidence` via `aggregate_confidence`.
Else trust the caller-supplied `confidence`.**

This means:
- SDK / wire callers keep the wire-supplied statement-level
  confidence (per-evidence metadata absent).
- In-process callers (phase 22 extractors, unit tests calling
  `statement_create` directly with `EvidenceEntry::from_parts(...)`)
  trigger server-side aggregation.

Documented as a v1 limitation; phase 22's `STATEMENT_ADD_EVIDENCE`
op (spec §19/06 Q5) brings per-evidence metadata through the wire.

### D4 — Overflow path uses the overflow row's parallel vectors

`EvidenceRef::Overflow(id)` resolves through `evidence_overflow_load`
which returns `Vec<EvidenceEntry>` populated from the four parallel
vectors. Aggregation then runs over the full set. No change needed —
the existing overflow row already carries per-entry metadata.

### D5 — No bucket re-indexing in 17.9

Spec §19/04 §6: when confidence changes by > 0.05 across recompute,
`statement_ops` re-keys the `STATEMENTS_BY_PREDICATE_TABLE` entry.
This is **not** required in v1 — confidence is only computed once
at `statement_create` time (no lazy recompute). The re-key path
lands in phase 21+ alongside the periodic confidence-sweep worker.

A doc comment in `statement_ops::statement_create` flags this
deferral.

### D6 — Clock skew via `saturating_sub`

`age_secs = (now.saturating_sub(timestamp)) / 1e9`. Future
timestamps → age = 0 → decay = 1.0. No panic, no negative ages.

### D7 — f32 precision

`f32` throughout (matches the storage row + wire). For 100 evidence
each c=0.1, `(0.9)^100 ≈ 2.66e-5` — well within f32 representable
range. Spec test §8 expects ≥ 0.99 — covered.

## Plan

### Step 1 — `brain-core/src/knowledge/confidence.rs`

New module:

```rust
//! Confidence aggregation per spec §19/04.

use crate::knowledge::{EvidenceEntry, StatementKind};

pub struct ConfidenceConfig {
    pub fact_half_life_seconds: u64,
    pub pref_half_life_seconds: u64,
    pub event_decay_disabled: bool,
}

impl ConfidenceConfig {
    pub const fn default_v1() -> Self { ... }
}
impl Default for ConfidenceConfig { ... }

pub fn aggregate_confidence(
    evidence: &[EvidenceEntry],
    now_unix_nanos: u64,
    kind: StatementKind,
    config: &ConfidenceConfig,
) -> f32 { /* per spec §5 verbatim */ }
```

### Step 2 — Re-exports

`crates/brain-core/src/knowledge/mod.rs`:

```rust
pub mod confidence;
pub use confidence::{aggregate_confidence, ConfidenceConfig};
```

### Step 3 — Wire into `statement_ops`

In `brain-metadata::statement_ops`:

- `statement_create`: after validation but before insert, if inline
  evidence has any entry with `confidence_milli > 0`, call
  `aggregate_confidence` and overwrite the statement's confidence
  in the to-insert row. Same for overflow path (resolve via
  `evidence_overflow_load`, aggregate, overwrite).
- `statement_supersede`: same logic on the new statement.
- Bucket re-keying deferred per D5 — flag with `// TODO(phase 21)`.

### Step 4 — Tests

Spec §8 lists 10 cases verbatim:

- `empty_evidence_zero`.
- `single_evidence_full_confidence_fact_zero_age` → 1.0.
- `two_evidence_each_0_9_no_decay` → 0.99 ± epsilon.
- `fact_at_one_year_age` → ~0.45 ± epsilon.
- `preference_at_60_day_age` → ~0.45.
- `event_no_decay_at_5_year_age` → 0.9 ± epsilon.
- `hundred_evidence_each_0_1_no_decay` → ≥ 0.99.
- `future_timestamp_clamps_to_zero_age`.
- Property: `confidence_monotonic_in_evidence_count` (using `proptest`).
- Property: `confidence_in_unit_interval` (using `proptest`).

Plus a sanity test on `ConfidenceConfig::default_v1()` byte values.

In `brain-metadata`:

- `statement_create_aggregates_when_evidence_has_metadata` —
  build a statement with per-evidence confidence_milli; verify
  stored confidence reflects aggregate.
- `statement_create_keeps_wire_confidence_when_evidence_lacks_metadata`
  — build with all `confidence_milli = 0`; verify caller-supplied
  confidence is preserved.

### Step 5 — Verify

```
cargo test -p brain-core knowledge::confidence
cargo test -p brain-core
cargo zigbuild --target x86_64-unknown-linux-gnu -p brain-metadata --tests
cargo zigbuild --target x86_64-unknown-linux-gnu --workspace --tests
cargo clippy -p brain-core -p brain-metadata --all-targets -- -D warnings
```

## Files written

| Path | Change |
|---|---|
| `crates/brain-core/src/knowledge/confidence.rs` | New. ~150 lines (impl + ~10 unit + 2 proptests). |
| `crates/brain-core/src/knowledge/mod.rs` | Add module + re-exports. |
| `crates/brain-metadata/src/statement_ops.rs` | Lightly extend `statement_create` + `statement_supersede` per D3. |

## Commit message draft

```
feat(brain-core): confidence aggregation (17.9)

Pure-function noisy-OR aggregation per spec §19/04 §1:
  confidence(S, now) = 1 - Π (1 - c_i · decay(age_i, kind))

aggregate_confidence(evidence, now, kind, config) → f32. Per-kind
decay (Fact: 365d half-life / Preference: 60d / Event: none) is
configurable via ConfidenceConfig::default_v1.

statement_ops::statement_create and _supersede recompute confidence
via aggregate_confidence iff inline evidence carries per-entry
metadata (confidence_milli > 0); otherwise the caller-supplied
wire-level confidence is preserved. This rule splits in-process
callers (phase 22 extractors, brain-metadata unit tests) from
wire/SDK callers — the v1 wire shape doesn't carry per-evidence
metadata. The richer ADD_EVIDENCE path lands in phase 22.

10 unit tests + 2 proptests cover empty-evidence (0.0), single
full-confidence (1.0), 2× 0.9 → 0.99, decay half-lives at 1y / 60d,
Event no-decay at 5y, 100× 0.1 → ≥ 0.99, clock skew, and the
monotonicity / [0,1] bounds invariants. Plus 2 brain-metadata
integration tests verifying the aggregation hookup.

Bucket re-indexing on confidence-bucket changes (§19/04 §6) defers
to phase 21 alongside the periodic confidence-sweep worker; a TODO
marker records the gap.

Plan: .claude/plans/phase-17-task-09.md.
```

## Risks

- **Wire-shape gap**. v1 SDK callers see no aggregation effect.
  Documented; users hitting it can use `statement_ops` directly or
  wait for phase 22's ADD_EVIDENCE.
- **`f32` cumulative error** at 100+ evidence is small but
  measurable. Tests assert `>= 0.99` rather than exact equality.
- **proptest** is already a dev-dep of brain-core (per Cargo.toml).
  No new deps.

## Out of scope

- Lazy recompute at read time (§19/04 §4 — phase 22+).
- Confidence-sweep worker (§19/04 §4 / §6 — phase 21+).
- Per-predicate decay overrides (§19/04 §3.4 — phase 19 schema DSL).
- Down-weighting contradicting Facts (§19/04 §"Open questions" —
  post-v1.0).
- Bucket re-indexing on confidence delta > 0.05 — phase 21.
