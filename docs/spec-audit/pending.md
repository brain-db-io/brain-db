# Pending audits — Tier-B / C worklist

The 14 spec sections not audited at depth in Phase 15.1-15.4
(§00, §01, §02, §04, §06, §07, §08, §09, §10, §11, §12, §13, §15,
§16). Each has a row below with the owning crate(s), MUST-count
heuristic, and a recommended audit priority for the operator to
drive subsequent passes.

The methodology is documented in
[`README.md`](README.md) and
[`../../.claude/plans/phase-15-spec-audit.md`](../../.claude/plans/phase-15-spec-audit.md).

## Priority rubric

- **P1** — touches the SemVer-stable v1.0 wire / data surface.
  An undocumented divergence here is a release blocker.
- **P2** — touches durability, correctness, or recovery
  semantics. Drift would break operator expectations.
- **P3** — fully internal; unit-tested at depth; drift would
  surface as a perf regression or a worker bug, not silent
  data loss.

## Worklist

### §00 Master overview — `P3`

- **Files:** 5
- **Owning code:** N/A (cross-cutting)
- **MUST count (raw):** 9
- **Audit hint:** This section indexes the rest. Most of its
  "MUSTs" point at other sections' contracts. Audit by
  checking the [`02_doc_map.md`](../../spec/00_master_overview/02_doc_map.md)
  pointers and the seven invariants in
  [`05_invariants.md`](../../spec/00_master_overview/05_invariants.md)
  → CLAUDE.md §5 already mirrors these.
- **Expected drift:** zero.

### §01 System architecture — `P2`

- **Files:** 11
- **Owning code:** all crates (system-wide layout)
- **MUST count (raw):** 35
- **Audit hint:** Verify the layer separation (L1 connection /
  L2 dispatch / L3 ops / L4 storage), the per-shard executor
  model, and the Tokio↔Glommio boundary primitive (flume).
  Cross-reference `brain-glommio-rules` + `brain-tokio-boundary`
  skills.
- **Expected drift:** zero; the architecture is well-defended by
  the type system (`!Send` types stay shard-local).

### §02 Data model — `P1`

- **Files:** 11
- **Owning code:** `brain-core`, `brain-protocol`
- **MUST count (raw):** 26
- **Audit hint:** `MemoryId` bit layout, `EdgeKind` enum,
  `MemoryKind` enum, salience semantics. Wire-visible — release
  blocker if anything has drifted.
- **Expected drift:** zero; verified by `brain-protocol-version-bump`
  skill on every PR.

### §04 Embedding layer — `P2`

- **Files:** 11
- **Owning code:** `brain-embed`
- **MUST count (raw):** 16
- **Audit hint:** BGE-small-en-v1.5 fingerprint, L2 normalization
  invariant, cache hit-rate spec, batch-window timing. Two SDs
  already recorded
  ([SD-04.1-1 pytorch refusal, SD-04.2-1 safetensors load](../spec-deviations.md)).
- **Expected drift:** zero; SDs cover the known divergences.

### §06 ANN index (HNSW) — `P2`

- **Files:** 11
- **Owning code:** `brain-index`
- **MUST count (raw):** 19
- **Audit hint:** HNSW parameters (M=16, ef_construction=200,
  ef_search=64), tombstone semantics, recall ≥ 0.95 at default
  params (spec §16/05). Three SDs already recorded
  (`SD-4.7-1..3`).
- **Expected drift:** zero; SDs cover the known divergences.
  Snapshot file layout is the main place to double-check.

### §07 Metadata + graph (redb) — `P2`

- **Files:** 12
- **Owning code:** `brain-metadata`
- **MUST count (raw):** 13
- **Audit hint:** 13 redb tables per spec §07/02, idempotency
  table TTL, graph edge cardinality, schema migration tracker.
  Two SDs recorded
  ([SD-07.2-1 IdempotencyEntry request_hash field](../spec-deviations.md),
   [SD-07.x-1 MetadataSink::apply signature](../spec-deviations.md)).
- **Expected drift:** zero.

### §08 Query planner — `P3`

- **Files:** 12
- **Owning code:** `brain-planner`
- **MUST count (raw):** 10
- **Audit hint:** Plan / Reason execution paths, depth limits,
  budget enforcement (max_steps / max_time / max_branches),
  cross-shard fan-out (deferred to v2 — should carry tracker).
- **Expected drift:** low; planner is well-unit-tested but the
  multi-step paths might surface drift around budget edge cases.

### §09 Cognitive operations — `P1`

- **Files:** 16
- **Owning code:** `brain-ops`
- **MUST count (raw):** 17
- **Audit hint:** Each opcode's semantic contract. ENCODE
  (idempotency, dedup, salience), RECALL (filter semantics,
  result ordering), FORGET (soft/hard, grace), LINK/UNLINK
  (edge type table), TXN_* (atomicity), SUBSCRIBE (event
  filtering). Wire-visible — every clause is part of the SDK
  contract.
- **Expected drift:** low; verified by the SDK e2e suite + 80
  feature-parity tests per spec §16/08 §6. **Suggest auditing
  next** — this is the next-highest-leverage section after the
  three already done.

### §10 Concurrency + epochs — `P2`

- **Files:** 11
- **Owning code:** all runtime crates
- **MUST count (raw):** 25
- **Audit hint:** Single-writer-per-shard discipline,
  `ArcSwap` for reader-side state, `crossbeam-epoch` GC.
  One SD recorded
  ([SD-10.x-1 `crossbeam-epoch` unused in v1](../spec-deviations.md)).
  Cross-reference Inv-2 in [`s05-storage.md`](s05-storage.md).
- **Expected drift:** zero on the runtime side; the epoch GC
  story may need adjustment in the SD if the impl never lands
  the primitive.

### §11 Background workers — `P3`

- **Files:** 17
- **Owning code:** `brain-workers`
- **MUST count (raw):** 5
- **Audit hint:** 12 workers per shard; spec §11/03 lists each
  worker's responsibility. Verify intervals, retry policy,
  metric emission. Coverage: `brain_worker_*` family.
- **Expected drift:** zero; workers are well-isolated and
  unit-tested.

### §12 Sharding + clustering — `P3` (v1)

- **Files:** 11
- **Owning code:** `brain-server::shard` + routing
- **MUST count (raw):** 10
- **Audit hint:** v1 is single-host per shard. The clustering
  surface (v2) is mostly TBD — confirm spec sections marked v2
  aren't accidentally invoked. RoutingTable hot-reload via
  ArcSwap is the live surface.
- **Expected drift:** zero for v1 scope.

### §13 SDK design — `P1`

- **Files:** 12
- **Owning code:** `brain-sdk-rust`
- **MUST count (raw):** 11
- **Audit hint:** Builder API shape, connection pool semantics,
  retry policy (exponential backoff + jitter), streaming iterator
  semantics, idempotency / RequestId generation. Wire-visible
  via observable client behavior.
- **Expected drift:** low; the SDK is well-tested but
  cross-SDK compat (when Python/Node SDKs land) will need an
  audit pass to confirm consistent behavior.

### §15 Failure recovery — `P2`

- **Files:** 12
- **Owning code:** `brain-storage::recovery` + chaos tests
- **MUST count (raw):** 11
- **Audit hint:** Recovery procedures per failure mode (torn
  WAL, missing segment, bit flip, sink failure). The Phase 13.3
  chaos suite (`random_kill`, `bit_flip`, `io_fault`) covers
  these; audit confirms test coverage matches spec §15's
  enumeration of failure modes.
- **Expected drift:** zero — Inv-7 already audited in
  [`s05-storage.md`](s05-storage.md).

### §16 Benchmarks + acceptance — `P1` (gate for release)

- **Files:** 12
- **Owning code:** `benches/` per crate + `scripts/acceptance/run.sh`
- **MUST count (raw):** 47
- **Audit hint:** Latency targets per op (§02), throughput targets
  (§03), resource budgets (§04), recall quality (§05), durability
  criteria (§06), acceptance gates 1-10 (§08). Most MUSTs are
  measurable; the audit cross-references benches + the operator's
  baselines doc.
- **Expected drift:** depends on the operator's reference-hardware
  run. CI gates compile-check the rigs; the real measurement is
  operator-side per [`scripts/acceptance/README.md`](../../scripts/acceptance/README.md).

## Recommended audit order

For the operator picking this up after release:

1. **§09 Cognitive operations** — next-highest-leverage; user-visible
   contract.
2. **§02 Data model** — wire-visible primitives.
3. **§13 SDK design** — operator-visible (especially when
   non-Rust SDKs land).
4. **§16 Benchmarks + acceptance** — driven by the reference-
   hardware run anyway.
5. **§15 Failure recovery** — sanity-check the chaos suite covers
   §15's enumeration.
6. Everything else as cadence permits.

Each audit follows the template in
[`README.md`](README.md) §"How to add an audit".
