//! Minimal hand-rolled blocking HTTP/1.1 GET.
//!
//! brain-cli's needs are tiny: hit the admin server's
//! `/healthz` / `/metrics` endpoints (sub-task 9.13) and read
//! the body. A full reqwest dep (or even reqwest::blocking)
//! pulls hyper / tokio for one syscall's worth of work. The
//! ~80 LOC below covers it.
//!
//! Assumptions:
//! - HTTP/1.1, plain text body (no chunked, no gzip).
//! - Server reads + replies + closes within the timeout.
//! - The admin endpoint never sets `Transfer-Encoding: chunked`
//!   (brain-server's admin.rs sends `Content-Length` always).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Parsed HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

/// Perform an HTTP/1.1 GET. `endpoint` is a `host:port` string,
/// `path` starts with `/`. Returns the parsed status + body.
pub fn get(endpoint: &str, path: &str) -> anyhow::Result<HttpResponse> {
    get_with_timeout(endpoint, path, DEFAULT_TIMEOUT)
}

pub fn get_with_timeout(
    endpoint: &str,
    path: &str,
    timeout: Duration,
) -> anyhow::Result<HttpResponse> {
    let addr = endpoint
        .to_socket_addrs()
        .map_err(|e| anyhow::anyhow!("resolve {endpoint}: {e}"))?
        .next()
        .ok_or_else(|| anyhow::anyhow!("resolve {endpoint}: no addresses"))?;
    let mut stream = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| anyhow::anyhow!("connect {addr}: {e}"))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;

    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {endpoint}\r\nUser-Agent: brain-cli/{ver}\r\nConnection: close\r\nAccept: */*\r\n\r\n",
        ver = env!("CARGO_PKG_VERSION")
    );
    stream.write_all(request.as_bytes())?;
    stream.flush()?;

    let mut raw = Vec::with_capacity(4096);
    stream.read_to_end(&mut raw)?;
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> anyhow::Result<HttpResponse> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("malformed response: no header/body separator"))?;
    let (head, rest) = raw.split_at(split);
    let body = &rest[4..]; // skip the \r\n\r\n

    let head_str =
        std::str::from_utf8(head).map_err(|e| anyhow::anyhow!("non-UTF-8 headers: {e}"))?;
    let status_line = head_str
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("empty response head"))?;
    // "HTTP/1.1 200 OK"
    let mut parts = status_line.splitn(3, ' ');
    let _version = parts.next();
    let status_str = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing status code in: {status_line:?}"))?;
    let status: u16 = status_str
        .parse()
        .map_err(|e| anyhow::anyhow!("bad status {status_str:?}: {e}"))?;

    let body =
        String::from_utf8(body.to_vec()).map_err(|e| anyhow::anyhow!("non-UTF-8 body: {e}"))?;
    Ok(HttpResponse { status, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_response() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";
        let r = parse_response(raw).expect("parse");
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "ok");
    }

    #[test]
    fn parses_non_200() {
        let raw = b"HTTP/1.1 503 Service Unavailable\r\n\r\nnope";
        let r = parse_response(raw).expect("parse");
        assert_eq!(r.status, 503);
        assert_eq!(r.body, "nope");
    }

    #[test]
    fn malformed_response_errors() {
        let raw = b"not-an-http-response";
        let err = parse_response(raw).expect_err("err");
        assert!(err.to_string().contains("no header/body separator"));
    }
}
