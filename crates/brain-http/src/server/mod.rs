//! HTTP server: builder, accept loop, per-connection serve, limits,
//! and graceful shutdown.
//!
//! Typical usage:
//!
//! ```no_run
//! use std::sync::Arc;
//! use brain_http::router::Router;
//! use brain_http::server::HttpServer;
//!
//! # async fn example() -> brain_http::Result<()> {
//! let router = Router::new();   // add routes here
//! let bound = HttpServer::bind("127.0.0.1:0".parse().unwrap())
//!     .router(router)
//!     .listen()
//!     .await?;
//! let addr = bound.local_addr();
//! let (handle, run) = bound.into_runner();
//! tokio::spawn(run);
//!
//! // ... do work ...
//!
//! handle.shutdown();
//! # Ok(()) }
//! ```

mod accept;
mod connection;
pub mod limits;
pub mod shutdown;

pub use accept::{run, DEFAULT_DRAIN_TIMEOUT};
pub use limits::{
    ServerLimits, DEFAULT_IDLE_TIMEOUT, DEFAULT_MAX_BODY_BYTES, DEFAULT_MAX_HEADER_BYTES,
    DEFAULT_REQUEST_TIMEOUT,
};
pub use shutdown::{channel, ShutdownHandle, ShutdownSignal};

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::router::Router;
use crate::tcp::{bind as tcp_bind, BindConfig};

/// Builder for an HTTP server. Type-state is unenforced for now;
/// the only required step is providing a `Router`. Limits and
/// bind-config default to spec values.
pub struct HttpServer {
    addr: SocketAddr,
    router: Option<Router<hyper::body::Incoming>>,
    limits: ServerLimits,
    bind_config: BindConfig,
}

impl HttpServer {
    /// Start building a server bound at `addr`. Port `0` is fine — the
    /// kernel picks one and you can read it via
    /// [`BoundServer::local_addr`].
    #[must_use]
    pub fn bind(addr: SocketAddr) -> Self {
        Self {
            addr,
            router: None,
            limits: ServerLimits::default(),
            bind_config: BindConfig::default(),
        }
    }

    /// Set the router. Required.
    #[must_use]
    pub fn router(mut self, router: Router<hyper::body::Incoming>) -> Self {
        self.router = Some(router);
        self
    }

    /// Override the default [`ServerLimits`].
    #[must_use]
    pub fn limits(mut self, limits: ServerLimits) -> Self {
        self.limits = limits;
        self
    }

    /// Override the default [`BindConfig`].
    #[must_use]
    pub fn bind_config(mut self, bind_config: BindConfig) -> Self {
        self.bind_config = bind_config;
        self
    }

    /// Bind the TCP listener but do not start accepting yet. Returns a
    /// [`BoundServer`] from which the caller obtains the local
    /// address (useful in tests with port 0) and the run-future.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::Io`] if the listener cannot be bound.
    pub async fn listen(self) -> crate::Result<BoundServer> {
        let router = self
            .router
            .expect("HttpServer requires a Router — call .router(...) before .listen()");
        let listener = tcp_bind(self.addr, &self.bind_config)?;
        Ok(BoundServer {
            listener,
            router: Arc::new(router),
            limits: Arc::new(self.limits),
            bind_config: Arc::new(self.bind_config),
        })
    }
}

/// A server with its TCP listener already bound. Use
/// [`Self::local_addr`] for the bound port (e.g. when binding port 0
/// in tests), and [`Self::into_runner`] to obtain the
/// `(ShutdownHandle, run_future)` pair.
pub struct BoundServer {
    listener: TcpListener,
    router: Arc<Router<hyper::body::Incoming>>,
    limits: Arc<ServerLimits>,
    bind_config: Arc<BindConfig>,
}

impl BoundServer {
    /// The address the listener is bound to.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`std::io::Error`] if the kernel cannot
    /// report the listener's local address — practically rare, but
    /// possible if the socket has been closed out from under us.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Convert into a `(ShutdownHandle, run_future)` pair. Spawn the
    /// future on tokio; trigger graceful shutdown by calling
    /// `handle.shutdown()`.
    pub fn into_runner(
        self,
    ) -> (
        ShutdownHandle,
        impl std::future::Future<Output = crate::Result<()>>,
    ) {
        let (handle, signal) = channel();
        let fut = accept::run(
            self.listener,
            self.router,
            self.limits,
            self.bind_config,
            signal,
        );
        (handle, fut)
    }
}
