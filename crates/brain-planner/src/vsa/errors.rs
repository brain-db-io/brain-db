//! Error type for the `vsa` module.

use thiserror::Error;

/// All failure modes for HRR ops. The module enforces fixed-dimension
/// vectors up front so once data has been built through `Codebook` or
/// `random_vec`, the algebra is total.
#[derive(Debug, Error, PartialEq)]
pub enum VsaError {
    /// Inputs to a binary op had different lengths or didn't equal
    /// [`crate::vsa::ops::VSA_DIM`].
    #[error("VSA dimension mismatch: expected {expected}, got lhs={lhs_len} rhs={rhs_len}")]
    DimensionMismatch {
        expected: usize,
        lhs_len: usize,
        rhs_len: usize,
    },

    /// `bundle` was called with an empty slice.
    #[error("VSA bundle requires at least one operand")]
    EmptyBundle,

    /// A vector argued to be a unit vector was numerically degenerate
    /// (e.g. zero norm). HRR retrieval depends on stable norms; we
    /// surface this rather than divide by zero.
    #[error("VSA vector has degenerate norm: {norm}")]
    DegenerateNorm { norm: f32 },
}
