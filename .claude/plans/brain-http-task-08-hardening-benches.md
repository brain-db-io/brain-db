# Phase 11 — Milestone M8 plan

**Task:** Hardening, observability instrumentation, criterion benches,
load test.

**Phase doc target:**
> `just bench brain-http` produces stable numbers;
> 10k-concurrent-connections load test runs 5 min without errors or
> leaks; clippy `-D warnings` green.

**Reads:**
- [`CLAUDE.md`](../../CLAUDE.md) §14 (tracing / OTel pattern).
- `spec/14_observability_ops/03_tracing.md` (OTel HTTP semconv).
- `crates/brain-http/src/observability/span.rs` (M1 stubs).
- Criterion docs.

---

## 1. Scope

M8 is the closing milestone for Phase 11. Three workstreams:

1. **Wire the existing observability stubs.** M1 added
   `connection_span()` and `request_span()` constructors but they're
   not entered anywhere. M8 actually uses them in `accept.rs` and
   `connection.rs`, populates the OTel attributes, and adds a few
   span events at lifecycle points (accept, upgrade, error,
   shutdown-signaled).
2. **Criterion benches.** Three benches as baseline for Phase 12
   regression detection: router dispatch, SSE event encoding, and
   end-to-end request (a single GET round-trip over real TCP). Not
   marketing numbers — they're the floor for "did Phase 12's
   instrumentation slow things down?"
3. **Load test.** A `#[ignore]`-d test in `tests/load.rs` that opens
   10k concurrent connections, drives ~1 request per connection per
   second for 5 minutes, observes no errors and stable memory.
   Operator-invoked (`cargo test ... -- --ignored load_10k`), not in
   CI.

**Out of scope:**

- New OTel exporter wiring (that's Phase 12's metrics + tracing
  work).
- Distributed tracing context propagation (Phase 12).
- Production hardening like `max_connections` cap or rate
  limiting — M8 verifies what we have works under load, not new
  defenses.
- Phase-12-ready metrics counters (e.g. `brain_http_request_total`)
  — those are Phase 12.

---

## 2. New files

```
crates/brain-http/
├── benches/
│   ├── router.rs                # router::Router::dispatch microbench
│   ├── sse_encoder.rs           # sse::encode throughput
│   └── end_to_end.rs            # full request over TCP
└── tests/
    └── load.rs                  # #[ignore] 10k-connection load test
```

Updates:
- `crates/brain-http/src/server/accept.rs` — `tracing::Instrument`
  every per-connection task with `connection_span(peer)`; emit an
  `accept` event on each successful accept.
- `crates/brain-http/src/server/connection.rs` — wrap
  `handle_request` body in `request_span` via `.instrument(span)`.
- `crates/brain-http/src/observability/span.rs` — extend
  `request_span` with `http.response.status_code` (recorded after
  handler returns) and `http.route` if the router can produce one;
  add a `record_status()` helper.
- `crates/brain-http/Cargo.toml` — add criterion dev-dep + `[[bench]]`
  entries.

---

## 3. Observability wiring

### `observability::span` extensions

```rust
use http::Request;
use tracing::Span;

/// Per-request span. Created BEFORE the handler runs; the
/// `http.response.status_code` field is recorded later via
/// [`record_status`] once the response is known.
#[must_use]
pub fn request_span<B>(req: &Request<B>) -> Span {
    tracing::info_span!(
        "http.request",
        http.method   = %req.method(),
        http.path     = %req.uri().path(),
        http.version  = ?req.version(),
        // OTel semconv: server-side status is required.
        http.response.status_code = tracing::field::Empty,
        otel.kind     = "server",
    )
}

/// Set `http.response.status_code` on a request span after the
/// handler returns. Caller passes the response status.
pub fn record_status(span: &Span, status: u16) {
    span.record("http.response.status_code", status);
}
```

### Wiring in `connection.rs`

```rust
pub(crate) async fn handle_request(
    router: Arc<Router<Incoming>>,
    request_timeout: Duration,
    req: http::Request<Incoming>,
) -> Result<Response<ResponseBody>, Infallible> {
    use tracing::Instrument;
    let span = crate::observability::span::request_span(&req);
    async move {
        let dispatched = router.dispatch(req);
        let resp = match tokio::time::timeout(request_timeout, dispatched).await {
            Ok(r) => r,
            Err(_) => timeout_response(request_timeout),
        };
        crate::observability::span::record_status(
            &tracing::Span::current(),
            resp.status().as_u16(),
        );
        Ok(resp)
    }
    .instrument(span)
    .await
}
```

### Wiring in `accept.rs`

```rust
use tracing::Instrument;

// Inside the accept loop:
let peer_span = crate::observability::span::connection_span(peer);
let conn_task = async move {
    if let Err(e) = conn.await {
        warn!(error = %e, "connection task ended with error");
    }
}.instrument(peer_span);
tasks.spawn(conn_task);
```

`connection_span(peer)` was already populated with `net.peer.ip` /
`net.peer.port` in M1; no extension needed there.

---

## 4. Criterion benches

### `benches/router.rs`

Measures pure `Router::dispatch` overhead — no I/O, no network.
Three scenarios:

1. **Hit on exact route** — 10-route table, hit the 5th.
2. **Hit on prefix route** — 10-route table, prefix match.
3. **Miss → 404 fallback** — 10-route table, no match.

```rust
fn bench_router(c: &mut Criterion) {
    let router = build_typical_router(); // 10 routes — 5 exact, 4 prefix, 1 fallback.
    let req_hit = Request::builder().method(Method::GET).uri("/v1/route/5").body(Full::<Bytes>::new(...)).unwrap();
    let req_miss = Request::builder().method(Method::GET).uri("/nope").body(...).unwrap();

    c.bench_function("router_exact_hit", |b| {
        b.to_async(tokio_rt()).iter(|| async {
            router.dispatch(req_hit.clone()).await
        });
    });
    c.bench_function("router_prefix_hit", |b| { ... });
    c.bench_function("router_miss_404", |b| { ... });
}
```

Target: <1 µs per dispatch on x86_64. Failing this means the router
is doing something wrong.

### `benches/sse_encoder.rs`

Pure synchronous; measures `sse::encode(&event)` throughput.

1. **Small event** — `id: 1\ndata: hello\n\n`.
2. **Multi-line event** — 10 newlines in data.
3. **Full event** — id + event + data + retry.

Target: >5 MB/s encoding throughput.

### `benches/end_to_end.rs`

The integration benchmark — measures latency of a full GET round-trip
through the real server.

```rust
fn bench_get_round_trip(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        // start brain-http server on 127.0.0.1:0
    });
    let addr = server.addr();

    c.bench_function("get_healthz_round_trip", |b| {
        b.to_async(&rt).iter(|| async {
            // open a NEW tcp connection, send GET, read response
        });
    });
}
```

Target: <500 µs per round-trip on loopback.

Note: end-to-end bench measures TCP setup + framing + dispatch +
response write. Most time is in TCP; the bench is a smoke check, not
a benchmark of our code in isolation.

---

## 5. Load test

`tests/load.rs`:

```rust
#![cfg(target_os = "linux")] // ulimit + epoll behaviour matters

mod common;
// ... handler etc.

#[ignore = "long-running; run manually with --ignored load_10k"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn load_10k_concurrent_connections() {
    let router = Router::new().get("/healthz", healthz_handler);
    let server = TestServer::start(router).await;
    let addr = server.addr();

    let connections = 10_000usize;
    let duration = std::time::Duration::from_secs(300); // 5 minutes
    let request_period = std::time::Duration::from_secs(1);

    // Spawn 10_000 client tasks. Each opens one keep-alive connection
    // and fires one GET per second for the test duration.
    let mut clients = tokio::task::JoinSet::new();
    let errors = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    for _ in 0..connections {
        let addr = addr;
        let errors = errors.clone();
        clients.spawn(async move {
            // ... persistent TCP, loop sending GETs
        });
    }

    // Wait until the test duration elapses; report stats.
    tokio::time::sleep(duration).await;
    clients.shutdown().await;

    let elapsed = started.elapsed();
    let total_errors = errors.load(Ordering::SeqCst);
    println!("load_10k: {connections} conns × {duration:?}, errors={total_errors}, elapsed={elapsed:?}");

    assert_eq!(total_errors, 0, "load test reported {total_errors} errors");
    server.shutdown().await.expect("shutdown");
}
```

Operator runs manually:

```
cargo test -p brain-http --test load -- --ignored load_10k --nocapture
```

The runner must raise `ulimit -n` to ≥20k before the test (each
connection needs a file descriptor on both sides).

---

## 6. Cargo.toml changes

```toml
[dev-dependencies]
criterion = { workspace = true }

[[bench]]
name = "router"
harness = false

[[bench]]
name = "sse_encoder"
harness = false

[[bench]]
name = "end_to_end"
harness = false
```

`harness = false` is criterion's contract.

---

## 7. Commit shape

```
feat(brain-http): hardening, observability, benches (M8)

Closes Phase 11. Wires the M1 observability stubs into the live
request/connection paths, adds three criterion benches as the
Phase-12 regression baseline, and ships a #[ignore]-d 10k-connection
load test.

Observability wiring:
- accept.rs: every per-connection task .instrument()-ed with
  observability::connection_span(peer). On accept errors and
  shutdown the span gets a warn-level event.
- connection.rs: handle_request body wrapped in
  observability::request_span(&req); status code recorded onto the
  span after the handler returns via record_status().
- observability/span.rs: request_span now records
  `http.response.status_code` per OTel HTTP server semconv. New
  record_status() helper.

Criterion benches (no new dep — criterion already at workspace level):
- benches/router.rs: exact hit, prefix hit, 404 miss. Target
  <1 µs/dispatch on x86_64.
- benches/sse_encoder.rs: small event, multi-line event, full
  event. Target >5 MB/s encoding throughput.
- benches/end_to_end.rs: GET round-trip over loopback TCP. Target
  <500 µs (mostly TCP overhead; smoke check for our path).

Load test:
- tests/load.rs (#[ignore] gated): 10k concurrent persistent
  connections, 1 GET/connection/sec for 5 minutes. Asserts zero
  errors. Operator runs with --ignored; CI doesn't.
- Linux-only via #[cfg]. Requires `ulimit -n` ≥20k.

Out of scope:
- New OTel exporters or metrics counters — Phase 12 territory.
- max_connections cap or rate limiting — verified-current behaviour
  under 10k load is the M8 gate; new defenses are post-Phase-11.

just docker-verify green. clippy --all-targets --all-features -D
warnings clean.
```

---

## 8. Done when

- [ ] Per-connection and per-request spans entered in the live
      paths; `http.response.status_code` recorded.
- [ ] 3 criterion benches compile and produce numbers when run via
      `cargo bench -p brain-http`.
- [ ] `tests/load.rs` compiles; `cargo test -- --ignored load_10k`
      runs to completion locally with zero errors (one operator-side
      run before commit, numbers recorded in the commit message).
- [ ] `just docker-verify` green.
- [ ] Phase doc 11.M8 ticked.

---

## 9. Open questions

1. **Should benches run in CI?** `cargo bench` takes ~30 s to
   stabilise; running on every PR is overhead. **Recommendation:**
   no. Run on demand. Phase 12 may add a regression-detection step.

2. **`tracing::Instrument` vs `Span::in_scope`?** `instrument()`
   wraps a future and re-enters the span on every poll;
   `in_scope` is for synchronous code. **Recommendation:**
   `instrument()` for the connection / request tasks. The router's
   internal match isn't worth a separate span.

3. **Record errors as span fields or events?** Currently
   `warn!(error = %e, ...)` emits a separate log event. Adding
   `error.message` to the span is also valid. **Recommendation:**
   keep the log event; the span carries status + lifecycle, the
   event carries the message. Mirrors most production OTel
   patterns.

4. **Load test timeout enforcement?** 5 minutes is exactly the
   `tokio::time::sleep` budget. If the spawn / shutdown loop takes
   longer the test runs longer. **Recommendation:** wrap the whole
   test in `tokio::time::timeout(Duration::from_secs(360), ...)` —
   60 s slack for setup/teardown.

---

## 10. Risks

- **`#[ignore]`-d load test rots.** No CI catches regressions in
  it. Mitigation: include a one-line note in `M8` of the phase doc
  that says when it was last run + what numbers it produced.

- **`tokio::task::JoinSet` shutdown semantics.** `JoinSet::shutdown`
  aborts in-flight tasks. The load test wants tasks to drain
  gracefully when the duration expires. Mitigation: pass a
  shutdown signal (CancellationToken-style) into each client task
  and have them exit cleanly.

- **`ulimit -n` mismatch.** Default macOS / Linux `ulimit -n` is
  often 1024 or 4096. The load test needs ~20k. Mitigation: assert
  the current ulimit at test start and skip with a helpful message
  if it's too low.

- **Span explosion.** Every connection enters a span; every request
  enters a child span. At 10k connections × 1 RPS × 5 min = 3M
  spans. The default `tracing-subscriber` will OOM if it tries to
  buffer them. Mitigation: the load test runs without a tracing
  subscriber installed (or with a no-op filter); operator can enable
  tracing for short observations only.

- **End-to-end bench variability.** Single-machine TCP latency over
  loopback varies by ±20 % depending on system load. Mitigation:
  document the variance in the bench's rustdoc; the bench is a
  baseline, not a benchmark of our code in isolation.

- **Criterion HTML report directory.** `cargo bench` produces
  `target/criterion/...` HTML reports. They're not in `.gitignore`
  by default. Mitigation: confirm `.gitignore` ignores
  `target/criterion`.
