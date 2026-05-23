//! Stream-control requests: CANCEL_STREAM, PING, CLIENT_PONG, BYE.

use rkyv::{Archive, Deserialize, Serialize};

use super::types::CancellationReason;

#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct CancelStreamRequest {
    pub target_stream_id: u32,
    pub reason: CancellationReason,
}

#[derive(Archive, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct PingRequest {
    pub client_timestamp_unix_nanos: u64,
}

/// — `CLIENT_PONG` (despite "Response" in the spec name, it's
/// a client→server frame replying to `SERVER_PING`).
#[derive(Archive, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct ClientPongRequest {
    pub server_timestamp_unix_nanos: u64,
    pub client_timestamp_unix_nanos: u64,
}

#[derive(Archive, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[archive(check_bytes)]
#[archive_attr(derive(Debug))]
pub struct ByeRequest {
    pub reason: Option<String>,
}
