# 11.06 Prompt Caching

How Brain's LLM tier uses provider-side prompt caching to keep cost and latency bounded. Applies to every LLM call that ships through `brain-llm` — the extractor tier and the supersession judge alike.

## Why

Each LLM call from the extractor tier carries a substantial system prompt — role description, output schema, extraction rules. The per-call query body is small relative to that header. Without caching, every call re-sends and re-tokenises the header.

Anthropic and OpenAI both offer **prompt caching** that lets the provider keep a tokenised prefix warm. Steady-state, the per-call cost drops to (cached header read) + (new query body) — typically a 30-90% reduction in input-token cost on long-prompt workloads. Latency drops too, since the provider skips re-encoding the prefix.

## How it's structured

Each LLM call's prompt splits into three blocks. The first two are cacheable; the third is per-call.

| Block | Content | Cached? |
|---|---|---|
| Role block | Persona + decision protocol + behavior rules | yes (`cache_control: ephemeral`) |
| Schema block | JSON-Schema for the typed response | yes (`cache_control: ephemeral`) |
| Query block | Memory text + top-m neighbors + rolling summary | no |

The role + schema blocks are byte-stable for a given extractor version. The provider's cache key is the prefix hash; as long as the role + schema haven't changed, every subsequent call within the cache TTL reads from the warm prefix.

## API shape (Anthropic)

```jsonc
{
  "system": [
    { "type": "text", "text": "<role block>", "cache_control": { "type": "ephemeral" } },
    { "type": "text", "text": "<schema block>", "cache_control": { "type": "ephemeral" } }
  ],
  "messages": [
    { "role": "user", "content": "<query block>" }
  ]
}
```

The two `cache_control: ephemeral` blocks tell Anthropic to keep the prefix in the ephemeral cache (5-minute TTL, refreshed on each hit). The query block is regular content and is not cached.

OpenAI's equivalent uses automatic prompt-prefix caching — no explicit `cache_control` marker — and Brain treats both providers uniformly through the `LlmClient` abstraction.

## Cache TTL behavior

The ephemeral cache TTL is **provider-controlled** (5 minutes for Anthropic at time of writing). Each cache hit refreshes the TTL, so a steady stream of extractor calls keeps the prefix warm indefinitely.

A cold start — first call after a TTL expiry — pays the full prefix tokenisation cost plus a small cache-write overhead. Subsequent calls within the TTL window read from cache.

## Steady-state targets

For a deployment running real extraction traffic:

| Metric | Target |
|---|---|
| Cache hit ratio (cache_read / total input tokens) | ≥ 0.7 |
| First-call cost (cache miss) | ~1.0× baseline |
| Steady-state cost (cache hit) | ~0.1-0.3× baseline |

The 0.7 floor reflects that some calls always pay the cold-start tax — first call per shard after a restart, first call after a TTL expiry on a low-volume shard, the first call after a prompt edit (which invalidates the cache).

## Metrics

`brain-llm`'s response handling captures the cache stats the provider returns:

- `cache_creation_input_tokens` — tokens written to cache on this call (typically a multiple of the role + schema block size, billed at the cache-write rate).
- `cache_read_input_tokens` — tokens read from cache on this call (billed at the cache-read rate, typically 10× cheaper than fresh input).

These land in the `ExtractionAudit.model_metadata` rkyv blob so operators can audit cache health per extractor.

Per-shard rollup metrics:

- `llm_cache_read_input_tokens_total{extractor_id}` — counter.
- `llm_cache_creation_input_tokens_total{extractor_id}` — counter.
- `llm_input_tokens_total{extractor_id}` — counter (non-cache input).

Cache hit ratio = `cache_read / (cache_read + cache_creation + non_cache_input)`. The dashboard panel surfaces it per-extractor; an extractor whose ratio dips below 0.5 is signaling either prompt churn or sub-TTL call frequency.

## Invalidation triggers

A change to either cacheable block evicts the prefix:

1. Edit the role block in the extractor's schema → next call is a cache miss.
2. Edit the schema block (output shape changes) → next call is a cache miss.
3. Provider-side cache eviction (TTL expiry, capacity, provider deploys) → next call is a cache miss.

Brain doesn't fight (3) — the cache is a provider primitive Brain doesn't control. (1) and (2) are operator-driven and accepted; the metric drop signals the change took effect.

## Plays well with Brain cache

Brain's own `llm_cache.redb` (see [`./05_idempotency.md`](./05_idempotency.md)) is upstream of provider-side prompt caching. The local cache short-circuits the entire LLM call on `(input_hash, extractor_id, extractor_version, model_id)` cache hits — no provider call at all. Provider-side prompt caching kicks in only when the local cache misses but the prompt prefix is still warm at the provider.

The two caches are complementary: the local one handles exact-replay idempotency; the provider one handles repeated-prefix amortisation.
