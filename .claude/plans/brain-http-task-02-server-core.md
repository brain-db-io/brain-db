# Phase 11 — Milestone M2 plan

**Task:** Server core — accept loop + Router + Connection + limits + graceful shutdown.

**Phase doc target:**
> Integration test issues GET/POST and round-trips bodies via hyper;
> keep-alive works automatically (free from hyper); graceful shutdown
> drains in-flight requests.

**Reads:**
- `crates/brain-server/src/network/connection.rs` — existing TCP
  bind + shutdown pattern to mirror.
- `crates/brain-server/src/network/dispatch.rs` — per-connection
  Tokio task pattern.
- [hyper 1.x `server::conn::http1`](https://docs.rs/hyper/1/hyper/server/conn/http1/index.html)
- [`hyper_util::server::graceful::GracefulShutdown`](https://docs.rs/hyper-util/0.1/hyper_util/server/graceful/struct.GracefulShutdown.html)
- [`hyper_util::rt::TokioIo`](https://docs.rs/hyper-util/0.1/hyper_util/rt/struct.TokioIo.html)

---

## 1. Scope

M2 produces a working HTTP/1.1 server that:

1. Binds a TCP socket with the same socket options the existing
   `brain-server::network::connection` uses (`SO_REUSEADDR`,
   `TCP_NODELAY`, `SO_KEEPALIVE`).
2. Accepts connections in a loop, spawning a per-connection task.
3. Drives each connection through `hyper::server::conn::http1::Builder`,
   bridged via `hyper_util::rt::TokioIo`.
4. Dispatches each request through a Brain-owned `Router`.
5. Enforces per-request limits (max header size, max body size,
   request timeout).
6. Drains in-flight connections on graceful shutdown via
   `hyper_util::server::graceful::GracefulShutdown`.

After M2, **brain-http can serve HTTP**. M3 migrates `brain-server::admin`
to use it.

**Explicitly out of scope:**
- TLS termination (gated behind `tls` feature; lands as a small
  follow-up; M2 ships the non-TLS path).
- Streaming bodies via chunked transfer (M4 — hyper already produces
  chunked responses transparently when no `Content-Length` is set, but
  Brain SSE etc. wait until M4).
- WebSocket Upgrade handler (M6).
- `max_connections` cap and rate limiting (M8 — hardening).
- HTTP/2 (deferred to a future phase; abstractions designed for it).
- Per-route middleware (deferred indefinitely — Brain's admin surface
  doesn't need it).

---

## 2. New files

```
crates/brain-http/src/
├── tcp/
│   ├── mod.rs                  # public surface (bind helpers, BindConfig)
│   └── socket.rs               # SO_REUSEADDR, TCP_NODELAY, SO_KEEPALIVE
├── router/
│   ├── mod.rs                  # Router type
│   ├── route.rs                # RouteEntry
│   └── matcher.rs              # exact + prefix matching, path-param extract
└── server/
    ├── mod.rs                  # HttpServer builder + entry point
    ├── accept.rs               # accept loop
    ├── connection.rs           # per-connection serve via hyper http1
    ├── limits.rs               # ServerLimits config struct
    └── shutdown.rs             # Shutdown handle (Notify + GracefulShutdown wrap)
```

Plus:
- `crates/brain-http/src/lib.rs` — expose `tcp`, `router`, `server`.
- `crates/brain-http/tests/server_smoke.rs`
- `crates/brain-http/tests/server_router.rs`
- `crates/brain-http/tests/server_shutdown.rs`

---

## 3. Type signatures

### `server/limits.rs`

```rust
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ServerLimits {
    /// Maximum bytes for the request head (request line + headers).
    /// Default 16 KiB. Mirrors the existing admin assumption.
    pub max_header_bytes: usize,
    /// Maximum inbound body size. Default `MAX_BODY_BYTES` (16 MiB).
    pub max_body_bytes: u64,
    /// Per-request wall-clock timeout (head + body + handler).
    /// Default 30 s.
    pub request_timeout: Duration,
    /// Per-connection idle timeout. Default 60 s.
    pub idle_timeout: Duration,
}

impl Default for ServerLimits {
    fn default() -> Self { ... }
}
```

### `server/shutdown.rs`

```rust
use std::sync::Arc;
use tokio::sync::Notify;
use hyper_util::server::graceful::GracefulShutdown;

#[derive(Clone)]
pub struct ShutdownSignal {
    notify: Arc<Notify>,
}

impl ShutdownSignal {
    pub fn new() -> (ShutdownHandle, Self) { ... }
    pub async fn wait(&self) { self.notify.notified().await }
}

pub struct ShutdownHandle {
    notify: Arc<Notify>,
}

impl ShutdownHandle {
    /// Signal accept loop to stop and connection tasks to drain.
    pub fn shutdown(self) { self.notify.notify_waiters(); }
}

/// Internal: wraps hyper-util's GracefulShutdown to track in-flight
/// connection futures. Exposed for tests; servers usually don't
/// instantiate this directly.
pub(crate) struct InFlight {
    graceful: GracefulShutdown,
}
```

### `tcp/socket.rs`

```rust
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct BindConfig {
    pub reuse_addr: bool,        // default true
    pub reuse_port: bool,        // default false (multi-listener setups)
    pub tcp_nodelay: bool,       // default true (lower per-request latency)
    pub keepalive: Option<Duration>,  // None = OS default
}

impl Default for BindConfig { ... }

/// Bind a TCP listener with Brain's standard socket options.
pub async fn bind(addr: SocketAddr, cfg: &BindConfig) -> io::Result<TcpListener> {
    // socket2-based: socket(), set_reuseaddr, set_nodelay, bind, listen.
    // Mirrors crates/brain-server/src/network/connection.rs::bind_listener.
}

/// Apply per-stream options after accept (TCP_NODELAY in particular —
/// inherited from the listener on most kernels but not all).
pub fn apply_stream_opts(stream: &TcpStream, cfg: &BindConfig) -> io::Result<()>;
```

### `router/mod.rs`

The router is intentionally simple. Brain's admin surface has ~15
routes; a match-based dispatch with exact / prefix / fallback is
plenty. Anything fancier (radix trie, typed extractors) is over-
tooled until we have 100s of routes.

```rust
use http::{Method, Request, Response};

use crate::body::ResponseBody;

pub struct Router<B> {
    routes: Vec<RouteEntry<B>>,
    fallback: Option<BoxedAsyncHandler<B>>,
}

enum RouteEntry<B> {
    Exact {
        method: Method,
        path: &'static str,
        handler: BoxedAsyncHandler<B>,
    },
    Prefix {
        method: Method,
        prefix: &'static str,
        handler: BoxedAsyncHandler<B>,
    },
}

impl<B: Send + 'static> Router<B> {
    #[must_use]
    pub fn new() -> Self { ... }

    /// Add an exact-match route. `path` must start with `/`.
    pub fn route(
        self,
        method: Method,
        path: &'static str,
        handler: impl AsyncHandler<B>,
    ) -> Self { ... }

    /// Convenience: GET.
    pub fn get(self, path: &'static str, h: impl AsyncHandler<B>) -> Self {
        self.route(Method::GET, path, h)
    }
    pub fn post(self, path: &'static str, h: impl AsyncHandler<B>) -> Self {
        self.route(Method::POST, path, h)
    }
    pub fn delete(self, path: &'static str, h: impl AsyncHandler<B>) -> Self {
        self.route(Method::DELETE, path, h)
    }

    /// Add a prefix-match route. Handler is responsible for parsing
    /// any additional path segments (e.g. `/v1/snapshots/{id}/delete`).
    /// Matches the dispatch pattern that brain-server::admin already uses.
    pub fn route_prefix(
        self,
        method: Method,
        prefix: &'static str,
        handler: impl AsyncHandler<B>,
    ) -> Self { ... }

    /// Fallback when nothing matches. If absent, the router returns
    /// 404 Not Found.
    pub fn fallback(self, handler: impl AsyncHandler<B>) -> Self { ... }

    /// Dispatch one request. Implements `hyper::service::Service`-style
    /// shape but is not itself a Service (we adapt in `Connection`).
    pub async fn dispatch(
        &self,
        req: Request<B>,
    ) -> crate::Result<Response<ResponseBody>>
    where
        B: Send + 'static;
}
```

### `server/connection.rs`

```rust
use std::sync::Arc;
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;

use crate::router::Router;
use crate::server::limits::ServerLimits;

/// Serve one accepted TCP connection through hyper http1.
///
/// Adapts the Brain `Router` into a `hyper::service::Service` and
/// hands it to hyper's connection driver. Keep-alive is automatic
/// (free from hyper). The future resolves when the peer closes or
/// the server drops the connection.
pub(crate) async fn serve_connection<B>(
    stream: TcpStream,
    router: Arc<Router<hyper::body::Incoming>>,
    limits: Arc<ServerLimits>,
) -> crate::Result<()>
where
    B: Send + 'static,
{
    let io = TokioIo::new(stream);
    let builder = http1::Builder::new()
        .max_buf_size(limits.max_header_bytes.max(8 * 1024))
        .keep_alive(true);

    let service = hyper::service::service_fn(move |req| {
        let router = router.clone();
        let limits = limits.clone();
        async move {
            // Wrap router dispatch with the per-request timeout.
            match tokio::time::timeout(
                limits.request_timeout,
                router.dispatch(req),
            ).await {
                Ok(Ok(resp)) => Ok(resp),
                Ok(Err(e))   => Ok(error_response(&e)),
                Err(_)       => Ok(timeout_response(limits.request_timeout)),
            }
        }
    });

    builder.serve_connection(io, service).await
        .map_err(crate::Error::Hyper)
}
```

(The `Result<Response<ResponseBody>, Error>` → response mapping
collapses errors into HTTP responses inside the service closure —
brain-http always returns a response, never an error, at the hyper
layer.)

### `server/accept.rs`

```rust
use std::net::SocketAddr;
use std::sync::Arc;
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::router::Router;
use crate::server::limits::ServerLimits;
use crate::server::shutdown::ShutdownSignal;

pub async fn run(
    listener: TcpListener,
    router: Arc<Router<hyper::body::Incoming>>,
    limits: Arc<ServerLimits>,
    shutdown: ShutdownSignal,
) -> crate::Result<()> {
    let graceful = GracefulShutdown::new();
    let local_addr = listener.local_addr()?;
    info!(addr = %local_addr, "brain-http server accepting");

    loop {
        tokio::select! {
            biased;
            () = shutdown.wait() => {
                info!(addr = %local_addr, "brain-http server shutting down");
                break;
            }
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(p) => p,
                    Err(e) => { warn!(error = %e, "accept failed"); continue; }
                };
                // Apply per-stream socket options.
                let _ = crate::tcp::apply_stream_opts(&stream, &Default::default());

                let router = router.clone();
                let limits = limits.clone();
                let conn = crate::server::connection::serve_connection(
                    stream, router, limits,
                );
                // Track this connection for graceful drain.
                let fut = graceful.watch(conn.into_owned());
                tokio::spawn(fut.in_current_span());
            }
        }
    }

    // Drain in-flight with a 30-second cap.
    match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        graceful.shutdown(),
    ).await {
        Ok(()) => info!("brain-http drained cleanly"),
        Err(_) => warn!("brain-http drain timed out; abandoning open connections"),
    }
    Ok(())
}
```

### `server/mod.rs`

```rust
use std::net::SocketAddr;
use std::sync::Arc;

use crate::router::Router;
use crate::server::limits::ServerLimits;
use crate::server::shutdown::{ShutdownHandle, ShutdownSignal};

pub struct HttpServer<B = hyper::body::Incoming> {
    addr: SocketAddr,
    router: Router<B>,
    limits: ServerLimits,
    bind_config: crate::tcp::BindConfig,
}

impl HttpServer<hyper::body::Incoming> {
    pub fn bind(addr: SocketAddr) -> Self { ... }
    pub fn router(mut self, r: Router<hyper::body::Incoming>) -> Self { ... }
    pub fn limits(mut self, l: ServerLimits) -> Self { ... }
    pub fn bind_config(mut self, c: crate::tcp::BindConfig) -> Self { ... }

    /// Serve forever (or until `shutdown.shutdown()` is called).
    /// Returns a `(handle, future)` pair. The caller spawns the
    /// future; the handle is used to trigger graceful shutdown.
    pub async fn serve(self) -> crate::Result<(ShutdownHandle, BoundServer)> { ... }
}

pub struct BoundServer {
    listener: tokio::net::TcpListener,
    router: Arc<Router<hyper::body::Incoming>>,
    limits: Arc<ServerLimits>,
    shutdown_signal: ShutdownSignal,
}

impl BoundServer {
    pub fn local_addr(&self) -> std::net::SocketAddr { ... }
    pub async fn run(self) -> crate::Result<()> {
        crate::server::accept::run(
            self.listener,
            self.router,
            self.limits,
            self.shutdown_signal,
        ).await
    }
}
```

Usage from tests / brain-server:

```rust
let router = Router::new()
    .get("/healthz", healthz_handler)
    .get("/metrics", metrics_handler)
    .route_prefix(Method::POST, "/v1/snapshots", snapshot::dispatch);

let (shutdown, bound) = HttpServer::bind("127.0.0.1:0".parse()?)
    .router(router)
    .serve().await?;

let addr = bound.local_addr();
tokio::spawn(bound.run());

// ... do work ...

shutdown.shutdown();
```

---

## 4. The `AsyncHandler` adapter to `hyper::Service`

This is the one piece of glue that's a bit subtle. Brain handlers
look like:

```rust
async fn healthz(req: Request<Incoming>) -> Result<Response<ResponseBody>>
```

hyper's `service_fn` accepts a closure of this exact shape — so the
glue inside `Router::dispatch` is just `(handler)(req).await`.

The boxing matters because `Router` stores heterogeneous handlers in
a `Vec`. We use `BoxedAsyncHandler<B>`:

```rust
type BoxedAsyncHandler<B> = Box<
    dyn Fn(Request<B>) -> Pin<Box<dyn Future<Output = crate::Result<Response<ResponseBody>>> + Send>>
    + Send + Sync,
>;
```

`Router::route` constructs the box from a closure:

```rust
pub fn route(mut self, method: Method, path: &'static str, handler: impl AsyncHandler<B>) -> Self {
    let boxed: BoxedAsyncHandler<B> = Box::new(move |req| {
        Box::pin(handler.call(req))
    });
    ...
}
```

This means `impl AsyncHandler<B>` is generic and we monomorphize
once per handler type at the call site. Identical to how axum / tower
do it.

---

## 5. Tests

### `tests/server_smoke.rs`

Drive the real server with a real TCP client. Three tests:

```rust
#[tokio::test]
async fn get_round_trip() {
    let server = TestServer::start(|| {
        Router::new().get("/healthz", healthz_handler)
    }).await;

    let resp = client_get(server.addr(), "/healthz").await;
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body, "ok");

    server.shutdown().await;
}

#[tokio::test]
async fn post_with_body_round_trip() { ... }

#[tokio::test]
async fn keep_alive_works_on_one_connection() {
    // Open one TcpStream, send 5 requests with `Connection: keep-alive`.
    // Assert all 5 round-trip.
}
```

A small `TestServer` helper in `tests/common/mod.rs` wraps the bind
+ spawn pattern.

### `tests/server_router.rs`

Test router matching directly (no real TCP):

```rust
#[tokio::test]
async fn exact_match_wins_over_prefix() { ... }
#[tokio::test]
async fn fallback_returns_when_no_match() { ... }
#[tokio::test]
async fn unmatched_method_returns_405() { ... }
#[tokio::test]
async fn unknown_path_returns_404() { ... }
```

### `tests/server_shutdown.rs`

```rust
#[tokio::test]
async fn shutdown_drains_in_flight() {
    // Handler that sleeps 100 ms. Issue a request, then trigger
    // shutdown. Verify the response still arrives (drain works) and
    // the accept loop exits after.
}

#[tokio::test]
async fn shutdown_timeout_abandons_stragglers() {
    // Handler that sleeps 60 s. Trigger shutdown. Verify the accept
    // loop exits within ~30 s (the drain timeout).
}
```

### Unit tests (colocated)

- `router::matcher::tests::exact_match`
- `router::matcher::tests::prefix_match`
- `router::matcher::tests::case_sensitive_path`
- `tcp::socket::tests::default_bind_config`

---

## 6. Commit shape

```
feat(brain-http): server core — accept loop, Router, Connection, shutdown (M2)

Adds the server side of brain-http. After M2 the crate can serve
HTTP/1.1 end to end; M3 migrates brain-server::admin to use it.

Components:
- tcp/: bind helpers (SO_REUSEADDR, TCP_NODELAY, SO_KEEPALIVE).
  Same socket options the existing brain-server::network::connection
  already applies — re-homed here so brain-http is self-contained.
- router/: match-based dispatch (exact + prefix + fallback). Brain's
  admin surface has ~15 routes; a radix trie or typed-extractor
  framework would be over-tooled. Mirrors the dispatch pattern
  brain-server::admin already uses internally.
- server/connection.rs: bridges tokio::net::TcpStream into hyper via
  TokioIo, runs hyper::server::conn::http1::Builder::serve_connection
  with the Brain router adapted as a hyper Service.
- server/limits.rs: max_header_bytes (16 KiB), max_body_bytes (16 MiB),
  request_timeout (30 s), idle_timeout (60 s).
- server/shutdown.rs: ShutdownSignal + ShutdownHandle pair. Triggers
  accept loop to stop and hyper_util::server::graceful::GracefulShutdown
  to drain in-flight connections with a 30 s cap.
- server/mod.rs + server/accept.rs: HttpServer builder + accept loop.

Keep-alive comes free from hyper. Chunked transfer encoding for
streaming bodies lands in M4 (SSE).

Test surface:
- tests/server_smoke.rs: GET / POST / keep-alive on real TCP.
- tests/server_router.rs: routing precedence + 404 + 405.
- tests/server_shutdown.rs: drain works; timeout abandons stragglers.
- Unit tests on router matcher + tcp socket config.
```

---

## 7. Done when

- [ ] `tcp/`, `router/`, `server/` modules compile and tests pass.
- [ ] Real-TCP integration test in `tests/server_smoke.rs` round-trips
      a GET and a POST through the new server.
- [ ] Keep-alive integration test sends 5 requests on one connection
      and all succeed.
- [ ] Graceful shutdown integration test passes (drain + timeout).
- [ ] Router unit tests pass for exact / prefix / fallback / 404 / 405.
- [ ] `just docker-verify` green.
- [ ] M2 commit lands.
- [ ] Phase doc 11.M2 ticked.

---

## 8. Open questions

1. **Should `Router` be generic over `B` or fixed to `hyper::body::Incoming`?**
   `Incoming` is what hyper produces on the server side. We never
   really see other body types at the router. **Recommendation:**
   generic. The unit tests benefit (we can route synthetic
   `Full<Bytes>` requests without a real listener), and Phase 12's
   internal middleware-style routing for metrics-collection wrappers
   will want generic too.

2. **Path-param extraction or just prefix?** Brain's existing admin
   code parses `/v1/snapshots/{id}/delete` manually inside the handler
   (`path.strip_prefix("/v1/snapshots/")?.split('/').next()`). The
   pattern works and we don't need to invent a typed extractor.
   **Recommendation:** prefix-only in M2. Handlers parse param
   segments themselves. If a future phase grows the route count past
   ~30, revisit with a small radix trie.

3. **`max_buf_size` mapping from `ServerLimits::max_header_bytes`.**
   hyper's `http1::Builder::max_buf_size` applies to the per-connection
   read buffer (both head and body bytes). Our `max_header_bytes`
   semantically is just the head. **Recommendation:** wire
   `max_buf_size = max(max_header_bytes, 8 KiB)` — large enough to
   not constrain small bodies, small enough that a single oversized
   request can't blow memory. M4 revisits when streaming bodies land.

4. **What's the response when a handler returns an `Error`?**
   `error_response(&err)` reads `err.status_code()` and returns a
   plain-text body `{ "error": "<error string>" }` (JSON-shaped). For
   admin routes this matches the existing pattern. **Recommendation:**
   start with plain JSON; M3 may refine when the migration surfaces
   specific response-shape needs (e.g. the 501 deferred-marker shape
   from Phase 10).

---

## 9. Risks

- **`graceful.watch` requires `Connection: 'static`.** hyper's
  connection future is `'static` only when the service and IO are
  both `'static`. The router is in an `Arc<Router>` (handles
  cloning); the limits in `Arc<ServerLimits>`; the IO is `TokioIo<TcpStream>`
  which is `'static`. Should be fine, but if lifetime errors surface,
  mitigation is to wrap the service in a `Box<dyn Service + 'static>`.

- **Per-request timeout interacts with streaming bodies.** Wrapping
  `router.dispatch(req)` in `tokio::time::timeout(30s, …)` aborts
  the future at 30 s. For M2 (only Content-Length bodies) this is
  fine. M4 (SSE — long-lived streams) needs to either disable the
  timeout for streaming responses or scope it to the head + handler
  return (excluding the body stream). **Tracking for M4.**

- **`hyper-util` minor-version churn.** 0.1.x has shipped small API
  refinements between releases. We pin to `0.1` workspace-wide;
  re-validate on each minor bump.

- **`Router::dispatch` is `async fn`, not `Service::call`.** Some
  callers may want a `Service` type directly. M2 ships only the
  `dispatch` interface; if M3 surfaces a need, we add an adapter
  (a free function that returns `service_fn(move |req| router.dispatch(req))`).
