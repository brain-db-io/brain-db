//! Graceful shutdown wiring.
//!
//! Two-part design:
//!
//! 1. [`ShutdownSignal`] — observed by the accept loop and by long-
//!    running handlers. Wraps [`tokio::sync::Notify`] so multiple
//!    awaiters can be notified at once.
//! 2. [`hyper_util::server::graceful::GracefulShutdown`] — tracks
//!    in-flight connection futures so the accept loop can wait for
//!    them to drain after the signal fires. Held internally by
//!    [`crate::server::accept::run`]; not exposed.
//!
//! Callers obtain a `(handle, signal)` pair from [`channel`]. The
//! handle triggers shutdown; the signal is cloned into the accept
//! loop.

use std::sync::Arc;

use tokio::sync::Notify;

/// Create a paired shutdown handle and signal. Drop the handle to
/// cancel the shutdown (it's never triggered); call
/// [`ShutdownHandle::shutdown`] to fire it.
#[must_use]
pub fn channel() -> (ShutdownHandle, ShutdownSignal) {
    let notify = Arc::new(Notify::new());
    (
        ShutdownHandle {
            notify: notify.clone(),
        },
        ShutdownSignal { notify },
    )
}

/// Trigger graceful shutdown. Constructed by [`channel`]; passed to
/// whoever owns the lifecycle (typically `main` or a test).
pub struct ShutdownHandle {
    notify: Arc<Notify>,
}

impl ShutdownHandle {
    /// Signal the accept loop to stop and queued connections to drain.
    /// Idempotent — subsequent calls have no additional effect.
    pub fn shutdown(self) {
        self.notify.notify_waiters();
    }
}

/// Awaitable shutdown signal. Cloned into the accept loop and into
/// any handler that wants to react to shutdown (e.g. an SSE stream
/// closing its event sender). Cheap to clone.
#[derive(Clone)]
pub struct ShutdownSignal {
    notify: Arc<Notify>,
}

impl ShutdownSignal {
    /// Returns once `ShutdownHandle::shutdown()` is called.
    ///
    /// Cancel-safe — drops cleanly if the surrounding future is
    /// aborted.
    pub async fn wait(&self) {
        self.notify.notified().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn signal_fires_on_handle_shutdown() {
        let (handle, signal) = channel();
        let s = signal.clone();
        let waiter = tokio::spawn(async move { s.wait().await });
        // Give the waiter a chance to register.
        tokio::time::sleep(Duration::from_millis(10)).await;
        handle.shutdown();
        tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("did not timeout")
            .expect("waiter did not panic");
    }

    #[tokio::test]
    async fn multiple_signals_all_fire() {
        let (handle, signal) = channel();
        let a = signal.clone();
        let b = signal.clone();
        let c = signal;
        let wa = tokio::spawn(async move { a.wait().await });
        let wb = tokio::spawn(async move { b.wait().await });
        let wc = tokio::spawn(async move { c.wait().await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        handle.shutdown();
        let joined = async {
            wa.await.expect("a");
            wb.await.expect("b");
            wc.await.expect("c");
        };
        tokio::time::timeout(Duration::from_secs(1), joined)
            .await
            .expect("all signaled in time");
    }
}
