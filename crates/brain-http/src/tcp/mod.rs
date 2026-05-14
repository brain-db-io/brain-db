//! TCP socket helpers.
//!
//! Mirrors the bind / per-stream-config pattern in
//! `crates/brain-server/src/network/connection.rs` so brain-http
//! servers behave the same way the existing data-plane listener does.

mod socket;

pub use socket::{apply_stream_opts, bind, BindConfig};
