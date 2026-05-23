//! Classifier model configuration.

use std::path::{Path, PathBuf};

use candle_core::{DType, Device};

use super::{
    DEFAULT_GLINER_THRESHOLD, DEFAULT_MAX_SEQ_LEN, DEFAULT_WARMUP_ITERS, NER_MODEL_DIR_NAME,
    NER_MODEL_PATH_ENV, NER_MODEL_REQUIRED_FILES, NER_THRESHOLD_ENV,
};

/// Operator-supplied classifier model configuration.
///
/// Mirrors [`brain_embed`'s `EmbedderConfig`] in shape (same security
/// posture, same path discipline) but targets the GLiNER zero-shot
/// NER model.
#[derive(Debug, Clone)]
pub struct ClassifierConfig {
    /// Directory containing `pytorch_model.bin` / `tokenizer.json` /
    /// `config.json` / `gliner_config.json`. `None` means no
    /// classifier model is configured — the extractor will run in
    /// degraded mode.
    pub model_path: Option<PathBuf>,
    /// Inference device. v1: `Device::Cpu`.
    pub device: Device,
    /// Tensor dtype. v1 default: `DType::F32` — exercises every candle
    /// kernel without F16/F32 broadcasting edge cases that surface in
    /// the GLiNER head's add/matmul mix. F16 halves memory but trips
    /// over a `dtype mismatch in add` panic inside the BiLSTM today;
    /// keep F32 until the head is audited end-to-end for F16 cleanliness.
    pub dtype: DType,
    /// Tokens past this length are truncated. Default 384 (matches
    /// `gliner_config.json`).
    pub max_seq_len: usize,
    /// Warm-up inferences after load. Default 1.
    pub warmup_iters: usize,
    /// Post-sigmoid acceptance threshold. Default 0.5.
    pub threshold: f32,
}

impl ClassifierConfig {
    /// Default config — model unloaded, CPU, F16.
    #[must_use]
    pub fn unloaded() -> Self {
        Self {
            model_path: None,
            device: Device::Cpu,
            dtype: DType::F32,
            max_seq_len: DEFAULT_MAX_SEQ_LEN,
            warmup_iters: DEFAULT_WARMUP_ITERS,
            threshold: DEFAULT_GLINER_THRESHOLD,
        }
    }

    /// Config pointing at an operator-provided model directory.
    #[must_use]
    pub fn with_model_path(path: PathBuf) -> Self {
        Self {
            model_path: Some(path),
            ..Self::unloaded()
        }
    }

    /// True iff the operator configured a model directory.
    #[must_use]
    pub fn has_path(&self) -> bool {
        self.model_path.is_some()
    }

    /// Borrow the configured model directory. Use [`Self::has_path`]
    /// to gate; this returns an empty path when unset so error
    /// messages can still format it without a panic.
    #[must_use]
    pub fn model_path(&self) -> &std::path::Path {
        self.model_path
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new(""))
    }

    /// Resolve the classifier model directory using the same cascade
    /// the bootstrap script writes to:
    ///
    ///   1. `BRAIN_NER_MODEL_PATH` if set to a non-empty value — used
    ///      verbatim. Treated as the operator's explicit choice, so we
    ///      don't probe its contents; load-time validation handles
    ///      missing files and surfaces a clear error.
    ///   2. `$XDG_DATA_HOME/brain/models/gliner-small-v2.1/` — used
    ///      iff the directory contains all four GLiNER files
    ///      (`pytorch_model.bin`, `tokenizer.json`, `config.json`,
    ///      `gliner_config.json`).
    ///   3. `$HOME/.local/share/brain/models/gliner-small-v2.1/` —
    ///      same content check as (2).
    ///
    /// Returns an [`unloaded`](Self::unloaded) config when none of the
    /// candidates resolve to a populated directory. Callers can read
    /// [`default_xdg_model_dir`] for the path Brain *would* have
    /// looked at — useful for diagnostic logs that tell the operator
    /// where to put the model.
    #[must_use]
    pub fn auto_discover() -> Self {
        Self::auto_discover_with(&|k| std::env::var(k).ok(), &|p| p.is_file())
    }

    /// Closure-injection variant of [`Self::auto_discover`] for tests.
    /// `env` looks up environment variables; `is_file` answers whether
    /// a candidate file exists on disk.
    #[must_use]
    pub fn auto_discover_with<E, F>(env: &E, is_file: &F) -> Self
    where
        E: Fn(&str) -> Option<String>,
        F: Fn(&Path) -> bool,
    {
        let threshold = parse_threshold_env(env);
        let mut config = if let Some(raw) = env(NER_MODEL_PATH_ENV) {
            if !raw.is_empty() {
                Self::with_model_path(PathBuf::from(raw))
            } else {
                Self::auto_discover_default(env, is_file)
            }
        } else {
            Self::auto_discover_default(env, is_file)
        };
        config.threshold = threshold;
        config
    }

    fn auto_discover_default<E, F>(env: &E, is_file: &F) -> Self
    where
        E: Fn(&str) -> Option<String>,
        F: Fn(&Path) -> bool,
    {
        let Some(default_dir) = default_xdg_model_dir_with(env) else {
            return Self::unloaded();
        };
        if NER_MODEL_REQUIRED_FILES
            .iter()
            .all(|f| is_file(&default_dir.join(f)))
        {
            Self::with_model_path(default_dir)
        } else {
            Self::unloaded()
        }
    }
}

/// Compute the XDG-default classifier model directory:
/// `$XDG_DATA_HOME/brain/models/gliner-small-v2.1/` (or
/// `$HOME/.local/share/brain/models/gliner-small-v2.1/` if
/// `XDG_DATA_HOME` is unset). Returns `None` only when neither env
/// var is available — exotic environments without a home dir at all.
#[must_use]
pub fn default_xdg_model_dir() -> Option<PathBuf> {
    default_xdg_model_dir_with(&|k| std::env::var(k).ok())
}

/// Parse `BRAIN_NER_THRESHOLD` env var. Returns the parsed value if it
/// is a valid f32 in `[0.0, 1.0]`; otherwise returns the default and
/// warns on invalid (non-numeric) input. Empty / unset returns default
/// silently.
fn parse_threshold_env<E>(env: &E) -> f32
where
    E: Fn(&str) -> Option<String>,
{
    let Some(raw) = env(NER_THRESHOLD_ENV) else {
        return DEFAULT_GLINER_THRESHOLD;
    };
    if raw.is_empty() {
        return DEFAULT_GLINER_THRESHOLD;
    }
    match raw.parse::<f32>() {
        Ok(v) if (0.0..=1.0).contains(&v) => v,
        Ok(v) => {
            tracing::warn!(
                target: "brain_extractors::classifier",
                env_var = NER_THRESHOLD_ENV,
                value = v,
                "threshold outside [0.0, 1.0]; using default"
            );
            DEFAULT_GLINER_THRESHOLD
        }
        Err(e) => {
            tracing::warn!(
                target: "brain_extractors::classifier",
                env_var = NER_THRESHOLD_ENV,
                value = %raw,
                error = %e,
                "threshold env var is not a valid f32; using default"
            );
            DEFAULT_GLINER_THRESHOLD
        }
    }
}

/// Closure-injection variant of [`default_xdg_model_dir`].
#[must_use]
pub fn default_xdg_model_dir_with<E>(env: &E) -> Option<PathBuf>
where
    E: Fn(&str) -> Option<String>,
{
    if let Some(xdg) = env("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        return Some(
            PathBuf::from(xdg)
                .join("brain")
                .join("models")
                .join(NER_MODEL_DIR_NAME),
        );
    }
    let home = env("HOME").filter(|s| !s.is_empty())?;
    Some(
        PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("brain")
            .join("models")
            .join(NER_MODEL_DIR_NAME),
    )
}

impl Default for ClassifierConfig {
    fn default() -> Self {
        Self::unloaded()
    }
}
