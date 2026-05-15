# Brain acceptance suite

Spec §16/08 §19 defines 10 release gates. `run.sh` runs them and
reports pass/fail per gate.

## The gates

| Gate | What | Spec § | Owner |
|---|---|---|---|
| 1 | Unit tests | §16/08 §2 | CI |
| 2 | Integration tests | §16/08 §3 | CI |
| 3 | End-to-end tests | §16/08 §4 | CI |
| 4 | Smoke test | §16/08 §5 | CI |
| 5 | Performance targets | §16/08 §12 | Operator (quiet hardware) |
| 6 | Chaos tests | §16/08 §13 | CI (subset); operator (network/resource) |
| 7 | Soak test (48 h) | §16/08 §14 | Operator (dedicated infra) |
| 8 | Compliance tests | §16/08 §15 | CI (TLS path); operator (audit log) |
| 9 | Documentation tests | §16/08 §17 | CI (`cargo doc`) |
| 10 | Security tests (fuzz) | §16/08 §18 | CI (60s); operator (overnight) |

CI gates 1-4, 6 (partial), 8 (partial), 9, 10 (smoke) run on every
PR. The operator-side gates (5 full bench, 7 soak, 6/8 full chaos +
compliance) run on dedicated infrastructure before release tagging.

## Running

```bash
# All gates (mix of CI and operator-side; some will be no-ops without infra):
bash acceptance/run.sh

# Selected gates only:
bash acceptance/run.sh 1 2 3 4

# All except slow ones:
bash acceptance/run.sh --skip 5 7

# Per-gate report path (default acceptance/last-run.jsonl):
BRAIN_ACCEPT_REPORT=/tmp/acceptance.jsonl bash acceptance/run.sh
```

Exit 0 iff every selected gate passes. The JSONL report is one
`{"gate": N, "status": "pass|fail|skipped", "reason": "..."}`
record per gate.

## Per-gate detail

### Gate 1 — Unit tests

`just docker-test --workspace` — every crate's in-source `#[test]`
suite. ~70 % line coverage target on substrate code per spec §16/08 §2.

### Gate 2 — Integration tests

Cross-module tests in `crates/*/tests/`. The runner exercises
brain-server's `e2e`, `sdk_e2e`, and `admin` test binaries.

### Gate 3 — End-to-end tests

Full substrate via wire protocol. Re-runs `sdk_e2e` + `cli_e2e`.

### Gate 4 — Smoke test

The one-encode-recall round-trip per spec §16/08 §5. The runner
re-runs `sdk_e2e::sdk_encode_recall_roundtrip` as a fast (~5 s)
sanity check.

### Gate 5 — Performance targets

`cargo bench --workspace --no-run` in CI (compile-only). A full
release run replaces this with `cargo bench` on quiet hardware
plus a comparison against `docs/performance/baselines-<date>.md`.

### Gate 6 — Chaos tests

`random_kill`, `bit_flip`, `io_fault` — the storage-layer scenarios
from Phase 13.3. Network-partition / resource-exhaustion / time-
anomaly chaos requires cgroups/tc/clock-mock and is operator-side.

### Gate 7 — Soak test

CI compile-checks `crates/brain-sdk-rust/examples/soak.rs`. The 48 h
run is operator-side; the result file lands at
`docs/performance/soak-<date>.md`.

### Gate 8 — Compliance tests

The TLS code path is exercised by the existing `admin` test
binary. The full audit-log integrity check is deferred — the audit
log primitive itself is on the Phase 12 deferred list (tracker
`phase-11/audit-log`).

### Gate 9 — Documentation tests

`cargo doc --workspace --no-deps` must build warning-free. Doctests
that ship in the workspace are validated as part of `docker-test`.

### Gate 10 — Security tests

CI builds the brain-protocol fuzz targets. A release run goes
overnight; results land in `docs/security/fuzz-<date>.md`.

## Adding a gate

Each gate is a shell function in `run.sh` (`gate_<name>`). Adding
one means:

1. Add the gate id to `ALL_GATES`.
2. Add a `gate_<name>` function whose exit code is the gate result.
3. Wire `case "$gate"` to call it.
4. Update the table in this README.

Gate failures must surface why — pipe through `tail -5` or `tail -3`
so the runner's per-gate output is informative.
