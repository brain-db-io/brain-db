//! Statement-kind pattern classifier perf bench (sub-task W1.3).
//!
//! Target: 100k ops/sec on a single core. Pure pattern matching over
//! a lowercased copy of the input + a handful of byte scans; no
//! allocation past the one `to_ascii_lowercase` per call.
//!
//! Run: `cargo bench -p brain-extractors --bench statement_kind_pattern`.

use brain_extractors::classify_statement_kind_pattern;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

const SENTENCES: &[&str] = &[
    "Alice works at Acme Corp.",
    "I prefer dark roast coffee.",
    "The all-hands is Friday at 10am.",
    "Bob lives in Berlin.",
    "I love async standups.",
    "The release is scheduled for 2026-06-15.",
    "Priya leads the platform team.",
    "I hate flaky tests.",
    "The deploy occurred at 15:00.",
    "Acme has 200 employees.",
    "I'd rather pair on the design.",
    "Our demo took place on Tuesday.",
    "Frank manages the data team.",
    "My favorite editor is helix.",
    "The standup is at 9:30am.",
    "Grace runs the design org.",
    "I prefer typed languages.",
    "The kickoff is on Monday.",
    "Eve runs the engineering org.",
    "I want a quiet sprint.",
];

fn build_corpus(n: usize) -> Vec<&'static str> {
    let mut out = Vec::with_capacity(n);
    let mut i = 0;
    while out.len() < n {
        out.push(SENTENCES[i % SENTENCES.len()]);
        i += 1;
    }
    out
}

fn bench_pattern(c: &mut Criterion) {
    let corpus = build_corpus(1000);

    c.bench_function("classify_statement_kind_pattern/1000", |b| {
        b.iter(|| {
            let mut acc = 0u32;
            for text in &corpus {
                if let Some((kind, _conf)) = classify_statement_kind_pattern(black_box(text)) {
                    acc = acc.wrapping_add(kind as u32);
                }
            }
            black_box(acc)
        });
    });

    c.bench_function("classify_statement_kind_pattern/single", |b| {
        b.iter(|| {
            black_box(classify_statement_kind_pattern(black_box(
                "The all-hands is Friday at 10am.",
            )))
        });
    });
}

criterion_group!(benches, bench_pattern);
criterion_main!(benches);
