//! End-to-end criterion bench — full GET round-trip over loopback
//! TCP.
//!
//! Most of the measured time is TCP setup/teardown; the bench is a
//! smoke check that establishes a baseline, not an isolated benchmark
//! of our code. Variance on loopback is ±20 % by system load.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use brain_http::body::{full, ResponseBody};
use brain_http::router::Router;
use brain_http::server::HttpServer;
use bytes::Bytes;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use http::{Request, Response, StatusCode};
use hyper::body::Incoming;
use tokio::runtime::Runtime;

async fn healthz(_req: Request<Incoming>) -> brain_http::Result<Response<ResponseBody>> {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain; charset=utf-8")
        .body(full(Bytes::from_static(b"ok")))
        .unwrap())
}

fn start_server(rt: &Runtime) -> (SocketAddr, brain_http::server::ShutdownHandle) {
    rt.block_on(async {
        let router: Router<Incoming> = Router::new().get("/healthz", healthz);
        let bound = HttpServer::bind("127.0.0.1:0".parse().unwrap())
            .router(router)
            .listen()
            .await
            .expect("bind");
        let addr = bound.local_addr().expect("local_addr");
        let (handle, run) = bound.into_runner();
        tokio::spawn(run);
        (addr, handle)
    })
}

fn one_round_trip(addr: SocketAddr) {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .expect("write");
    let mut buf = Vec::with_capacity(256);
    s.read_to_end(&mut buf).expect("read");
    black_box(buf);
}

fn bench_end_to_end(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio rt");
    let (addr, _handle) = start_server(&rt);

    c.bench_function("get_healthz_round_trip", |b| {
        b.iter(|| one_round_trip(addr));
    });

    // _handle dropped at end of fn — kernel reclaims the listener.
}

criterion_group!(benches, bench_end_to_end);
criterion_main!(benches);
