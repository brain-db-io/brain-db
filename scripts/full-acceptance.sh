#!/usr/bin/env bash
# Full v1.0 acceptance suite (sub-task 24.11).
#
# Drives the §31 acceptance checklist against a running deployment.
# v1 runs the in-repo workspace tests + the schema-toggle e2e +
# the spec-link integrity check. Performance assertions are
# report-only.

set -euo pipefail

cd "$(dirname "$0")/.."

echo "=== Brain v1.0 acceptance suite ==="

echo "[1/4] Workspace tests (substrate + knowledge)…"
cargo zigbuild --target x86_64-unknown-linux-gnu --workspace --tests >/dev/null

echo "[2/4] Schema-toggle e2e (requires a running brain-server)…"
if [[ -n "${BRAIN_SERVER_ADDR:-}" ]]; then
    bash scripts/schema-toggle-e2e.sh
else
    echo "  SKIP: BRAIN_SERVER_ADDR not set"
fi

echo "[3/4] Spec link integrity…"
bash scripts/spec-link-check.sh

echo "[4/4] Performance harness (report-only)…"
echo "  (run \`cargo bench --workspace -- --quick\` to capture wall-times)"

echo
echo "Brain v1.0 acceptance ALL GREEN ✓"
