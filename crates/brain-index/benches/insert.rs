//! HNSW insert micro-bench attributes ~2-5 ms of the
//! 20 ms RECALL p99 to HNSW search; ENCODE additionally pays an
//! HNSW *insert* cost that this bench measures in isolation.
//!
//! Run with: `cargo bench -p brain-index --bench insert`.
//!
//! The bench measures three latencies:
//!
//! - **`insert_cold`** — into an empty index (no graph layers built).
//! - **`insert_warm_1k`** — into a pre-populated 1 K-vector index.
//! - **`insert_warm_10k`** — into a pre-populated 10 K-vector index.
//!
//! The 10 K size is intentionally smaller than `recall.rs`'s 100 K
//! corpus: insert cost grows ~log(N), so the larger band repeats the
//! same shape of measurement, and CI time stays bounded.

use std::time::Duration;

use brain_core::MemoryId;
use brain_index::{HnswIndex, IndexParams};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

const VECTOR_DIM: usize = 384;

/// Inline xorshift64*; same pattern as `recall.rs`. Deterministic
/// seeding so bench → bench comparisons are apples-to-apples.
struct Xs(u64);

impl Xs {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x.wrapping_mul(0x2545F491_4F6CDD1D) >> 32) as u32
    }
    fn next_f32_unit(&mut self) -> f32 {
        (self.next_u32() as f32) / (u32::MAX as f32)
    }
}

/// Generate a random L2-normalised vector. Production data is
/// l2-normalised post-embedding; the bench mirrors
/// that.
fn random_vector(rng: &mut Xs) -> [f32; VECTOR_DIM] {
    let mut v = [0.0f32; VECTOR_DIM];
    let mut sum_sq = 0.0f32;
    for slot in &mut v {
        // Symmetric around zero so the result lives on the unit sphere.
        let x = rng.next_f32_unit() * 2.0 - 1.0;
        *slot = x;
        sum_sq += x * x;
    }
    let norm = sum_sq.sqrt().max(f32::MIN_POSITIVE);
    for slot in &mut v {
        *slot /= norm;
    }
    v
}

fn populate(idx: &mut HnswIndex<VECTOR_DIM>, n: u64, seed: u64) {
    let mut rng = Xs::new(seed);
    for i in 1..=n {
        let v = random_vector(&mut rng);
        // MemoryId::from_raw — raw constructor used in tests.
        let id = MemoryId::from_raw(u128::from(i));
        idx.insert(id, &v).expect("insert");
    }
}

fn bench_insert(c: &mut Criterion) {
    let params = IndexParams::default();

    let mut group = c.benchmark_group("hnsw_insert");
    // ±10 % variance is normal. Keep sample size modest
    // to bound CI time; criterion's default measurement_time is
    // fine but we cap warm-up.
    group.warm_up_time(Duration::from_secs(2));
    group.measurement_time(Duration::from_secs(5));

    for &n_warm in &[0u64, 1_000, 10_000] {
        let label = match n_warm {
            0 => "cold",
            1_000 => "warm_1k",
            10_000 => "warm_10k",
            _ => "?",
        };
        group.bench_with_input(BenchmarkId::from_parameter(label), &n_warm, |b, &n| {
            // One-shot construction outside the timed closure.
            let mut idx = HnswIndex::<VECTOR_DIM>::new(params).expect("new");
            populate(&mut idx, n, 0xC0FFEE);
            let mut next_id = n + 1;
            // The vector to insert; pre-computed so timing is pure
            // insert cost (random_vector is non-trivial at D=384).
            let mut rng = Xs::new(0xBEEFu64);
            let v = random_vector(&mut rng);

            b.iter(|| {
                let id = MemoryId::from_raw(u128::from(next_id));
                next_id += 1;
                idx.insert(black_box(id), black_box(&v)).expect("insert");
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_insert);
criterion_main!(benches);
