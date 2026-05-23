# 06. SDK Design

> **TL;DR.** The design contract every client SDK satisfies. Rust is the reference; Python, TypeScript, and Go are official. SDKs are moderate-thickness: typed API, connection pooling, retries with backoff, idempotency-key generation, routing in clustered mode, observability hooks, mock clients for tests. No caching, no pre-fetching, no re-ranking. Server and SDK ship together — one wire version, one matched SDK release per language.

## Status

| Field | Value |
|---|---|
| Status | Draft |
| Audience | SDK authors; agent integrators |
| Voice | Hybrid (rationale + normative) |
| Depends on | [04. Wire Protocol](../04_wire_protocol/00_purpose.md), [05. Operations](../05_operations/00_purpose.md) |
| Referenced by | — |

## What this spec defines

The design of client SDKs for Brain — the libraries agents use to talk to Brain. Includes both the abstract contract every SDK should fulfill and concrete idioms for specific languages.

## What this document covers

- The abstract contract every SDK should fulfill.
- Connection management.
- Retry and error handling.
- Streaming responses.
- Language-specific conventions.
- Versioning.

## What this document does not cover

- **The wire protocol.** Defined in [04. Wire Protocol](../04_wire_protocol/00_purpose.md).
- **The semantic operations.** Defined in [05. Operations](../05_operations/00_purpose.md).
- **Server-side concerns.** SDKs are pure-client.

## 1. Why an SDK

The wire protocol is sufficient for any client to talk to Brain. But real applications benefit from:

- A typed, ergonomic API matching the language's idioms.
- Connection pooling and retry logic.
- Observability (metrics, tracing).
- Testing utilities.

SDKs provide these on top of the wire protocol.

## 2. The "official" SDKs

Brain ships official SDKs for:

- **Rust** (the reference; built alongside Brain).
- **Python** (the most common language for agents).
- **TypeScript / JavaScript** (for web-based agents).
- **Go** (for high-throughput agent infrastructure).

Other languages can use the wire protocol directly or generate bindings from a schema.

## 3. The "unofficial" SDK landscape

Third parties may build SDKs for Java, Kotlin, Swift, etc. The wire protocol is documented openly; the spec is implementable.

The Brain project provides:

- The wire-protocol spec.
- A reference implementation (the Rust SDK).
- A test suite that any SDK should pass.

These together let third-party SDKs be drop-in replacements.

## 4. The SDK's API surface

Every SDK exposes:

- A **client** type (the connection / connection pool).
- **Operations** (encode, recall, plan, reason, forget, link, unlink, txn, subscribe, admin).
- **Result types** (memory, recall_result, etc.).
- **Error types** (error codes + messages).
- **Configuration** (servers, retries, timeouts).

These map roughly 1-to-1 to the wire protocol. The SDK is a thin layer in semantic terms; thicker in ergonomic terms.

## 5. The "thin" vs "thick" SDK debate

Two extremes:

- **Thin**: just wire protocol bindings; minimal logic.
- **Thick**: lots of helper methods, abstractions, integrations.

Brain's official SDKs are **moderate**: enough abstraction to feel idiomatic, not so much that they hide Brain's behavior.

Specific abstractions provided:

- Connection pooling.
- Retries with backoff.
- Idempotency-key generation.
- Routing (in clustered mode).

Specific abstractions NOT provided:

- Caching of results.
- Pre-fetching.
- Result re-ranking.
- Cross-operation orchestration.

These are application-level concerns. The SDK doesn't second-guess the application.

## 6. The SDK and async

Most modern languages have async patterns:

- Rust: `async/await`.
- Python: `asyncio` (and sync wrappers for non-async users).
- TypeScript: `async/await`.
- Go: goroutines + channels.

Brain's SDKs are async-first. Synchronous wrappers exist for languages where mixing async/sync is awkward (e.g., Python's sync mode).

## 7. The SDK and types

Strongly-typed languages get full type definitions:

- Rust: idiomatic types with derives.
- TypeScript: typed interfaces.
- Python: type hints (PEP 484+).
- Go: typed structs.

Dynamic languages (JavaScript, Python sans hints) get untyped APIs that match the same shapes.

## 8. The SDK and validation

Client-side validation reduces round-trip errors:

- Check required fields are non-empty.
- Check K is in valid range.
- Check filters are well-formed.

The SDK validates before sending. Server-side validation is the source of truth, but client-side validation gives faster feedback.

## 9. The SDK and observability

SDKs emit:

- Per-request structured logs.
- Per-request metrics (latency, error counts).
- Optional distributed tracing spans.

This integrates with the application's observability stack (Prometheus, Datadog, etc.).

## 10. The SDK and configuration

Configuration via:

- Constructor arguments (typed).
- Environment variables (BRAIN_SERVERS, BRAIN_AUTH_TOKEN, etc.).
- Optional config files.

Sensible defaults so first-time users don't need to configure much.

## 11. The SDK and testing

SDKs include:

- A mock/fake client for unit tests.
- Helpers for spinning up an in-process test server.
- Test fixtures (sample agents, contexts, memories).

Application authors can test their integration without a real Brain server.

## 12. The "hello world" agent

A minimal agent in Python:

```python
import brain

client = brain.Client(servers=["localhost:9090"])

# Encode a memory
memory_id = await client.encode(
    text="The user said hi",
    agent_id="agent-001",
    context="conversation_42",
)

# Recall similar memories
results = await client.recall(
    cue="user greeting",
    agent_id="agent-001",
)

# Forget when no longer needed
await client.forget(memory_id)
```

Three lines of substantive code. No setup beyond the client.

## 13. The "hello world" agent in Rust

```rust
use brain::Client;

let client = Client::new("localhost:9090").await?;

let memory_id = client.encode("The user said hi")
    .agent("agent-001")
    .context("conversation_42")
    .send()
    .await?;

let results = client.recall("user greeting")
    .agent("agent-001")
    .send()
    .await?;

client.forget(memory_id).send().await?;
```

Builder pattern feels idiomatic in Rust. Fluent.

## 14. The SDK and the wire protocol

The SDK and the server ship together; each Brain release publishes one wire version and one matching SDK version per language (see [`01_principles.md`](01_principles.md) §Versioning Policy). When the wire protocol changes, the SDK is republished with the new types and methods, and the matched pair is the only supported configuration.

## 15. The "minimal viable SDK"

A new-language SDK at minimum:

- Implements the wire protocol's framing.
- Supports ENCODE / RECALL / PLAN / REASON / FORGET + LINK / UNLINK.
- Has connection management.
- Has basic retries.

Optional but recommended:

- Streaming.
- Transactions.
- Admin operations.
- Observability hooks.

A minimal SDK is ~1000-2000 lines per language. A full SDK is ~3000-5000 lines.

## 16. The SDK as documentation

For users learning Brain, SDKs are often the first contact. Their API design is part of Brain's documentation.

Brain invests in SDK ergonomics not because they're necessary (the wire protocol is enough) but because they shape user perception.

---

*Continue to [`01_principles.md`](01_principles.md) for design principles.*
