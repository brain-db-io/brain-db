//! Bidirectional single-layer LSTM that sits between the DeBERTa
//! backbone and the GLiNER span head.
//!
//! GLiNER v2.1 trains a `nn.LSTM(input_size=768, hidden_size=256,
//! bidirectional=True, batch_first=True)` layer. The pickle exposes
//! eight tensors at the `rnn.lstm.` prefix:
//!
//! ```text
//! weight_ih_l0          [1024, 768]   # forward gates (i, f, g, o stacked)
//! weight_hh_l0          [1024, 256]
//! bias_ih_l0            [1024]
//! bias_hh_l0            [1024]
//! weight_ih_l0_reverse  [1024, 768]   # backward gates
//! weight_hh_l0_reverse  [1024, 256]
//! bias_ih_l0_reverse    [1024]
//! bias_hh_l0_reverse    [1024]
//! ```
//!
//! candle 0.8 has no native bidirectional LSTM — we run two `LSTM`
//! instances (one `Forward`, one `Backward`) at the same VarBuilder
//! prefix; candle's `LSTMConfig::direction` already appends
//! `_reverse` to the parameter names for the backward instance, so a
//! single `vb.pp("lstm")` resolves both sets cleanly.

use candle_core::{Result as CandleResult, Tensor};
use candle_nn::rnn::{Direction, LSTMConfig, LSTM, RNN as _};
use candle_nn::VarBuilder;

use super::GlinerError;

/// Single-layer bidirectional LSTM. Concatenates forward and reverse
/// hidden states along the feature axis, matching PyTorch's
/// `nn.LSTM(..., bidirectional=True)` output convention.
#[derive(Debug)]
pub struct BiLstm {
    forward: LSTM,
    backward: LSTM,
}

impl BiLstm {
    /// Load from a VarBuilder rooted at the LSTM's parent path. The
    /// caller is expected to pass `vb.pp("lstm")` so candle resolves
    /// `weight_ih_l0` / `weight_ih_l0_reverse` directly under that
    /// prefix.
    pub(crate) fn load(
        vb: VarBuilder,
        in_features: usize,
        hidden: usize,
    ) -> Result<Self, GlinerError> {
        let fwd_cfg = LSTMConfig {
            direction: Direction::Forward,
            ..LSTMConfig::default()
        };
        let bwd_cfg = LSTMConfig {
            direction: Direction::Backward,
            ..LSTMConfig::default()
        };
        let forward = LSTM::new(in_features, hidden, fwd_cfg, vb.clone())?;
        let backward = LSTM::new(in_features, hidden, bwd_cfg, vb)?;
        Ok(Self { forward, backward })
    }

    /// Forward pass. Input: `[B, L, in_features]`. Output: `[B, L,
    /// 2 * hidden]` — forward states concatenated with reverse states
    /// along the last axis.
    pub fn forward(&self, x: &Tensor) -> Result<Tensor, GlinerError> {
        let fwd = run_lstm(&self.forward, x)?;
        let reversed = reverse_along_seq(x)?;
        let bwd_reversed = run_lstm(&self.backward, &reversed)?;
        let bwd = reverse_along_seq(&bwd_reversed)?;
        Ok(Tensor::cat(&[&fwd, &bwd], 2)?)
    }
}

/// Run an `LSTM` over `[B, L, F]` input and stack hidden states into
/// `[B, L, hidden]`.
fn run_lstm(lstm: &LSTM, x: &Tensor) -> CandleResult<Tensor> {
    let states = lstm.seq(x)?;
    lstm.states_to_tensor(&states)
}

/// Reverse a `[B, L, F]` tensor along the sequence axis. candle 0.8
/// does not expose `flip`, so we build a descending-index vector and
/// use `index_select`.
fn reverse_along_seq(x: &Tensor) -> CandleResult<Tensor> {
    let seq_len = x.dim(1)?;
    let indices: Vec<u32> = (0..seq_len as u32).rev().collect();
    let idx = Tensor::from_vec(indices, seq_len, x.device())?;
    x.index_select(&idx, 1)
}
