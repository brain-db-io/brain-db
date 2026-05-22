//! Thin wrapper around [`brain_sdk_rust::Client`] that applies the
//! shell's defaults (timeout, pool size).

use std::net::SocketAddr;
use std::time::Duration;

use brain_core::AgentId;
use brain_sdk_rust::{Client, ClientConfig, ClientError, PoolConfig};

/// Shell pool size. Two connections is the minimum for the
/// hold-a-stream-while-running-a-write pattern that `encode --wait`
/// relies on: one connection for the open SUBSCRIBE stream, one
/// for the ENCODE round-trip. A `PoolConfig::single()` would
/// deadlock — the subscribe stream borrows the only connection
/// until we close it, so the encode blocks indefinitely on
/// `acquire`.
const SHELL_POOL_SIZE: u32 = 4;

/// Open a `Client` to `addr` configured with shell defaults.
///
/// - per-op `timeout`
/// - small connection pool (≥ 2) so a held SUBSCRIBE stream
///   doesn't starve concurrent ENCODE / RECALL calls.
pub async fn connect(
    addr: SocketAddr,
    agent_id: AgentId,
    timeout: Duration,
) -> Result<Client, ClientError> {
    let config = ClientConfig::default()
        .with_timeout(timeout)
        .with_pool(PoolConfig::default().with_max(SHELL_POOL_SIZE).with_min(1));
    Client::connect_with(addr, agent_id, config).await
}
