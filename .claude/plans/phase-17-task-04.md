# 17.4 — `statement_ops` module

The CRUD + supersession + contradiction engine for the Layer-3 graph.
Mirrors `entity_ops` precedent: free functions over `redb::{Read,
Write}Transaction` so callers compose them inside their own txns. All
writes inside one redb txn — supersession and tombstone never split a
chain mid-flight.

## Spec refs

- `spec/19_statements/00_purpose.md` — overall schema, predicate
  vocabulary, operations recipe.
- `spec/19_statements/01_supersession.md` — chain mechanics,
  valid_to inheritance.
- `spec/19_statements/02_contradiction.md` — Fact-only detection,
  surface-don't-resolve.
- `spec/19_statements/03_storage.md` — every per-op write path (the
  most load-bearing doc).
- `spec/19_statements/05_evidence.md` — inline ≤ 8 → overflow
  spill, FORGET cascade hooks.

## Reads-only files

- `crates/brain-metadata/src/tables/knowledge/statement.rs` —
  `StatementMetadata` rkyv row + 8 tables.
- `crates/brain-core/src/knowledge/statement.rs` — value types from
  17.2 (`Statement`, `StatementObject`, `EvidenceRef`,
  `SubjectRef`, `TombstoneReason`).
- `crates/brain-metadata/src/entity_ops.rs` — single-shard CRUD
  precedent.
- `crates/brain-metadata/src/entity_merge_ops.rs` — multi-table
  multi-step txn precedent (atomic primary + indexes).
- `crates/brain-metadata/src/predicate_ops.rs` — predicate validation
  (17.3) used by `statement_create`.

## Key design decisions

### D1 — Object encoding (Vec<u8> in StatementMetadata.object_blob)

`brain-core::knowledge::StatementObject` is serde-only — no rkyv. The
redb row needs bytes. Two options:

- **(A)** Add rkyv derives to brain-core. **Reject.** Brain-core
  intentionally keeps rkyv out of the value crate (mirrors how
  `MemoryId` etc. don't carry rkyv).
- **(B) — chosen.** Add a small rkyv shim type
  `StatementObjectBlob` in
  `crates/brain-metadata/src/tables/knowledge/statement.rs` (private)
  with a `From<&StatementObject>` / `to_object()`. Encode via
  `rkyv::to_bytes`, decode via `rkyv::from_bytes`. Same pattern as the
  existing `EvidenceOverflow` row.

The blob discriminant byte mirrors `StatementObject::discriminant()`
from 17.2 — `1=Entity / 2=Value / 3=Memory / 4=Statement` — so
phase 23's query router can peek without full deserialisation.

### D2 — Encoding/decoding helpers live in tables module, not ops

Keep `statement_ops.rs` free of rkyv calls. `tables/knowledge/statement.rs`
gains:

- `encode_object(o: &StatementObject) -> Vec<u8>`.
- `decode_object(bytes: &[u8]) -> Option<StatementObject>`.
- `statement_from_metadata(m: &StatementMetadata) -> Statement` —
  full row → brain-core value type, including object decode + evidence
  inline/overflow projection.
- `metadata_from_statement(s: &Statement, is_current: u8) -> StatementMetadata`
  — value type → redb row.

`statement_ops` calls these but doesn't know about rkyv.

### D3 — `is_current` semantics

Per §03 §1.2 the bit is derived `superseded_by.is_none() && !tombstoned
&& valid_at(now)`. We **persist** the bit in `StatementMetadata.is_current`
so the `STATEMENTS_BY_SUBJECT_TABLE` key can flip without re-deriving.
Refresh rule: every write that mutates `superseded_by`, `tombstoned`,
or `valid_to` recomputes the bit and re-inserts the by-subject index.

We do **not** recompute on read for valid_at(now) timing — that becomes
a phase-23 query concern (cf. §06 Q3 "confidence recomputation at read
time").

### D4 — Auto-supersession is gated on `kind == Preference`

`statement_create` for kind=Preference looks up the current Preference
at `(subject, predicate, kind=Preference, is_current=1)`. If present:
delegate to `statement_supersede(old.id, new, now)` inside the same
txn — single-write-per-shard discipline keeps it atomic without
locking.

For Fact: no auto-anything. Run the contradiction detector (read-only)
and insert. Discrete `STATEMENT_CONTRADICTED` event deferred to phase
23 per §06 Q1.

For Event: no auto-anything either. Multiple events with same
`(subject, predicate, event_at)` are valid — events don't merge.

### D5 — Idempotency on caller-supplied StatementId

Caller passes a `Statement` whose `id` is set (UUIDv7). `statement_create`
rejects if `STATEMENTS_TABLE.get(&id).is_some()` with `AlreadyExists`.

Wire-side opaque request_id idempotency is wider but lives at the
handler (17.7), not here.

### D6 — `statement_history` chain-root resolution

§01 §4.1: anchor may be a chain root OR any chain member. Implementation:

```text
fn statement_history(rtxn, anchor) -> Vec<Statement> {
    // Probe STATEMENT_CHAIN_TABLE at (anchor, 1).
    // If present → anchor IS a chain_root → range-scan (anchor, *).
    // Else → load STATEMENTS_TABLE[anchor], follow .chain_root_bytes,
    //        range-scan that.
    // Either way: yield Statements in version ascending order.
}
```

### D7 — `statement_retract` schedules zero-out via tombstone byte

v1 doesn't ship the GC worker that physically removes (§05 §5 — phase
21+). `statement_retract` therefore behaves identically to
`statement_tombstone` in 17.4 with `tombstone_reason =
ExtractorRetraction` AND writes an audit marker. The GC scheduling
contract is a doc-only `// TODO(phase 21)` in the function body —
honest about the gap.

## Plan

### Step 1 — Object encoding + projection helpers

In `crates/brain-metadata/src/tables/knowledge/statement.rs`:

- Define private rkyv struct `StatementObjectBlob` with tagged-union
  layout: `discriminant: u8, entity_bytes: [u8; 16],
  value_kind: u8, value_blob: Vec<u8>` (Value variant payload encoded
  by `StatementValueBlob` sub-shim).
- Define `StatementValueBlob` for the `StatementValue` 6-variant
  enum. Same discriminant + payload pattern.
- Functions:
  - `pub fn encode_object(o: &StatementObject) -> Vec<u8>`.
  - `pub fn decode_object(bytes: &[u8]) -> Option<StatementObject>`.
  - `pub fn confidence_bucket(c: f32) -> u8` (`(c * 10.0).floor() as u8`, clamp 0..=10).
- `pub fn metadata_from_statement(s: &Statement) -> StatementMetadata` —
  fills every field. Computes `is_current` from `superseded_by /
  tombstoned`.
- `pub fn statement_from_metadata(m: &StatementMetadata) -> Statement`
  — including overflow evidence resolution from a passed-in closure
  (so the function stays I/O-free at this layer).

Tests: each `StatementObject` variant + each `StatementValue` variant
round-trips through encode/decode.

### Step 2 — `statement_ops.rs` skeleton

New file `crates/brain-metadata/src/statement_ops.rs`. Module-level docs
mirror `entity_ops`. Error enum:

```rust
#[derive(thiserror::Error, Debug)]
pub enum StatementOpError {
    #[error("redb storage error: {0}")]
    Storage(#[from] redb::StorageError),
    #[error("redb table error: {0}")]
    Table(#[from] redb::TableError),

    #[error("statement {0:?} not found")]
    NotFound(StatementId),
    #[error("statement {0:?} already exists")]
    AlreadyExists(StatementId),
    #[error("predicate {0} not registered")]
    UnknownPredicate(u32),
    #[error("subject {0:?} not registered")]
    UnknownSubject(EntityId),

    #[error("invalid argument: {0}")]
    InvalidArgument(&'static str),

    #[error("statement {0:?} already superseded by {1:?}")]
    AlreadySuperseded(StatementId, StatementId),
    #[error("statement {0:?} is tombstoned")]
    AlreadyTombstoned(StatementId),
    #[error("events cannot be superseded")]
    EventCannotSupersede,
    #[error("kind mismatch: old={old:?} new={new:?}")]
    KindMismatch { old: StatementKind, new: StatementKind },
    #[error("subject mismatch on supersede")]
    SubjectMismatch,
    #[error("predicate mismatch on supersede")]
    PredicateMismatch,
}
```

### Step 3 — `statement_create`

```rust
pub fn statement_create(
    wtxn: &WriteTransaction,
    s: &Statement,
    now_unix_nanos: u64,
) -> Result<StatementId, StatementOpError>;
```

Steps inside one redb txn:

1. **Validate** —
   - `id` not already in `STATEMENTS_TABLE`.
   - `subject.as_entity()` exists in `ENTITIES_TABLE` (use
     `entity_ops::entity_get`). `SubjectRef::Pending` rejected with
     `InvalidArgument("pending subjects deferred to phase 22 audits")`
     — phase 17 only handles resolved subjects.
   - `predicate` row exists in `PREDICATES_TABLE` (use
     `predicate_ops::predicate_get`). Validate against
     `kind_constraint` and `object_type_constraint_byte`.
   - Per-kind field invariants:
     - Fact / Preference: `event_at_unix_nanos` MUST be `None`.
     - Event: `event_at_unix_nanos` MUST be `Some`.
   - `valid_from <= valid_to` if both set.

2. **For Preference** — look up current Preference at
   `(subject_bytes, Preference as u8, predicate.raw(), 1)` in
   `STATEMENTS_BY_SUBJECT_TABLE`. If present → delegate to
   `statement_supersede(old_id, s, now)` and return its result.

3. **For Fact** — read-only contradiction probe:
   - Range-scan `STATEMENTS_BY_SUBJECT_TABLE` at
     `(subject, Fact as u8, predicate.raw(), 1)`.
   - If any has `object != s.object` → write a contradiction audit
     row (re-use `entity_resolution_audit`; phase 22 will add a
     dedicated table). v1.0: write a tracing event at WARN level +
     audit row.
   - Insert proceeds either way.

4. **Insert** — per §03 §2:
   - `STATEMENTS_TABLE.insert(s.id, metadata_from_statement(s))`.
   - `STATEMENTS_BY_SUBJECT_TABLE.insert(
        (subject, kind, predicate, 1), s.id_bytes)`.
   - `STATEMENTS_BY_PREDICATE_TABLE.insert(
        (predicate, kind, confidence_bucket(confidence)), s.id_bytes)`.
   - If `s.object` is `Entity(eid)`:
     `STATEMENTS_BY_OBJECT_ENTITY_TABLE.insert((eid_bytes, kind), s.id_bytes)`.
   - If `kind == Event`:
     `STATEMENTS_BY_EVENT_TIME_TABLE.insert(
        (event_at, subject), s.id_bytes)`.
   - For each `MemoryId` in `evidence.inline()`:
     `STATEMENTS_BY_EVIDENCE_TABLE.insert((mem_id, s.id_bytes), ())`.
   - `STATEMENT_CHAIN_TABLE.insert(
        (s.chain_root_bytes, s.version), s.id_bytes)`.

5. **Return** `Ok(s.id)`.

`EvidenceRef::Overflow` is handled via a follow-up
`statement_create_with_overflow` variant in step 3b below; phase 17.4
ships the inline path + the overflow-blob hook.

#### 3.1 Overflow path

Helper:

```rust
pub fn allocate_evidence_overflow(
    wtxn: &WriteTransaction,
    memory_ids: &[MemoryId],
    now_unix_nanos: u64,
) -> Result<EvidenceOverflowId, StatementOpError>;
```

Generates a fresh `EvidenceOverflowId` (UUIDv7), writes an
`EvidenceOverflow` row, returns the id. Caller assembles the
`EvidenceRef::Overflow(id)` and passes the resulting `Statement` to
`statement_create` — which then walks the overflow row to populate
`STATEMENTS_BY_EVIDENCE_TABLE`.

### Step 4 — `statement_get`

```rust
pub fn statement_get(
    rtxn: &ReadTransaction,
    id: StatementId,
) -> Result<Option<Statement>, StatementOpError>;
```

Point lookup + `statement_from_metadata` projection. If the row has
`evidence_overflow_id_bytes`, also loads the overflow row.

### Step 5 — `statement_supersede`

```rust
pub fn statement_supersede(
    wtxn: &WriteTransaction,
    old_id: StatementId,
    new_statement: &Statement,
    now_unix_nanos: u64,
) -> Result<StatementId, StatementOpError>;
```

Implements §01 §3 verbatim. Pre-conditions enumerated; mutation order:

1. Load old. Reject per `KindMismatch`, `SubjectMismatch`,
   `PredicateMismatch`, `AlreadySuperseded`, `AlreadyTombstoned`,
   `EventCannotSupersede`.
2. Compute `chain_root`:
   - If `old.supersedes.is_none()` → `chain_root = old.id`.
   - Else → `chain_root = old.chain_root` (already on row).
3. Build the new `StatementMetadata`:
   - `version = old.version + 1`.
   - `supersedes_bytes = Some(old.id_bytes)`.
   - `chain_root_bytes = chain_root_bytes`.
   - `superseded_by_bytes = None`.
   - `is_current = 1`.
4. Update old in place:
   - `superseded_by_bytes = Some(new.id_bytes)`.
   - If `old.kind != Event && old.valid_to_unix_nanos.is_none()`:
     `valid_to_unix_nanos = Some(new.extracted_at_unix_nanos)`.
   - `is_current = 0`.
   - Re-insert old in `STATEMENTS_BY_SUBJECT_TABLE` with `is_current=0`
     (remove old key, insert new key).
5. Run `statement_create` index inserts for new (already a fresh row).
6. Return `Ok(new.id)`.

### Step 6 — `statement_tombstone`

```rust
pub fn statement_tombstone(
    wtxn: &WriteTransaction,
    id: StatementId,
    reason: TombstoneReason,
    now_unix_nanos: u64,
) -> Result<(), StatementOpError>;
```

Soft delete. Sets:
- `tombstoned = 1`.
- `tombstoned_at_unix_nanos = Some(now)`.
- `tombstone_reason = reason.as_u8()`.
- `is_current = 0`.

Re-inserts in `STATEMENTS_BY_SUBJECT_TABLE` with flipped is_current bit.
Reverse-evidence index preserved per §03 §4.

Errors:
- `NotFound`.
- `AlreadyTombstoned` (idempotency check — re-tombstoning is a no-op,
  not an error; v1 returns Ok).

### Step 7 — `statement_retract`

Hard-delete intent; in 17.4 wraps `statement_tombstone` and writes an
audit marker with `RetractMarker::Pending` (phase 21+ worker reclaims).

```rust
pub fn statement_retract(
    wtxn: &WriteTransaction,
    id: StatementId,
    reason: TombstoneReason,
    now_unix_nanos: u64,
) -> Result<(), StatementOpError>;
```

### Step 8 — `statement_history`

```rust
pub fn statement_history(
    rtxn: &ReadTransaction,
    anchor: StatementId,
) -> Result<Vec<Statement>, StatementOpError>;
```

Per §01 §4. Resolve `chain_root`; prefix-scan
`STATEMENT_CHAIN_TABLE` at `(chain_root, *)`; load each row.
Statements returned in `version` ascending order.

### Step 9 — `statement_list`

```rust
pub struct StatementListFilter {
    pub subject: Option<EntityId>,
    pub predicate: Option<PredicateId>,
    pub kind: Option<StatementKind>,
    pub current_only: bool,
    pub min_confidence: Option<f32>,
    pub limit: usize,         // bounded; default cap 1000
}

pub fn statement_list(
    rtxn: &ReadTransaction,
    filter: &StatementListFilter,
) -> Result<Vec<Statement>, StatementOpError>;
```

Dispatch by filter shape:
- `subject + predicate + kind` set: `STATEMENTS_BY_SUBJECT_TABLE` point lookup.
- `subject` only: `STATEMENTS_BY_SUBJECT_TABLE` prefix scan.
- `predicate + kind`: `STATEMENTS_BY_PREDICATE_TABLE` prefix scan.
- `predicate` only: same with kind iter.
- No filters: scan `STATEMENTS_TABLE` capped by `limit`.

Apply `current_only` + `min_confidence` after the index lookup.

### Step 10 — `statements_contradicting`

Per §02 §3. Returns the contradicting set or `vec![]`.

```rust
pub fn statements_contradicting(
    rtxn: &ReadTransaction,
    subject: EntityId,
    predicate: PredicateId,
) -> Result<Vec<Statement>, StatementOpError>;
```

### Step 11 — Tests

Colocated unit tests in `statement_ops.rs` (cfg(test)):

- `create_fact_round_trips_via_get` — minimal happy path.
- `create_fact_writes_all_six_indexes` — assert each secondary index
  has the new id.
- `create_preference_auto_supersedes_prior` — second create on
  same `(subject, predicate)` yields a chain of length 2, old has
  `superseded_by`, new `is_current=1`.
- `create_event_requires_event_at` — `InvalidArgument` when None.
- `create_fact_with_event_at_is_rejected` — symmetric.
- `create_unknown_predicate_rejected` — `UnknownPredicate`.
- `create_unknown_subject_rejected` — `UnknownSubject`.
- `create_pending_subject_rejected_v1` — phase-22 deferral.
- `create_contradictory_facts_both_stored` — both ids reachable, both
  `is_current=1`, `statements_contradicting` returns both.
- `supersede_fact_chain_root_inherited` — second supersede inherits
  root from old, not from old.id.
- `supersede_preserves_explicit_valid_to` — caller-set valid_to wins
  over default-from-supersession.
- `supersede_event_rejected` — `EventCannotSupersede`.
- `supersede_kind_mismatch_rejected`.
- `supersede_subject_mismatch_rejected`.
- `tombstone_flips_is_current_bit` — `STATEMENTS_BY_SUBJECT_TABLE`
  lookup at `is_current=1` returns nothing; at `is_current=0`
  returns the row.
- `tombstone_preserves_evidence_index` — `STATEMENTS_BY_EVIDENCE_TABLE`
  still has the row.
- `retract_writes_audit_marker` — placeholder audit row visible.
- `history_walks_chain_in_version_order`.
- `history_works_from_any_chain_member`.
- `list_subject_predicate_returns_single_current`.
- `list_predicate_only_returns_all_kinds`.
- `list_with_min_confidence_filters`.
- `list_respects_limit_cap`.
- `contradicting_returns_two_when_disagreeing_facts`.
- `contradicting_empty_when_only_one_fact`.
- `evidence_overflow_round_trip` — > 8 evidence entries route through
  overflow row; primary still references via `evidence_overflow_id_bytes`.

Test helper: `fresh_db_with_entity_and_predicate(...)` to scaffold.

### Step 12 — Re-exports

`lib.rs`:

```rust
pub mod statement_ops;
pub use statement_ops::{
    allocate_evidence_overflow,
    statement_create, statement_get, statement_history, statement_list,
    statement_retract, statement_supersede, statement_tombstone,
    statements_contradicting,
    StatementListFilter, StatementOpError,
};
```

## Files written

| Path | Change |
|---|---|
| `crates/brain-metadata/src/tables/knowledge/statement.rs` | Add `StatementObjectBlob`, `StatementValueBlob`, `encode_object`, `decode_object`, `confidence_bucket`, `metadata_from_statement`, `statement_from_metadata`. |
| `crates/brain-metadata/src/statement_ops.rs` | New. 9 ops + error enum + filter struct + ~25 tests. |
| `crates/brain-metadata/src/lib.rs` | Re-exports. |

## Files NOT written this sub-task

- Statement HNSW (17.5).
- Wire structs (17.6).
- Handlers + events (17.7).
- SDK builders (17.8).
- `aggregate_confidence` from `evidence_inline` (17.9 — `statement_create`
  uses caller-supplied confidence in v1).
- Phase-21 GC worker for retract reclamation.
- Phase-22 audit table for contradictions (re-uses
  `entity_resolution_audit` for now).

## Verification gate

```
cargo zigbuild --target x86_64-unknown-linux-gnu -p brain-metadata --tests
cargo zigbuild --target x86_64-unknown-linux-gnu --workspace --tests
cargo clippy --target x86_64-unknown-linux-gnu -p brain-metadata --all-targets -- -D warnings
cargo test -p brain-core  (sanity — should still be 87 pass)
```

All clean before committing.

## Commit message draft

```
feat(brain-metadata): statement_ops module (17.4)

Layer-3 CRUD + supersession + contradiction surface, all single-redb-
txn. Mirrors entity_ops precedent.

- statement_create — 6 secondary index writes per spec §19/03 §2;
  auto-supersedes prior current Preference for same
  (subject, predicate); contradiction detector emits audit + WARN
  trace for Facts.
- statement_supersede — atomic two-step (write new + flip old
  is_current + valid_to inheritance) per §19/01 §3.
- statement_tombstone / _retract — soft-delete + GC marker.
- statement_history — chain traversal via STATEMENT_CHAIN_TABLE,
  resolves anchor as root or member per §19/01 §4.1.
- statement_list — filter dispatch picks the right index.
- statements_contradicting — read-only contradiction probe per §19/02.

Object encoding via private StatementObjectBlob in
tables/knowledge/statement.rs — keeps brain-core rkyv-free.

~25 unit tests cover happy path + every error variant + chain
traversal + evidence-overflow spill.

Plan: .claude/plans/phase-17-task-04.md.
```

## Risks

- **Lots of surface in one sub-task.** ~9 ops + ~25 tests. We
  estimated this would be the largest 17.x commit. If it grows
  unwieldy, split before commit into 17.4a (create/get + helpers) and
  17.4b (supersede/tombstone/retract/history/list/contradicting).
- **Cross-shard `by_object_entity` write** documented in §03 §9 as a
  best-effort cross-shard write — phase 17 implementation ships
  same-shard only with a `// TODO(phase 23): cross-shard write` note.
- **rkyv encoding of `StatementObject` introduces a new on-disk shape**
  but the `object_blob` field already exists as `Vec<u8>` in
  `StatementMetadata`, so this is a write-only addition.
- **Predicate validation is strict.** Statements with kind/object that
  violate the registered predicate's constraints get rejected with
  `InvalidArgument`. Users hitting this should re-register the
  predicate with looser constraints (or fix their data).
- **Auto-supersession recursion.** `statement_create` → `statement_supersede`
  → `statement_create`-style inserts. We unify by sharing a private
  `insert_new_statement` helper used by both paths, avoiding double
  validation.

## Out of scope (this sub-task)

- HNSW writes (17.5).
- Confidence aggregation (17.9).
- Wire surface (17.6 / 17.7).
- Cross-shard `by_object_entity` (phase 23).
- Discrete `STATEMENT_CONTRADICTED` event (phase 23, per §06 Q1).
- Multi-chunk overflow chains > 1000 evidence entries (phase 22,
  §06 Q6).
