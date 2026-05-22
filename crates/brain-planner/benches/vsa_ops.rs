//! Criterion benches for the HRR algebra in `brain_planner::vsa`.
//!
//! Targets (single-threaded CPU, dev container):
//!
//! - `bind` at D=512 → < 100 µs
//! - `unbind` at D=512 → < 100 µs
//! - `bundle` of 3 vectors at D=512 → < 100 µs
//! - codebook cleanup over 1000 fillers → < 1 ms
//!
//! Run:
//!
//! ```bash
//! cargo bench -p brain-planner --bench vsa_ops -- --quick
//! ```

use criterion::{black_box, criterion_group, criterion_main, Criterion};

use brain_planner::vsa::{
    analogy::{encode_triple, ROLE_OBJECT},
    bind, bundle, random_vec, unbind, Codebook,
};

fn bench_bind(c: &mut Criterion) {
    let a = random_vec(1);
    let b = random_vec(2);
    c.bench_function("vsa/bind D=512", |bn| {
        bn.iter(|| {
            let out = bind(black_box(&a), black_box(&b)).unwrap();
            black_box(out);
        });
    });
}

fn bench_unbind(c: &mut Criterion) {
    let a = random_vec(1);
    let b = random_vec(2);
    let bound = bind(&a, &b).unwrap();
    c.bench_function("vsa/unbind D=512", |bn| {
        bn.iter(|| {
            let out = unbind(black_box(&bound), black_box(&a)).unwrap();
            black_box(out);
        });
    });
}

fn bench_bundle3(c: &mut Criterion) {
    let v1 = random_vec(11);
    let v2 = random_vec(22);
    let v3 = random_vec(33);
    c.bench_function("vsa/bundle k=3 D=512", |bn| {
        bn.iter(|| {
            let out = bundle(black_box(&[&v1, &v2, &v3])).unwrap();
            black_box(out);
        });
    });
}

fn bench_cleanup_1k(c: &mut Criterion) {
    let mut cb = Codebook::new(7);
    for i in 0..1000 {
        let name = format!("filler_{i}");
        let _ = cb.get_or_create_filler(&name);
    }
    // Use a real bound-and-unbound noisy vector so the bench reflects
    // the actual cleanup distribution, not zeros.
    let triple = encode_triple(&mut cb, "filler_42", "filler_7", "filler_999").unwrap();
    let r = cb.get_or_create_role(ROLE_OBJECT).clone();
    let noisy = unbind(&triple, &r).unwrap();
    c.bench_function("vsa/cleanup 1k fillers", |bn| {
        bn.iter(|| {
            let out = cb.cleanup(black_box(&noisy));
            black_box(out);
        });
    });
}

criterion_group!(
    benches,
    bench_bind,
    bench_unbind,
    bench_bundle3,
    bench_cleanup_1k
);
criterion_main!(benches);
