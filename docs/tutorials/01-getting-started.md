# Getting started with Brain v1.0

A 15-minute walk from a blank deployment to a working hybrid
query. You'll:

1. Install and start `brain-server`.
2. Encode three memories.
3. Run a substrate recall.
4. Declare a schema.
5. Run a query against the hybrid engine.

---

## 1. Install

Brain ships as a workspace of Rust crates. From a checkout:

```bash
cargo install --path crates/brain-server
cargo install --path crates/brain-cli
```

Or, on Linux (recommended — Glommio + io_uring), use a
cross-compiled binary from your `target/x86_64-unknown-linux-gnu`
directory after `cargo zigbuild --release`.

## 2. Start the server

```bash
brain-server --data-dir /tmp/brain-tutorial --listen 127.0.0.1:7332
```

Leave it running in one terminal. The data directory is created
on first start; it's empty until you encode.

## 3. Encode three memories

In another terminal:

```bash
brain-cli --server 127.0.0.1:7332 encode \
    "ticket ACME-1247 reproduces under heavy load"

brain-cli --server 127.0.0.1:7332 encode \
    "Priya merged the budget pushback decision on Friday"

brain-cli --server 127.0.0.1:7332 encode \
    "strawberry rhubarb cobbler recipe — bake at 375F"
```

Each call returns a `MemoryId`. The substrate stores the text
in the WAL, indexes the vector in HNSW, and indexes the text
in tantivy.

## 4. Substrate recall (no schema yet)

```bash
brain-cli --server 127.0.0.1:7332 recall "budget pushback"
```

The response is a list of `MemoryResult` rows with their
similarity scores. With no schema declared, `contributing_retrievers`
is empty and `fused_score` is `0.0` — this is the substrate's
pure-semantic recall path.

## 5. Declare a schema

```bash
cat > /tmp/my-schema.brain <<'EOF'
namespace acme

define entity_type Foo { attributes {} }
EOF

brain-cli --server 127.0.0.1:7332 schema upload \
    --file /tmp/my-schema.brain
```

The server's per-shard `SchemaGate` flips from `false` to
`true`. Substrate `RECALL_REQ` now routes through the hybrid
pipeline transparently (spec §28/08 §5).

## 6. Hybrid query

```bash
brain-cli --server 127.0.0.1:7332 query "budget pushback"
```

The response now includes per-hit `contributing_retrievers`
(`Semantic` for text-only queries by the auto router) and
non-zero `fused_score`. Same data, hybrid path.

You can also call `recall` and see the same hybrid metadata
on `MemoryResult` — the runbook in
[`docs/runbooks/schema-toggle.md`](../runbooks/schema-toggle.md)
walks through this and the backfill flow.

## What's next

- [Operator runbook: schema toggle](../runbooks/schema-toggle.md)
- [Spec overview](../../spec/00_master_overview/02_doc_map.md)
- [SDK reference (`brain-sdk-rust`)](../../crates/brain-sdk-rust)
- [Hybrid query design](../../spec/24_hybrid_query/00_purpose.md)

For a tour of the cognitive primitives (ENCODE / RECALL / PLAN /
REASON / FORGET) see
[`spec/09_cognitive_operations/`](../../spec/09_cognitive_operations/).
