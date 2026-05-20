#!/usr/bin/env bash
#
# Download BGE-small-en-v1.5 from HuggingFace into the XDG default
# location Brain looks for it. brain-server refuses to start without
# this model on disk; this script is the one-line bootstrap for new
# operators and dev containers.
#
# What it places where:
#
#   $XDG_DATA_HOME/brain/models/bge-small-en-v1.5/
#                          ├── config.json
#                          ├── tokenizer.json
#                          └── model.safetensors      (~130 MiB)
#
#   ($XDG_DATA_HOME defaults to $HOME/.local/share)
#
# Override the destination with $BRAIN_EMBED_MODEL_DIR if you need
# a different path (e.g. a shared volume in production). When that
# env is set, brain-server uses it verbatim — no XDG resolution.
#
# Spec: spec/04_embedding_layer/01_model_choice.md (BGE-small-en-v1.5)
# Spec: spec/04_embedding_layer/03_inference.md §9 (operator-downloaded,
#       not auto-downloaded by the substrate at runtime).
#
# Usage:
#   ./scripts/bootstrap-model.sh                  # default XDG path
#   BRAIN_EMBED_MODEL_DIR=/var/lib/brain/models/bge-small-en-v1.5 \
#       ./scripts/bootstrap-model.sh              # explicit path
#   ./scripts/bootstrap-model.sh --force          # re-download even if files exist
#   ./scripts/bootstrap-model.sh --verify         # check existing files; no download

set -euo pipefail

MODEL_REPO="BAAI/bge-small-en-v1.5"
MODEL_NAME="bge-small-en-v1.5"
HF_BASE="https://huggingface.co/${MODEL_REPO}/resolve/main"

# Required files. ModelHandle::load (brain-embed/src/model.rs:41-43)
# refuses to start without any of these and explicitly rejects the
# pickle variant for safety (per spec §04/03 §11).
FILES=(config.json tokenizer.json model.safetensors)

# ---------------------------------------------------------------------------
# Destination resolution. Mirrors EmbedderConfig::resolve_model_dir in
# crates/brain-server/src/config/mod.rs — keep these in sync if the
# resolution order changes.
# ---------------------------------------------------------------------------

if [[ -n "${BRAIN_EMBED_MODEL_DIR:-}" ]]; then
  DEST="${BRAIN_EMBED_MODEL_DIR}"
elif [[ -n "${XDG_DATA_HOME:-}" ]]; then
  DEST="${XDG_DATA_HOME}/brain/models/${MODEL_NAME}"
else
  DEST="${HOME}/.local/share/brain/models/${MODEL_NAME}"
fi

# ---------------------------------------------------------------------------
# Flags.
# ---------------------------------------------------------------------------

FORCE=0
VERIFY_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --force)  FORCE=1 ;;
    --verify) VERIFY_ONLY=1 ;;
    -h|--help)
      grep -E '^# ' "$0" | sed -e 's/^# //' -e 's/^#$//'
      exit 0
      ;;
    *)
      echo "error: unknown flag: $arg" >&2
      echo "use --help for usage" >&2
      exit 2
      ;;
  esac
done

# ---------------------------------------------------------------------------
# Verify mode: just check whether the files exist and exit.
# ---------------------------------------------------------------------------

if [[ "$VERIFY_ONLY" -eq 1 ]]; then
  echo "verifying model files in: ${DEST}"
  missing=()
  for f in "${FILES[@]}"; do
    if [[ -f "${DEST}/${f}" ]]; then
      echo "  ✓ ${f} ($(du -h "${DEST}/${f}" | cut -f1))"
    else
      echo "  ✗ ${f} (missing)"
      missing+=("$f")
    fi
  done
  if [[ ${#missing[@]} -eq 0 ]]; then
    echo
    echo "all required files present."
    exit 0
  else
    echo
    echo "missing ${#missing[@]} file(s); re-run without --verify to download."
    exit 1
  fi
fi

# ---------------------------------------------------------------------------
# Download.
# ---------------------------------------------------------------------------

# Prefer curl over wget — curl ships in Alpine and Debian-slim by
# default and behaves identically across distros.
if ! command -v curl >/dev/null 2>&1; then
  echo "error: curl is required but not found in PATH" >&2
  echo "install with: apt-get install curl  /  apk add curl  /  brew install curl" >&2
  exit 1
fi

mkdir -p "${DEST}"

echo "downloading ${MODEL_REPO} → ${DEST}"
echo

for f in "${FILES[@]}"; do
  target="${DEST}/${f}"
  if [[ -f "${target}" && "${FORCE}" -eq 0 ]]; then
    echo "  ✓ ${f} already present (use --force to re-download)"
    continue
  fi
  url="${HF_BASE}/${f}"
  echo "  → ${f}"
  # -L follows redirects (HuggingFace 302s to a CDN host).
  # -f makes curl exit non-zero on 4xx/5xx so we don't silently land
  #   an HTML error page in place of the file.
  # --retry handles transient CDN flakiness.
  if ! curl --fail --location --retry 3 --retry-delay 2 \
            --progress-bar \
            --output "${target}" "${url}"; then
    echo "error: download failed for ${f}" >&2
    echo "  url: ${url}" >&2
    echo "  target: ${target}" >&2
    rm -f "${target}"
    exit 1
  fi
done

# ---------------------------------------------------------------------------
# Post-check.
# ---------------------------------------------------------------------------

echo
echo "verifying download:"
for f in "${FILES[@]}"; do
  size=$(stat -c %s "${DEST}/${f}" 2>/dev/null || stat -f %z "${DEST}/${f}")
  echo "  ✓ ${f} (${size} bytes)"
done

# Sanity check: model.safetensors should be ~130 MiB. If it's tiny
# we likely landed an error page or a redirect HTML instead.
weight_size=$(stat -c %s "${DEST}/model.safetensors" 2>/dev/null || stat -f %z "${DEST}/model.safetensors")
if [[ "${weight_size}" -lt 10000000 ]]; then
  echo
  echo "warning: model.safetensors is only ${weight_size} bytes — expected ~130 MiB." >&2
  echo "the download likely failed silently. inspect ${DEST}/model.safetensors" >&2
  echo "and re-run with --force." >&2
  exit 1
fi

echo
echo "model ready. start brain-server:"
if [[ -n "${BRAIN_EMBED_MODEL_DIR:-}" ]]; then
  echo "  BRAIN_EMBED_MODEL_DIR=${BRAIN_EMBED_MODEL_DIR} ./target/release/brain-server"
else
  echo "  ./target/release/brain-server"
fi
