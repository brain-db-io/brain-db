# Embedding model — install + operate

> Substrate-owned embedding model install. Referenced from
> [`phase-05-embedding.md`](../development/phases/phase-05-embedding.md) §3.

## What gets installed

Brain's substrate owns its embedding model. Clients send text;
the substrate runs the model and stores a 384-dim L2-normalised
`f32` vector per memory. The default v1 model is **BGE-small-en-v1.5**
(384 dim, BERT-shaped, ~130 MiB on disk), chosen per
[`spec/04_embedding_layer/01_model_choice.md`](../../spec/04_embedding_layer/01_model_choice.md).

The substrate **does not auto-download the model at runtime** — that's
an operator concern, locked in by
[`spec/04_embedding_layer/03_inference.md`](../../spec/04_embedding_layer/03_inference.md)
§9. If the model directory is missing or incomplete, `brain-server`
refuses to start with a pointer at this document.

## Quick start

The repo ships `scripts/bootstrap-model.sh` that downloads BGE-small
from HuggingFace into the default XDG location:

```bash
./scripts/bootstrap-model.sh
```

After it finishes, start the server:

```bash
./target/release/brain-server
```

If you see `model ready. start brain-server:` the install worked. Go.

## Where the model lives

`brain-server` looks for the model directory in this order:

1. **`$BRAIN_EMBED_MODEL_DIR`** — explicit override, must be an
   absolute path. Use this in production to pin a known location
   (e.g., a shared volume or a container's mount path).
2. **An absolute path in `config.embedder.model`** — if the value
   in your config TOML starts with `/`, the substrate treats it as
   a literal filesystem path. Useful when you want the location
   baked into config rather than env.
3. **`$XDG_DATA_HOME/brain/models/<model_name>`** — the XDG default.
4. **`$HOME/.local/share/brain/models/<model_name>`** — fallback when
   `$XDG_DATA_HOME` is unset.

`<model_name>` is whatever the config has under `[embedder] model =
"…"`. The default in `config/dev.toml` and `config/docker.toml` is
`bge-small-en-v1.5`, so the default landing directory is
`~/.local/share/brain/models/bge-small-en-v1.5/`.

The directory must contain exactly these three files:

| File | What it is | Approx size |
|---|---|---|
| `config.json` | BERT architecture spec (layers, dim, attention) | ~600 bytes |
| `tokenizer.json` | HuggingFace tokenizer state (vocab, special tokens) | ~700 KB |
| `model.safetensors` | The BGE-small weights | ~130 MiB |

Brain refuses to load the legacy PyTorch `pytorch_model.bin` format
(spec §04/03 §11 — pickle is unsafe and we tighten beyond the spec
to refuse outright). Use the safetensors variant only.

## Manual install (no script)

If you'd rather not run the bootstrap script — air-gapped install,
mirror downloads, immutable image build, etc. — the three files come
from HuggingFace:

```bash
DEST="${XDG_DATA_HOME:-$HOME/.local/share}/brain/models/bge-small-en-v1.5"
mkdir -p "$DEST"
cd "$DEST"

curl -fL -o config.json \
  https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/config.json

curl -fL -o tokenizer.json \
  https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/tokenizer.json

curl -fL -o model.safetensors \
  https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/model.safetensors
```

Or, if you already use `huggingface_hub`:

```python
from huggingface_hub import snapshot_download
snapshot_download(
    'BAAI/bge-small-en-v1.5',
    local_dir='$XDG_DATA_HOME/brain/models/bge-small-en-v1.5',
)
```

## Verify an existing install

```bash
./scripts/bootstrap-model.sh --verify
```

Reports each file's presence + size and exits non-zero if any are
missing. Useful in a container's healthcheck or a CI pre-flight.

## Re-download / upgrade

```bash
./scripts/bootstrap-model.sh --force
```

Re-downloads everything. The model **fingerprint** (BLAKE3 of the
config + tokenizer + weights, per [`spec/04_embedding_layer/07_fingerprinting.md`](../../spec/04_embedding_layer/07_fingerprinting.md))
changes any time the bytes change. Every memory stored before the
change keeps its old fingerprint, and `RECALL`'s default
`fingerprint_match: true` filter excludes them from cross-model
queries — by design, since cross-model cosine similarity is
meaningless. To migrate stored memories across a model change, see
[`docs/runbooks/op-07-embedder-model-upgrade.md`](../runbooks/op-07-embedder-model-upgrade.md).

## File checksums

The bytes shipping today are pinned by their BLAKE3 hashes. If you
fetched the files manually and want to confirm you got the same
ones we tested against:

```bash
b3sum config.json tokenizer.json model.safetensors
```

(`brain-embed` ships `blake3_hash_file()` for the same computation
done in-process; you can also call `cargo run -p brain-cli --
embed-fingerprint <dir>` once that admin command lands.)

We don't ship the expected hashes here because they're tied to
whatever HuggingFace serves under
`BAAI/bge-small-en-v1.5/resolve/main/` — if HF updates the model
in place (rare but possible), the hashes shift. The fingerprint
computed at server startup is the source of truth and is logged at
`INFO` level the first time `brain-server` loads.

## Troubleshooting

**`brain-server` exits with `missing required model file:
<path>/config.json`.**

The directory exists but the files don't. Re-run
`./scripts/bootstrap-model.sh` (or check that `$BRAIN_EMBED_MODEL_DIR`
points at the right place, if you set it).

**`brain-server` exits with `load model: …`.**

The files are there but BertModel rejected one of them. Two common
causes:
- `model.safetensors` was truncated by an interrupted download.
  `--force` re-downloads.
- The model directory has `pytorch_model.bin` instead of (or in
  addition to) `model.safetensors`. Brain rejects the pickle path
  outright; delete the `.bin` file.

**`brain-server` first encode is slow (~5 s).**

The model loads lazily on first encode if startup-eager-load is off
(it's on by default — see `EmbedderConfig::warmup_iters` defaulting
to 3). If you're seeing slow first-encode, check the startup log
for `loaded embedding model fingerprint=…`. If that line never
appeared, the lazy-load is firing on demand.

**`brain-server` shows `embedder fingerprint = 00000000…` in
`encode -o wide`.**

The model failed to load and `brain-server` is using a stub
dispatcher (development builds only — production refuses to start).
Check the server log for the load error.

## Related

- [`spec/04_embedding_layer/`](../../spec/04_embedding_layer/) —
  full embedding-layer spec.
- [`docs/architecture/06-embedding-pipeline.md`](../architecture/06-embedding-pipeline.md) —
  how the substrate uses the model at encode + recall time.
- [`docs/runbooks/op-07-embedder-model-upgrade.md`](../runbooks/op-07-embedder-model-upgrade.md) —
  upgrading the model in a live deployment.
- [`docs/development/usage/07-configuration.md`](../development/usage/07-configuration.md) —
  `[embedder]` config section reference.
