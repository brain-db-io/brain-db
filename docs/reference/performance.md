# Performance baselines

Phase 13 (Benchmarks & Chaos) captures Brain's measured latency,
throughput, and recovery behaviour against the spec §14 targets.

## Targets

The contract Brain v1 must meet on reference hardware (16-core
x86_64, 64 GB RAM, NVMe, 1 M memories/shard):

- p99 RECALL (K=10, no text) ≤ 20 ms — `spec/19_benchmarks/02_performance_targets.md` §2.
- p99 ENCODE ≤ 25 ms.
- All other operations: see `spec/19_benchmarks/02_performance_targets.md` §2.
- Throughput: see `spec/19_benchmarks/02_performance_targets.md`.

## Running the benches

Per crate:

```bash
cargo bench -p brain-index           # recall, insert
cargo bench -p brain-storage         # crc32c
cargo bench -p brain-embed           # throughput
cargo bench -p brain-http            # router, sse_encoder, end_to_end
```

Phase 13.2's `load_generator` binary runs the SDK-driven end-to-end
mix:

```bash
cargo run --release --bin load_generator -- --rate 1000 --duration 5m
```

## Reporting

Each baseline run captures `criterion`'s default output (mean,
std-dev, min, max) plus the p50 / p95 / p99 / p99.9 quantiles
spec §02/02 reports. Results land in `baselines-<date>.md` files
under this directory.

Spec §02/13 expects ±10 % run-to-run variance. ±30 % indicates
instability; investigate before reporting numbers.

## Methodology

Per `spec/19_benchmarks/04_benchmark_methodology.md`:

- Quiet machine; no other tenants.
- Fixed CPU governor (performance).
- Disabled thermal throttling where possible.
- 5-minute warm-up before measurement.
- 10-minute measurement window.
- Multiple runs; report median + std dev.

Benches that ship in-tree (the `cargo bench -p <crate>` invocations
above) target component-level baselines. The end-to-end p99 numbers
spec §02/02 reports require Phase 13.2's load generator plus
production-sized data — those baselines land as `e2e-<date>.md`
files when the operator runs the rig.
