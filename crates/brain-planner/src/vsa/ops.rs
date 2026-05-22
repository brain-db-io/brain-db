//! Core HRR ops: random vector, normalize, bind, bundle, unbind.

use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;

use super::errors::VsaError;
use super::fft::{convolve_circular, correlate_circular};

/// HRR vectors are length-`VSA_DIM` real-valued. 512 is the smallest
/// dim where retrieval is reliable for vocabularies in the few-hundreds
/// range; we standardize on it.
pub const VSA_DIM: usize = 512;

/// Concrete vector type for HRR ops. Heap-allocated so callers can
/// move them through codebooks and back without lifetimes.
pub type VsaVec = Vec<f32>;

/// Generate a unitary HRR vector — a real vector whose FFT has unit
/// magnitude at every frequency (uniformly random phases). Unitary
/// vectors are the canonical HRR primitive because they make
/// `unbind(bind(a, b), a) = b` an exact recovery instead of an
/// approximate one, which dramatically improves retrieval reliability
/// at modest dimensions. Deterministic for a given seed so codebooks
/// reproduce across shards.
///
/// Construction: draw uniform phases φ_k ∈ [0, 2π) in the frequency
/// domain, set magnitudes to 1, enforce Hermitian symmetry so the
/// inverse FFT is real, then inverse-FFT. The result is unit-norm by
/// Parseval (after dividing by sqrt(N) — handled by `FftPlanner`'s
/// unnormalized transform and our explicit scaling).
pub fn random_vec(seed: u64) -> VsaVec {
    let n = VSA_DIM;
    let mut state = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut spec: Vec<Complex32> = vec![Complex32::new(0.0, 0.0); n];

    // DC and Nyquist bins must be real for the inverse FFT to be real.
    spec[0] = Complex32::new(1.0, 0.0);
    // For even N, bin N/2 is Nyquist; pick +1 or -1.
    let nyquist_sign = if (next_u64(&mut state) & 1) == 0 {
        1.0
    } else {
        -1.0
    };
    spec[n / 2] = Complex32::new(nyquist_sign, 0.0);

    // Bins 1..N/2 get uniform random phases; their conjugates fill
    // N/2+1..N to enforce Hermitian symmetry.
    for k in 1..n / 2 {
        let phi = std::f32::consts::TAU * next_unit(&mut state);
        let c = Complex32::new(phi.cos(), phi.sin());
        spec[k] = c;
        spec[n - k] = c.conj();
    }

    let mut planner = FftPlanner::<f32>::new();
    let ifft = planner.plan_fft_inverse(n);
    ifft.process(&mut spec);

    // The inverse FFT is unnormalized; divide by sqrt(N) so the
    // result is unit-L2-norm (Parseval). Take the real part — the
    // imaginary part is numerical zero by Hermitian construction.
    let scale = 1.0_f32 / (n as f32).sqrt();
    let mut v: Vec<f32> = spec.iter().map(|c| c.re * scale).collect();
    // Final L2-normalize to absorb fp drift.
    normalize(&mut v).expect("invariant: random_vec produces non-zero norm");
    v
}

/// L2-normalize in place. Returns `DegenerateNorm` if the input norm
/// is below a small epsilon (almost never hit for HRR vectors but
/// guards against degenerate `bundle` outputs).
pub fn normalize(v: &mut VsaVec) -> Result<(), VsaError> {
    let norm_sq: f32 = v.iter().map(|x| x * x).sum();
    let norm = norm_sq.sqrt();
    if !norm.is_finite() || norm < 1e-12 {
        return Err(VsaError::DegenerateNorm { norm });
    }
    let inv = norm.recip();
    for x in v.iter_mut() {
        *x *= inv;
    }
    Ok(())
}

/// Circular convolution: `bind(a, b) = a ⊛ b`. Commutative and
/// associative. The output is the same length as the inputs and is
/// approximately unit-norm when the inputs are unit-norm.
pub fn bind(a: &VsaVec, b: &VsaVec) -> Result<VsaVec, VsaError> {
    if a.len() != VSA_DIM || b.len() != VSA_DIM {
        return Err(VsaError::DimensionMismatch {
            expected: VSA_DIM,
            lhs_len: a.len(),
            rhs_len: b.len(),
        });
    }
    Ok(convolve_circular(a, b))
}

/// Element-wise sum of the inputs, then L2-normalize. Bundling is
/// the HRR analog of set union — the result is similar to each
/// operand (cosine ~ 1/sqrt(k) for k operands of unit norm), which
/// is what lets `unbind` followed by cleanup recover the individual
/// fillers.
pub fn bundle(vs: &[&VsaVec]) -> Result<VsaVec, VsaError> {
    if vs.is_empty() {
        return Err(VsaError::EmptyBundle);
    }
    for v in vs {
        if v.len() != VSA_DIM {
            return Err(VsaError::DimensionMismatch {
                expected: VSA_DIM,
                lhs_len: v.len(),
                rhs_len: VSA_DIM,
            });
        }
    }
    let mut out = vec![0.0_f32; VSA_DIM];
    for v in vs {
        for (o, x) in out.iter_mut().zip(v.iter()) {
            *o += *x;
        }
    }
    normalize(&mut out)?;
    Ok(out)
}

/// Circular correlation: `unbind(c, a) = c ⊛ a⁻¹` where
/// `a⁻¹[k] = a[(n-k) mod n]` is the involution. If `c = bind(a, b)`
/// then `unbind(c, a) ≈ b` (the approximation gets better with
/// dimension; lossy bundling means the recovered vector is noisy and
/// needs codebook cleanup).
pub fn unbind(c: &VsaVec, a: &VsaVec) -> Result<VsaVec, VsaError> {
    if c.len() != VSA_DIM || a.len() != VSA_DIM {
        return Err(VsaError::DimensionMismatch {
            expected: VSA_DIM,
            lhs_len: c.len(),
            rhs_len: a.len(),
        });
    }
    Ok(correlate_circular(c, a))
}

/// Cosine similarity between two equal-length vectors. Public because
/// the codebook + analogy modules use it, and the smoke tests assert
/// over it.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len());
    let mut num = 0.0_f32;
    let mut da = 0.0_f32;
    let mut db = 0.0_f32;
    for (x, y) in a.iter().zip(b.iter()) {
        num += x * y;
        da += x * x;
        db += y * y;
    }
    let denom = da.sqrt() * db.sqrt();
    if denom < 1e-12 {
        0.0
    } else {
        num / denom
    }
}

// ---------- SplitMix64 deterministic PRNG ----------

fn next_u64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Uniform float in [0, 1). 24-bit precision is plenty for HRR
/// phase generation.
fn next_unit(state: &mut u64) -> f32 {
    let bits = (next_u64(state) >> 40) as u32;
    bits as f32 / (1u32 << 24) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_vec_is_unit_norm_and_deterministic() {
        let a = random_vec(42);
        let b = random_vec(42);
        let c = random_vec(43);
        assert_eq!(a.len(), VSA_DIM);
        let norm: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm={norm}");
        assert_eq!(a, b, "same seed must reproduce");
        assert_ne!(a, c, "different seeds must diverge");
    }

    #[test]
    fn bind_is_commutative() {
        let a = random_vec(1);
        let b = random_vec(2);
        let ab = bind(&a, &b).unwrap();
        let ba = bind(&b, &a).unwrap();
        for (x, y) in ab.iter().zip(ba.iter()) {
            assert!((x - y).abs() < 1e-4, "x={x} y={y}");
        }
    }

    #[test]
    fn bind_is_associative() {
        let a = random_vec(11);
        let b = random_vec(22);
        let c = random_vec(33);
        let lhs = bind(&bind(&a, &b).unwrap(), &c).unwrap();
        let rhs = bind(&a, &bind(&b, &c).unwrap()).unwrap();
        // Single-precision FFT, so we compare via cosine — exact
        // equality is unrealistic for two FFT/IFFT round-trips.
        let cos = cosine(&lhs, &rhs);
        assert!(cos > 0.999, "associativity cos={cos}");
    }

    #[test]
    fn unbind_recovers_filler_above_chance_similarity() {
        // Build a single bound pair and unbind. Should recover near-
        // perfectly (cosine > 0.99) because there's no bundling noise.
        let role = random_vec(100);
        let filler = random_vec(200);
        let bound = bind(&role, &filler).unwrap();
        let recovered = unbind(&bound, &role).unwrap();
        let cos = cosine(&recovered, &filler);
        assert!(cos > 0.99, "single-pair unbind cos={cos}");
    }

    #[test]
    fn unbind_from_bundled_triple_above_chance_similarity() {
        // Bundle three role/filler pairs and unbind one role. With
        // bundling noise we expect cos > 0.5, much higher than chance.
        let r_subj = random_vec(1);
        let r_pred = random_vec(2);
        let r_obj = random_vec(3);
        let f_alice = random_vec(101);
        let f_works = random_vec(102);
        let f_acme = random_vec(103);

        let triple = bundle(&[
            &bind(&r_subj, &f_alice).unwrap(),
            &bind(&r_pred, &f_works).unwrap(),
            &bind(&r_obj, &f_acme).unwrap(),
        ])
        .unwrap();

        let cos = cosine(&unbind(&triple, &r_obj).unwrap(), &f_acme);
        assert!(cos > 0.5, "unbind-from-triple cos={cos}");
    }

    #[test]
    fn bundle_then_unbundle_via_cleanup_recovers_each_operand() {
        // Bundle 3 fillers (no role binding) — each should be similar
        // to the bundle with cosine ~ 1/sqrt(3) ≈ 0.577. The "cleanup"
        // here is just argmax over a vocabulary of the operands.
        let f1 = random_vec(7);
        let f2 = random_vec(8);
        let f3 = random_vec(9);
        let bundled = bundle(&[&f1, &f2, &f3]).unwrap();
        for (label, vec) in [("f1", &f1), ("f2", &f2), ("f3", &f3)] {
            let cos = cosine(&bundled, vec);
            assert!(cos > 0.3, "{label} cos={cos}");
        }
    }

    #[test]
    fn bind_rejects_dimension_mismatch() {
        let a = vec![0.0_f32; 4];
        let b = random_vec(0);
        let err = bind(&a, &b).unwrap_err();
        assert!(matches!(err, VsaError::DimensionMismatch { .. }));
    }

    #[test]
    fn bundle_rejects_empty_input() {
        let err = bundle(&[]).unwrap_err();
        assert_eq!(err, VsaError::EmptyBundle);
    }
}
