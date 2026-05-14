# Phase 13 — Benchmarks & Chaos

## Goal

Measure Brain against the spec's latency/throughput targets, then break
it on purpose to validate the recovery story. The output is a set of
reproducible baselines + a chaos harness that drives the Phase 14
acceptance gates.

## Prerequisites

- [ ] Phase 12 complete (`phase-12-complete` tag). Benchmarks read the
      metric counters that Phase 12 wires; chaos asserts use the same
      counters to verify recovery.

## Reading list

1. [`spec/16_benchmarks_acceptance/02_latency_targets.md`](../../spec/16_benchmarks_acceptance/02_latency_targets.md)
2. [`spec/16_benchmarks_acceptance/03_throughput_targets.md`](../../spec/16_benchmarks_acceptance/03_throughput_targets.md)
3. [`spec/16_benchmarks_acceptance/07_benchmark_methodology.md`](../../spec/16_benchmarks_acceptance/07_benchmark_methodology.md)
4. [`spec/15_failure_recovery/07_chaos_testing.md`](../../spec/15_failure_recovery/07_chaos_testing.md)

## Outputs

- Per-operation criterion benches in each runtime crate.
- `benches/load_generator.rs` — sustained-rate end-to-end load harness.
- `tests/chaos/` — kill-at-point, I/O fault, network failure, corruption
  injection scenarios.
- `tests/soak/` — 48 h continuous-load rig (run on dedicated infra; not CI).
- Performance report committed to `docs/performance/baselines-<date>.md`.
- Tag: `phase-13-complete`.

## Sub-tasks

### Task 13.1 — Per-operation criterion benches
**Reads:** spec §16/02, §16/03, §16/07.
**Writes:** `benches/*.rs` in `brain-storage`, `brain-index`, `brain-ops`, `brain-planner`, `brain-server` (one bench harness per crate; one benchmark per spec'd operation).
**Done when:** every cognitive operation has a criterion baseline; results table commits to `docs/performance/baselines-<date>.md`; spec §16 latency targets met on reference hardware.

### Task 13.2 — End-to-end load generator
**Writes:** `benches/load_generator.rs` (binary) — sustains a configurable rate of mixed encode / recall / link traffic over the SDK; reports p50/p95/p99 and per-op error rates.
**Done when:** generator hits spec §16/03 throughput targets without saturating CPU; emits a CSV summary suitable for diffing across runs.

### Task 13.3 — Chaos harness
**Reads:** spec §15/07.
**Writes:** `tests/chaos/{kill_during_wal_write,io_fault,network_partition,bit_flip,resource_exhaustion}.rs`.
**Done when:** each scenario reproduces the spec'd failure mode and asserts the spec'd recovery behaviour (no data loss, no silent corruption, fail-stop where mandated). Loom coverage for the select concurrency-critical paths flagged in §15/07.

### Task 13.4 — Soak rig
**Writes:** `tests/soak/{driver,asserts}.rs` — drives sustained mixed traffic for 48 h; samples memory, fd count, latency every 60 s; fails the run on memory leak / latency drift / error rate exceeding spec §16/04 thresholds.
**Done when:** soak completes one 48 h run on dedicated infra with no failures; results land in `docs/performance/soak-<date>.md`.

## Phase exit checklist

- [ ] Sub-tasks 13.1–13.4 complete.
- [ ] Performance baseline document committed.
- [ ] Chaos scenarios all green (or the failure mode is documented as a
      known limitation in the v1.0.0 release notes).
- [ ] One 48 h soak result recorded.
- [ ] Tag `phase-13-complete`.

## Notes

Benchmarks need a **quiet machine** — no other tenants, fixed CPU
governor, no thermal throttling. The methodology doc in spec §16/07
covers this; follow it precisely or the numbers are worthless.

Chaos tests intentionally bring the process down. Run them in a sandbox;
never against a real corpus.
