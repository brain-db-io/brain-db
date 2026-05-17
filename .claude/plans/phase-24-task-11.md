# Plan: Phase 24 — Task 11, Full acceptance suite

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1
**Depends on:** every prior 24.x sub-task; the substrate
                acceptance work from phase 14.

---

## 1. Scope

Operationalize the acceptance checklist in
`spec/31_complete_acceptance/00_purpose.md` as **a runnable
test suite** plus **a top-level acceptance-runner script**.

The bulk of the underlying tests already exist as
per-phase integration tests (16/17/18/19/20/21/22/23/24.10).
24.11 doesn't re-implement them — it orchestrates them.

Concrete deliverables:

1. **`crates/brain-server/tests/full_acceptance.rs`** (new)
   — a Rust integration test that asserts the **functional**
   checklist from §31 by chaining the per-phase fixtures
   into one long-running scenario. Each assertion ticks one
   `[ ]` line from §31. Sectioned per §31:
   - Schema operations
   - Entity operations
   - Statement operations
   - Relation operations
   - Extraction
   - Query
   - Provenance and versioning
2. **`scripts/full-acceptance.sh`** (new) — top-level
   orchestrator the operator runs at release time:
   ```bash
   set -euo pipefail
   cargo test --workspace --features acceptance
   bash scripts/schema-toggle-e2e.sh           # 24.10
   cargo bench --workspace -- --quick           # spot-check perf
   ./scripts/perf-acceptance.sh                 # 24.11.b
   ```
3. **`scripts/perf-acceptance.sh`** (new) — runs the criterion
   benches that map to §31 §"Performance acceptance",
   parses the JSON output, and asserts each P50/P99 number
   against the target. Reports pass/fail with a summary.
4. **`crates/brain-server/Cargo.toml`** —
   `[features] acceptance = []`-gated test entry for the
   slowest scenario (so default `cargo test` stays fast).
5. **`docs/acceptance/README.md`** (new) — operator-facing
   "how to run the acceptance suite" page that points at
   the script.

## 2. Spec references

- `spec/31_complete_acceptance/00_purpose.md` — entire file
  is the source of truth.
- `spec/16_benchmarks_acceptance/02_latency_targets.md` —
  perf targets the `perf-acceptance.sh` validates.

## 3. External validation

| Item | Source | Status |
|---|---|---|
| Phase-N integration tests | various | shipped per phase |
| Criterion bench JSON output | criterion 0.5 | supported |
| `brain-cli` orchestration | brain-cli | exists |
| Wire test harness | support_harness | shipped |

## 4. Architecture sketch

```
crates/brain-server/tests/full_acceptance.rs           (new)

#![cfg(all(target_os = "linux", feature = "acceptance"))]
mod support_harness;
use support_harness::{start_in, all_helpers::*};

#[tokio::test(flavor = "current_thread")]
async fn functional_acceptance() {
    let data_dir = TempDir::new().unwrap();
    let server = start_in(data_dir.path(), 1).await;
    let mut client = connect_and_handshake(&server).await;

    section_schema_ops(&mut client).await;        // ~8 assertions
    section_entity_ops(&mut client).await;        // ~10 assertions
    section_statement_ops(&mut client).await;     // ~8 assertions
    section_relation_ops(&mut client).await;      // ~6 assertions
    section_extraction(&mut client).await;        // ~6 assertions
    section_query(&mut client).await;             // ~7 assertions
    section_provenance(&mut client).await;        // ~5 assertions

    server.stop().await;
}

scripts/full-acceptance.sh                            (new)
#!/usr/bin/env bash
set -euo pipefail
echo "=== Brain v1.0 acceptance suite ==="
echo "[1/4] Workspace tests (substrate + knowledge)..."
cargo test --workspace --features acceptance
echo "[2/4] Schema-toggle e2e..."
bash scripts/schema-toggle-e2e.sh
echo "[3/4] Performance acceptance..."
bash scripts/perf-acceptance.sh
echo "[4/4] Spec link integrity..."
bash scripts/spec-link-check.sh    # exists from earlier docs polish
echo "ALL GREEN ✓"

scripts/perf-acceptance.sh                            (new)
#!/usr/bin/env bash
set -euo pipefail
# Runs the four bench targets we care about for acceptance.
# Each bench writes its results to target/criterion/.../base/estimates.json.
# We parse the JSON and assert P50/P99 against the §16/02 target.
for bench in lexical_retrieve hybrid_query encode_pipeline classifier_dispatch; do
    cargo bench -p <crate-for-$bench> --bench "$bench" -- --quick
done
# Parse with jq + a small awk filter; print one line per metric:
#   [✓] hybrid_three_retriever P50 8.2 ms (target ≤ 10 ms)
#   [✗] encode_pipeline P99 12 ms (target ≤ 10 ms)
# Exit 1 on any [✗].
```

### Sectioned assertion functions

Each `section_*` fn maps directly to a §31 sub-heading. The
fn body is a sequence of `// §31 §Schema operations §1` style
comments + the asserting calls — keeps the test self-
documenting against the spec.

### Performance targets

The `perf-acceptance.sh` script asserts at **10K corpus
scale** by default (same scale as the per-phase benches).
A `--full-scale` flag triggers 100K / 1M corpus rebuilds for
production-reference validation; that path is the phase-14
acceptance regime and runs separately.

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Single Rust test (this plan) | Atomic; one fixture | Long-running; fails fail-fast | ✓ |
| One Rust test per §31 sub-heading | Smaller blast radius | Setup duplicated; slower overall | rejected |
| Bash-only acceptance | Matches "how operators run it" | No fine-grained assertions; less Rust-friendly | both forms (Rust for assertions, Bash for orchestration) |
| Performance assertions in CI | Catches regressions automatically | Hardware-dependent flakiness | report-only by default; gate at `--strict` flag |
| Re-implement per-phase tests in this file | Self-contained | Massive duplication | leverage existing tests via `cargo test --features acceptance` filter |

## 6. Risks / open questions

- **Risk:** Performance asserts flake on slower hardware. **Mitigation:** the default is report-only; `--strict` gates against the §2.10 targets only on the reference hardware (Linux + the dev workstation specs).
- **Risk:** The Rust test takes 5+ minutes. **Mitigation:** gate behind `[features] acceptance` so default `cargo test` stays fast; CI runs the gated variant separately.
- **Open question:** Do we need to validate the documentation-acceptance bullets (§31 last section) in this test? **Resolution:** doc bullets are validated by 24.12 (documentation polish) via link-check + lints. This sub-task focuses on functional + perf.

## 7. Test plan

24.11 IS the test. Validated by:

- `cargo test -p brain-server --test full_acceptance --features acceptance` passes.
- `bash scripts/full-acceptance.sh` exits 0 on the reference Linux workstation.

## 8. Commit shape

```
test(server,scripts,docs): 24.11 — full acceptance suite

- crates/brain-server/tests/full_acceptance.rs (new): single
  long-running scenario chaining the §31 checklist; feature-
  gated `acceptance` to keep default `cargo test` fast.
- crates/brain-server/tests/support_harness/all_helpers.rs
  (new): per-section helper fns (section_schema_ops,
  section_entity_ops, ...).
- crates/brain-server/Cargo.toml: [features] acceptance = [].
- scripts/full-acceptance.sh (new): top-level operator script.
- scripts/perf-acceptance.sh (new): perf-target parser
  against criterion JSON output.
- docs/acceptance/README.md (new): operator how-to.

Verified: cargo zigbuild --target x86_64-unknown-linux-gnu
-p brain-server --tests --features acceptance;
cargo clippy -- -D warnings;
bash scripts/full-acceptance.sh (on reference Linux).
```

## 9. Confirmation

1. **One Rust test, sectioned** by §31 sub-heading; feature-gated `acceptance` to keep default test runs fast.
2. **Two helper scripts** (`full-acceptance.sh` orchestrator, `perf-acceptance.sh` perf gate); the orchestrator chains workspace tests + the schema-toggle e2e + perf.
3. **Perf assertions report-only by default**; `--strict` mode gates against §16/02 §2.10.
4. **Documentation acceptance lives in 24.12** (link checks + lints).
5. **Reference hardware**: Linux + dev workstation specs.
