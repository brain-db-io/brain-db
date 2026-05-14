//! Per-request handler used by the accept loop.
//!
//! Bridges the Brain [`Router`] dispatch to hyper's [`Service`]
//! shape. Lives in its own module so the accept loop stays focused
//! on listener / lifecycle concerns.
//!
//! The actual hyper [`Connection`] construction happens inline in
//! [`crate::server::accept::run`] because
//! [`hyper_util::server::graceful::GracefulShutdown::watch`] requires
//! the concrete `Connection<I, S>` value — exposing a typed builder
//! here would require naming hyper's internal `ServiceFn` type, which
//! is not part of the stable surface.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use http::{Response, StatusCode};
use hyper::body::Incoming;

use crate::body::{full, ResponseBody};
use crate::router::Router;

/// Run one request through the router with the per-request timeout
/// from the connection's `ServerLimits`. Returns `Infallible` so it
/// fits hyper's `service_fn` shape directly; errors inside the router
/// are converted to canned status responses by the router itself.
pub(crate) async fn handle_request(
    router: Arc<Router<Incoming>>,
    request_timeout: Duration,
    req: http::Request<Incoming>,
) -> Result<Response<ResponseBody>, Infallible> {
    let dispatched = router.dispatch(req);
    let resp = match tokio::time::timeout(request_timeout, dispatched).await {
        Ok(r) => r,
        Err(_) => timeout_response(request_timeout),
    };
    Ok(resp)
}

pub(crate) fn timeout_response(after: Duration) -> Response<ResponseBody> {
    let body = format!("{{\"error\":\"request timed out after {after:?}\"}}\n");
    Response::builder()
        .status(StatusCode::GATEWAY_TIMEOUT)
        .header("content-type", "application/json; charset=utf-8")
        .body(full(Bytes::from(body)))
        .expect("static response always builds")
}
