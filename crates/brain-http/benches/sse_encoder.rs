//! Criterion bench for `sse::encode` — pure synchronous encoding
//! throughput. No I/O, no async runtime.

use std::time::Duration;

use brain_http::sse::{self, SseEvent};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_sse_encoder(c: &mut Criterion) {
    let small = SseEvent::new().with_id("1").with_data("hello");
    let multi_line = SseEvent::new()
        .with_id("2")
        .with_data("line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10");
    let full = SseEvent::new()
        .with_id("42")
        .with_event("update")
        .with_data("payload-with-some-content")
        .with_retry(Duration::from_millis(5000));

    c.bench_function("sse_encode_small", |b| {
        b.iter(|| sse::encode(black_box(&small)));
    });
    c.bench_function("sse_encode_multi_line", |b| {
        b.iter(|| sse::encode(black_box(&multi_line)));
    });
    c.bench_function("sse_encode_full", |b| {
        b.iter(|| sse::encode(black_box(&full)));
    });
}

criterion_group!(benches, bench_sse_encoder);
criterion_main!(benches);
