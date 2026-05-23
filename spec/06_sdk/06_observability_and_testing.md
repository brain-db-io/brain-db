# 06.06 Observability and Testing

> **TL;DR.** SDK-level observability (metrics, logs, traces — OpenTelemetry-style with auto-propagated context) and the test support story (mock clients, fixture helpers, in-process fake server for integration tests).

## SDK-Level Observability

What the SDK exposes for monitoring, logging, and tracing.

## 1. The three signals

The SDK supports OpenTelemetry-style observability:

- **Logs** — structured per-request entries.
- **Metrics** — counters, histograms, gauges.
- **Traces** — distributed tracing spans.

These integrate with the application's observability stack.

## 2. Logs

Each request produces log entries:

```json
{
  "ts": "2026-05-07T12:00:00Z",
  "level": "debug",
  "operation": "encode",
  "agent_id": "agent-001",
  "request_id": "...",
  "duration_ms": 8,
  "status": "success"
}
```

Log fields:

- Operation name.
- Agent ID (for correlation).
- Request ID.
- Duration.
- Status (success / error code).
- Optional: server, retry attempt, etc.

The user configures the log level (default INFO; debug shows per-request).

## 3. The logger interface

The SDK uses the language's standard logging:

- Rust: `tracing` crate.
- Python: `logging` module.
- TypeScript: pluggable; defaults to `console`.
- Go: `log/slog` (Go 1.21+).

Users can plug their own logger:

```rust
let client = Client::builder()
    .logger(MyCustomLogger::new())
    .build();
```

## 4. Metrics

The SDK exposes metrics:

```
brain_client_requests_total{operation="encode", status="success"} 12345
brain_client_request_duration_ms{operation="encode", quantile="0.99"} 0.025
brain_client_retries_total{operation="encode"} 23
brain_client_connections_active{server="host1:9090"} 4
brain_client_streams_active 2
```

Standard Prometheus naming.

The Client exports a `metrics()` accessor; users can scrape or push to their stack.

## 5. The metrics integration

```rust
use prometheus::Registry;

let registry = Registry::new();
let client = Client::builder()
    .metrics_registry(registry.clone())
    .build();

// Other parts of the app use the same registry.
```

For Python:

```python
from prometheus_client import REGISTRY

client = brain.Client(metrics_registry=REGISTRY)
```

The SDK's metrics integrate with the rest of the app's metrics.

## 6. Tracing

For distributed tracing, the SDK creates spans:

```rust
let _span = tracing::info_span!("brain.encode", agent_id, request_id).entered();
```

Each operation has a span:

- Span name: `brain.<operation>`.
- Span attributes: operation parameters (agent ID, etc.).
- Status: success / error.
- Duration: from start to response.

Spans nest in the application's tracing context. If the application is using OpenTelemetry, the Brain SDK spans appear as children of the application's spans.

## 7. The trace propagation

The SDK propagates trace context to Brain:

- Brain logs include the trace ID.
- Brain's traces (if it uses OTel) become children of the SDK's spans.

End-to-end traces show the request flowing from application → SDK → server → response.

This integration matters for debugging in production.

## 8. The custom hooks

For arbitrary side effects, the SDK exposes hooks:

```rust
let client = Client::builder()
    .on_request(|req| log::debug!("Request: {:?}", req))
    .on_response(|resp| log::debug!("Response: {:?}", resp))
    .on_error(|err| metrics::increment("brain.errors", &[("code", err.code().as_str())]))
    .build();
```

Hooks fire at well-defined points. They're optional; default is no-op.

## 9. The "audit" mode

For compliance scenarios:

```rust
let client = Client::builder()
    .audit_log(AuditConfig {
        enabled: true,
        log_path: "/var/log/brain-audit.log",
        include_payloads: false,    // Privacy-aware
    })
    .build();
```

Audit mode logs every operation with a stable schema. Used for compliance, security review, and debugging.

## 10. The "request tracing" detail

For debugging, the SDK can trace individual requests:

```rust
client.encode("text")
    .trace(true)
    .send()
    .await?;
```

Tracing for one request includes:

- The request payload.
- Per-attempt details.
- The final response.
- Latency breakdown.

This is intentionally verbose; useful for debugging specific failures.

## 11. The "circuit breaker" metrics

If the SDK has a circuit breaker (per [§06.03 Connection](03_connection.md)):

```
brain_client_circuit_state{server="host1:9090"} 0    # 0=closed, 1=open, 2=half-open
brain_client_circuit_failures_total{server="host1:9090"} 5
brain_client_circuit_opens_total{server="host1:9090"} 2
```

These help operators understand when the SDK is shedding load.

## 12. The "connection pool" metrics

```
brain_client_connections_active{server="host1:9090"} 4
brain_client_connections_idle{server="host1:9090"} 2
brain_client_connections_failed_total{server="host1:9090"} 1
brain_client_connection_age_sec{server="host1:9090", quantile="0.5"} 120
```

The pool's behavior is exposed; tuning becomes data-driven.

## 13. The "stream" metrics

For SUBSCRIBE clients:

```
brain_client_subscribe_active 2
brain_client_subscribe_events_received_total 1234567
brain_client_subscribe_buffer_size{stream_id="..."} 42
brain_client_subscribe_lag_sec{stream_id="...", quantile="0.99"} 0.005
```

Stream health is visible.

## 14. The "user-defined attributes"

The SDK allows custom tags:

```rust
let client = Client::builder()
    .default_tags([("team", "search"), ("env", "production")])
    .build();
```

These tags are added to all metrics from this client. Useful for multi-team deployments.

## 15. The "performance" measurement

For benchmarking:

```rust
client.metrics().request_count();
client.metrics().avg_latency();
client.metrics().error_rate();
```

These are also available as Prometheus metrics; the API just gives quick access.

## 16. The "dump state" debugging

For deep debugging:

```rust
let snapshot = client.debug_snapshot();
println!("{:#?}", snapshot);
```

Returns a structure describing:

- Connection state.
- Pending requests.
- Recent errors.
- Configuration.

Used during troubleshooting; not for production code.

## 17. The "logging level guidance"

What to log at each level:

- ERROR: failed requests after retries; connection failures; programmer errors.
- WARN: retries; slow requests; unusual patterns.
- INFO: client lifecycle (start, stop, reconnects).
- DEBUG: per-request details.
- TRACE: per-frame details.

Default level: INFO. Production deployments may use WARN or ERROR.

---

## Test Support

What the SDK provides for testing.

## 1. The test challenge

Testing applications that use the Brain SDK has typical issues:

- Real Brain connections require a running server (heavy).
- Mocking every call manually requires a lot of boilerplate.
- Tests need specific scenarios (errors, slow responses, etc.).

The SDK provides:

- A **mock client** with programmable responses.
- An **in-process fake server** for end-to-end testing.
- Test fixtures and utilities.

## 2. The mock client

```rust
let mock = MockClient::new();
mock.on_encode(|req| {
    Ok(EncodeResult {
        memory_id: MemoryId::test(1),
        ...
    })
});

mock.on_recall(|req| {
    if req.cue_text == "hello" {
        Ok(vec![RecallResult { ... }])
    } else {
        Err(BrainError::NotFound)
    }
});

// Use mock as a regular client
let id = mock.encode("text").send().await?;
```

```python
mock = brain.testing.MockClient()
mock.expect_encode(returns=brain.MemoryId(b'\x01' * 16))
mock.expect_recall(when=lambda req: req.cue == "hello", returns=[...])

memory_id = await mock.encode("text", agent_id="test")
```

The mock is functionally identical to a real client; calls are recorded, responses are programmed.

## 3. Call recording

The mock records all calls:

```rust
let calls = mock.calls();
assert_eq!(calls.encode_count(), 1);
assert_eq!(calls.recall_count(), 0);
let last_encode = calls.last_encode().unwrap();
assert_eq!(last_encode.text, "text");
```

Tests can verify the application's interaction with the SDK.

## 4. The fake server

For higher-fidelity tests, an in-process fake server:

```rust
let fake = FakeBrain::new();
let client = Client::connect_to_fake(&fake).await?;

client.encode("text").send().await?;
let results = client.recall("text").send().await?;
assert_eq!(results.len(), 1);
```

The fake:
- Accepts wire-protocol calls from a real Client.
- Maintains in-memory state (memories, edges, contexts).
- Implements simplified versions of the operations.

It's not a real Brain server (no HNSW, no WAL); it's just enough to test client logic end-to-end.

## 5. The fake's fidelity

The fake implements:
- ENCODE: records the memory in an in-memory store.
- RECALL: simple text similarity (or programmable).
- FORGET: removes the memory.
- LINK / UNLINK: edge tracking.
- TXN: groups operations.
- SUBSCRIBE: streams events.

The fake doesn't implement:
- Vector embedding (uses placeholder vectors).
- HNSW search (uses placeholder similarity).
- Real durability (in-memory only).

For tests of client behavior, this is enough.

## 6. The "real server" for integration tests

For tests that need a real Brain server:

```rust
#[test]
async fn integration_test() {
    let server = TestServer::start_in_memory().await?;
    let client = Client::connect(server.address()).await?;

    // Run a real test against a real server
    let id = client.encode("real text").send().await?;
    let results = client.recall("real text").send().await?;

    server.shutdown().await?;
}
```

`TestServer` spins up a real Brain process with in-memory storage:

- No persistent files.
- Single shard.
- Auto-shutdown.

Integration tests are slower (~seconds to start) but exercise the full stack.

## 7. The fixture library

Common test fixtures:

```rust
let fixtures = brain::testing::Fixtures::new();
let agent = fixtures.agent("test-agent");
let memory = fixtures.encode("test memory", &agent);
let context = fixtures.context("test-context", &agent);
```

Saves boilerplate in tests.

## 8. The deterministic test mode

For deterministic tests:

```rust
let client = Client::builder()
    .deterministic_request_ids(seed = 42)
    .deterministic_clock(start_at = "2026-05-07T00:00:00Z")
    .build();
```

Request IDs become deterministic (based on the seed). Time-based features (timestamps) use the fake clock.

This makes tests reproducible — same input, same output, every time.

## 9. The chaos / failure injection

```rust
let mock = MockClient::new();
mock.inject_failure_rate(0.1);    // 10% of calls fail with random errors
mock.inject_latency(Duration::from_millis(100));    // All calls have 100ms delay
mock.inject_intermittent_disconnect();
```

Tests for retry / error handling logic.

## 10. The schema tests

For validating that custom code matches the wire protocol:

```rust
brain::testing::wire_schema_test! {
    #[test]
    fn test_encode_request_schema() {
        let req = EncodeRequest { ... };
        let bytes = serialize(&req);
        let parsed: EncodeRequest = deserialize(&bytes)?;
        assert_eq!(req, parsed);
    }
}
```

Ensures clients and servers agree on the wire format.

## 11. The "test the SDK itself" tests

The SDK has its own test suite:

- Unit tests for individual functions.
- Integration tests against the fake server.
- End-to-end tests against a real Brain server.
- Property tests for invariants.

This is the SDK's quality bar. Application authors don't run these (they're internal to the SDK).

## 12. The "shared test suite"

A canonical test suite verifies any SDK conforms to the spec:

```
tests/
├── conformance/
│   ├── encode_basic.yaml
│   ├── recall_filters.yaml
│   ├── transactions.yaml
│   └── ...
└── ...
```

Each YAML describes a scenario:

```yaml
name: encode_then_recall
steps:
  - operation: encode
    text: "hello world"
    agent_id: agent-1
    expect:
      success: true
      memory_id: not_null
  - operation: recall
    cue: "hello"
    agent_id: agent-1
    expect:
      results_count: 1
      first_score: ">0.5"
```

Each language SDK runs the same scenarios and verifies its output. Conformance is checkable.

## 13. The "snapshot testing" pattern

For complex outputs:

```rust
let response = client.plan("goal").send().await?;
insta::assert_yaml_snapshot!(response);
```

Snapshot files capture expected output; tests fail if output diverges.

For text-heavy responses (PLAN, REASON), snapshot testing prevents regressions in output formatting.

## 14. The "load testing" facility

```rust
let client = Client::connect(server).await?;
let load = brain::testing::LoadGenerator::new()
    .ops_per_second(1000)
    .duration(Duration::from_secs(60))
    .pattern(LoadPattern::EncodeRecall { ratio: 0.7 });

let stats = load.run(&client).await?;
println!("p99 latency: {}", stats.p99_latency);
```

For benchmarking the SDK and server together. Used in CI for regression detection.

## 15. The "test isolation"

Each test should be isolated:

- Fresh Client per test.
- Fresh test data (or rolled back).
- No global state leaking between tests.

The SDK's test utilities support this:

```rust
let _guard = brain::testing::IsolatedTest::start();
// Test code here.
// Guard's drop cleans up.
```

## 16. The "production safety" check

A common test: make sure the test suite doesn't accidentally talk to production:

```rust
fn assert_test_environment() {
    let env = std::env::var("BRAIN_ENV").unwrap();
    assert!(env == "test" || env == "staging");
}
```

The SDK can panic if production endpoints are used in tests.

---

*Continue to [`07_typed_graph_sdk.md`](07_typed_graph_sdk.md) for the typed-graph SDK surface.*
