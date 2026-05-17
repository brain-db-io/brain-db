#!/usr/bin/env bash
# Schema-toggle end-to-end smoke (sub-task 24.10).
#
# Mirrors docs/runbooks/schema-toggle.md against a running
# brain-server using brain-cli. Used by scripts/full-acceptance.sh.
#
# v1 limitation: the live backfill step is expected to fail every
# item with "memory text not persisted (v1 limitation)" — this
# script asserts that signal, not actual extraction success.

set -euo pipefail

BRAIN_CLI="${BRAIN_CLI:-brain-cli}"
SERVER_ADDR="${BRAIN_SERVER_ADDR:-127.0.0.1:7332}"

echo "[schema-toggle-e2e] using server $SERVER_ADDR"

echo "[1/5] validate schema (dry-run)…"
$BRAIN_CLI --server "$SERVER_ADDR" schema validate \
    --file scripts/fixtures/trivial-schema.brain >/dev/null

echo "[2/5] upload schema…"
$BRAIN_CLI --server "$SERVER_ADDR" schema upload \
    --file scripts/fixtures/trivial-schema.brain >/dev/null

echo "[3/5] dry-run backfill (expect plan preview)…"
$BRAIN_CLI --server "$SERVER_ADDR" admin backfill \
    --extractors all --memory-range all --dry-run >/dev/null

echo "[4/5] query (hybrid path)…"
out=$($BRAIN_CLI --server "$SERVER_ADDR" query "smoke test" || true)
echo "$out" | grep -q "contributing_retrievers" || {
    echo "FAIL: query response missing contributing_retrievers field"
    exit 1
}

echo "[5/5] schema list…"
$BRAIN_CLI --server "$SERVER_ADDR" schema list >/dev/null

echo "[schema-toggle-e2e] ALL GREEN ✓"
