# 11.07 Plugin Surface

Two compile-time plugin traits let third parties extend Brain's extraction pipeline without forking the workspace. Plugins run on the writer's executor under strict isolation rules.

## 1. Why plugins exist

The pattern → classifier → LLM pipeline is opinionated. Real deployments need entry points for behavior that doesn't fit neatly into any of the three tiers:

- Surface normalization (emoji-to-text, casing, punctuation) that should happen before deduplication.
- External-system lookups (a CRM, a directory service) that enrich a candidate before extraction continues.

Both are out-of-scope for the core extractor traits — they don't produce structured Statements / Entities / Relations from text, they shape the input to those that do. Plugins are the right granularity.

## 2. The two traits

```rust
pub trait EnricherPlugin: Send + Sync {
    /// Mutates extractor candidates before dedupe.
    fn enrich(&self, candidates: &mut Vec<ExtractedItem>) -> Result<(), PluginError>;

    /// Stable identifier; appears in audit rows.
    fn id(&self) -> &'static str;
}

pub trait ConnectorPlugin: Send + Sync {
    /// Wraps the pre_filter / external fetch stage.
    fn fetch(&self, query: &ConnectorQuery) -> Result<ConnectorResult, PluginError>;

    fn id(&self) -> &'static str;
}
```

**EnricherPlugin** slots between the extractor's output and the dedupe stage. It can rewrite text fields, attach metadata, or drop candidates the plugin's heuristics say are noise. It cannot create new candidates — that's the extractor's job.

**ConnectorPlugin** wraps the pre-filter stage where Brain decides which memories to feed into the extractor on a given ENCODE. It can pull external context (a CRM record, a vector store lookup, a directory entry) that becomes part of the extractor's prompt.

## 3. Registration

Plugins are **compile-time** registered. There is no dynamic load path; a deployment that wants new plugins builds a new binary with the plugin crate added.

```rust
// In the binary's main, before shard startup:
brain_extractors::register_enricher(Box::new(EmojiNormalizer::new()));
brain_extractors::register_connector(Box::new(DirectoryConnector::new(...)));
```

The registration call is idempotent on plugin id — re-registering the same id is a no-op so test harnesses can re-register safely.

Why compile-time only:

1. Brain is a single-binary deployment. A dynamic-load path adds operational surface (versioning, ABI compatibility) without a use case to justify it.
2. The single-writer-per-shard invariant relies on knowing which code paths the writer executes; dynamic plugins make that opaque.

## 4. Execution surface

Plugins run on the **writer's executor only**. This is the per-shard single-writer thread that owns the redb / arena / HNSW for that shard.

The implication: plugins are `!Send` from outside the shard's perspective. A plugin that holds external connections (HTTP client, etc.) is fine — the connection lives on the writer's executor — but plugins MUST NOT spawn work that would need to land on the connection-layer Tokio runtime.

This is the same discipline every shard-local crate follows; the plugin trait inherits it.

## 5. Panic-safe dispatch

Every plugin invocation is wrapped in `std::panic::catch_unwind`. A panicking plugin:

1. Has its panic caught at the dispatch boundary.
2. Writes an audit row with `status = Failure` and `error = "plugin panic: <plugin_id>"`.
3. Returns control to the surrounding extractor as if the plugin had returned `Err(PluginError::Internal)`.

The surrounding ENCODE / extractor flow does NOT fail. A panicking plugin is isolated to its own audit row; the rest of the pipeline continues with whatever input the plugin was about to mutate, unmutated.

Why panic-safe and not panic-as-bug: plugins are third-party code. Brain's correctness guarantees don't extend to "a buggy third-party plugin won't crash a shard". Catching panics keeps the shard's other work alive while making the failure visible.

## 6. Error surface

```rust
pub enum PluginError {
    Skip(String),         // not an error; skip this candidate
    Internal(String),     // plugin failed; audit + continue
    Fatal(String),        // plugin is misconfigured; halt the registration
}
```

`Skip` is the common case for ConnectorPlugin — "this candidate doesn't need enrichment from me". It does not write an audit row.

`Internal` is the panic-catch's equivalent; the plugin's audit row records the message, the pipeline continues.

`Fatal` is the registration-time check — a plugin that returns `Fatal` from `enrich` / `fetch` fails the shard's startup. Reserved for misconfiguration (missing env var, unreachable external service the plugin requires).

## 7. Idempotency

Plugins must be idempotent on their inputs. A re-run with the same input must produce the same mutation (for EnricherPlugin) or the same fetched result (for ConnectorPlugin). Brain uses this when WAL replay re-runs the extractor pipeline: the plugin output must match the original or the divergence is treated as supersession.

The plugin trait does not enforce idempotency — it's a contract the plugin author signs. Tests should cover it.

## 8. Sample plugin: emoji-normalizer

A simple EnricherPlugin that demonstrates the surface:

```rust
pub struct EmojiNormalizer;

impl EnricherPlugin for EmojiNormalizer {
    fn enrich(&self, candidates: &mut Vec<ExtractedItem>) -> Result<(), PluginError> {
        for candidate in candidates.iter_mut() {
            if let Some(text) = candidate.surface_text_mut() {
                *text = replace_emoji_with_words(text);
            }
        }
        Ok(())
    }

    fn id(&self) -> &'static str { "brain.builtin.emoji_normalizer" }
}
```

The normalizer rewrites unicode emoji to their text equivalents (`👍` → `:thumbs_up:`) before the dedupe stage sees the candidate. Dedupe then compares the rewritten forms, catching duplicates that differ only by emoji rendering.

This plugin ships in v1.0 as the canonical example. It is not registered by default.

## 9. Observability

Per-plugin metrics:

- `plugin_invocations_total{plugin_id, outcome}` — counter (outcome ∈ ok, skip, internal, fatal, panic).
- `plugin_latency_seconds{plugin_id}` — histogram.
- `plugin_audit_writes_total{plugin_id, status}` — counter.

A plugin whose `panic` or `internal` count rises is an operational signal — the plugin is breaking or its external dependency is degraded.

## 10. Open questions

Future versions may add:

- Dynamic plugin loading (currently compile-time only).
- Plugin sandboxing (currently relies on Rust's memory safety + panic catching).
- A plugin marketplace / discovery mechanism.

None of these are blockers for v1.0; the compile-time surface is sufficient for the deployments that need plugins today.
