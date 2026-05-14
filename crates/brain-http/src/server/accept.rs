//! Accept loop + graceful drain.
//!
//! Loops on `listener.accept()`, races against the shutdown signal,
//! and tracks every spawned per-connection task via
//! [`hyper_util::server::graceful::GracefulShutdown`] so the loop
//! can wait for them to drain after the signal fires.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use hyper_util::server::graceful::GracefulShutdown;
use tokio::net::TcpListener;
use tracing::{info, warn};

use crate::body::ResponseBody;
use crate::router::Router;
use crate::server::connection::handle_request;
use crate::server::limits::ServerLimits;
use crate::server::shutdown::ShutdownSignal;
use crate::tcp::BindConfig;

/// Default deadline for in-flight connections to drain after the
/// shutdown signal fires. Stragglers beyond this are abandoned.
pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Run the accept loop until `shutdown` fires, then drain in-flight
/// connections.
///
/// # Errors
///
/// Returns [`crate::Error::Io`] if reading the listener's bound
/// address fails. Per-connection errors are logged via `tracing` and
/// do not propagate.
pub async fn run(
    listener: TcpListener,
    router: Arc<Router<hyper::body::Incoming>>,
    limits: Arc<ServerLimits>,
    bind_config: Arc<BindConfig>,
    shutdown: ShutdownSignal,
) -> crate::Result<()> {
    let graceful = GracefulShutdown::new();
    let local_addr: SocketAddr = listener.local_addr()?;
    info!(addr = %local_addr, "brain-http accepting");

    loop {
        tokio::select! {
            biased;
            () = shutdown.wait() => {
                info!(addr = %local_addr, "brain-http shutdown signalled");
                break;
            }
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(p) => p,
                    Err(e) => { warn!(error = %e, "accept failed"); continue; }
                };
                if let Err(e) = crate::tcp::apply_stream_opts(&stream, &bind_config) {
                    warn!(error = %e, peer = %peer, "apply_stream_opts failed");
                }

                let router_for_conn = router.clone();
                let request_timeout = limits.request_timeout;
                let service = service_fn(move |req| {
                    let router = router_for_conn.clone();
                    // Box-pin the per-request future so its `Send`
                    // bound is enforced at this boundary (rather than
                    // inferred through the trait-object dispatch).
                    let fut: std::pin::Pin<
                        Box<
                            dyn Future<
                                    Output = Result<
                                        http::Response<ResponseBody>,
                                        std::convert::Infallible,
                                    >,
                                > + Send,
                        >,
                    > = Box::pin(handle_request(router, request_timeout, req));
                    fut
                });

                let conn = http1::Builder::new()
                    .max_buf_size(limits.max_header_bytes.max(8 * 1024))
                    .keep_alive(true)
                    .serve_connection(TokioIo::new(stream), service);

                let watched = graceful.watch(conn);
                tokio::spawn(async move {
                    if let Err(e) = watched.await {
                        warn!(peer = %peer, error = %e, "connection task ended with error");
                    }
                });
            }
        }
    }

    match tokio::time::timeout(DEFAULT_DRAIN_TIMEOUT, graceful.shutdown()).await {
        Ok(()) => info!(addr = %local_addr, "brain-http drained cleanly"),
        Err(_) => warn!(addr = %local_addr, "brain-http drain timed out; abandoning stragglers"),
    }
    Ok(())
}
