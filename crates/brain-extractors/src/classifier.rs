//! Classifier extractor framework + GLiNER-based zero-shot NER.
//!
//! Labels are passed per `predict()` call (not loaded from a static
//! file) so the classifier tracks the active schema's entity-type
//! qnames verbatim — no per-schema retraining and no OntoNotes
//! relabel layer.
//!
//! ## Load path
//!
//! On [`GlinerClassifier::load`]:
//! 1. Validate `model_path` exists.
//! 2. Delegate to [`crate::gliner::GlinerModel::load`] which validates
//!    `pytorch_model.bin` / `tokenizer.json` / `config.json` /
//!    `gliner_config.json` and adds the `[ENT]` marker token.
//! 3. Compute a BLAKE3 fingerprint over `pytorch_model.bin` for
//!    `ClassifierModel::version`.
//!
//! ## Inference
//!
//! Per dispatch:
//! 1. Strip the namespace prefix from each schema qname
//!    (`"brain:Person"` → `"Person"`) — GLiNER was trained on plain
//!    labels, so feeding it the colon-prefixed qname tokenizes the
//!    namespace into the `[ENT]` label pool and tanks scores.
//! 2. Run the GLiNER forward pass over the input text with the
//!    stripped labels.
//! 3. Remap each returned span's label back to the originating qname
//!    before projection. Spans with labels we didn't ask for are
//!    dropped.
//!
//! ## Degraded state
//!
//! When `ClassifierConfig.model_path == None` or the load fails, the
//! [`ClassifierExtractor`] registers in a **degraded state** — every
//! `run()` dispatch returns
//! `ExtractionResult::skipped(SkippedDisabled, "classifier model not
//! loaded")`. "Not configured" isn't a failure: no inference was
//! attempted, so nothing was dropped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brain_core::knowledge::{ExtractorKind, StatementKind};
use brain_core::{ExtractorId, Memory};
use brain_protocol::schema::ExtractorTarget;
use candle_core::{DType, Device};

use crate::extractor::{
    ExtractionContext, ExtractionFuture, ExtractionResult, ExtractionStatus, Extractor,
    ExtractorError,
};
use crate::gliner::{GlinerConfig, GlinerError, GlinerModel};
use crate::item::{EntityMention, ExtractedItem};

const WEIGHTS_FILE: &str = "pytorch_model.bin";

/// Env-var override for the classifier model directory. When set to a
/// non-empty value, [`ClassifierConfig::auto_discover`] uses this path
/// verbatim and skips the XDG cascade.
pub const NER_MODEL_PATH_ENV: &str = "BRAIN_NER_MODEL_PATH";

/// Environment variable for overriding the classifier confidence
/// threshold. Operators tune this when their domain text scores below
/// the 0.5 default (short names, unusual casing, compound nouns).
/// Parsed as f32 in [0.0, 1.0]; invalid values fall back to the default
/// with a `tracing::warn!`.
pub const NER_THRESHOLD_ENV: &str = "BRAIN_NER_THRESHOLD";

/// Directory name under `$XDG_DATA_HOME/brain/models/` populated by
/// `scripts/bootstrap-model.sh` for the GLiNER NER model.
pub const NER_MODEL_DIR_NAME: &str = "gliner-small-v2.1";

/// Files the bootstrap script writes for a healthy GLiNER install.
/// Used by the XDG-discovery probe to decide whether to point the
/// classifier at the default directory or fall back to the unloaded
/// (degraded) config. Tokenizer-side companions (`spm.model`) are
/// optional from the loader's perspective so they're not on this list.
pub const NER_MODEL_REQUIRED_FILES: &[&str] = &[
    "pytorch_model.bin",
    "tokenizer.json",
    "config.json",
    "gliner_config.json",
];

const DEFAULT_MAX_SEQ_LEN: usize = 384;
const DEFAULT_WARMUP_ITERS: usize = 1;
const DEFAULT_GLINER_THRESHOLD: f32 = 0.5;

/// Strip the first namespace prefix from a schema qname. `"brain:Person"`
/// becomes `"Person"`; an input without a `':'` is returned unchanged.
/// Only splits on the first colon so `"a:b:c"` becomes `"b:c"`.
fn simple_label(qname: &str) -> &str {
    qname.split_once(':').map(|(_, rest)| rest).unwrap_or(qname)
}

// ---------------------------------------------------------------------------
// Config.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Trait + output.
// ---------------------------------------------------------------------------

/// Object-safe model surface. v1 ships [`GlinerClassifier`]; future
/// kinds (custom feature extractors, ONNX, etc.) plug in here.
pub trait ClassifierModel: Send + Sync {
    /// Run NER with the given label set. Labels are the plain (de-prefixed)
    /// entity type names the active schema declared, e.g. `["Person",
    /// "Organization"]`. Returned spans carry the same plain label; the
    /// caller (`ClassifierExtractor::run`) remaps back to the schema qname.
    fn predict(&self, text: &str, labels: &[&str]) -> Result<Vec<ClassifiedSpan>, ExtractorError>;

    /// Run NER over a batch of `(text, labels)` pairs amortising the
    /// model forward pass across `inputs.len()` rows. The classifier
    /// tier dominates extractor latency (single-input GLiNER inference
    /// is ~4s on CPU); batching the backbone GEMM is the lever that
    /// keeps the worker's drain throughput ahead of the encode arrival
    /// rate.
    ///
    /// The default impl falls back to per-row [`predict`] for any
    /// model that doesn't override; downstream callers can still call
    /// `predict_batch` unconditionally. Real impls (`GlinerClassifier`)
    /// override to run a single batched forward pass.
    fn predict_batch(
        &self,
        inputs: &[(&str, &[&str])],
    ) -> Result<Vec<Vec<ClassifiedSpan>>, ExtractorError> {
        let mut out = Vec::with_capacity(inputs.len());
        for (text, labels) in inputs {
            out.push(self.predict(text, labels)?);
        }
        Ok(out)
    }

    /// Pinned model identifier — BLAKE3 fingerprint hex truncated to
    /// 16 bytes. Bumps when weights change.
    fn version(&self) -> &str;
}

/// Output shape from a zero-shot classifier. `label` is verbatim from
/// the labels passed to [`ClassifierModel::predict`] — i.e. the plain
/// (de-prefixed) entity name. The extractor pipeline remaps it back to
/// the schema qname before downstream projection.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedSpan {
    /// Plain entity-type label (e.g. `"Person"`).
    pub label: String,
    /// Span text sliced from the original input.
    pub text: String,
    /// Inclusive character offset of the span start.
    pub char_start: usize,
    /// Exclusive character offset of the span end.
    pub char_end: usize,
    /// Post-sigmoid confidence in `[0, 1]`.
    pub confidence: f32,
}

// ---------------------------------------------------------------------------
// GlinerClassifier — real model load + inference.
// ---------------------------------------------------------------------------

/// GLiNER-backed [`ClassifierModel`]. Wraps a loaded
/// [`crate::gliner::GlinerModel`] plus a fingerprint over the
/// `pytorch_model.bin` blob.
pub struct GlinerClassifier {
    model: Arc<GlinerModel>,
    fingerprint_hex: String,
}

impl std::fmt::Debug for GlinerClassifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlinerClassifier")
            .field("fingerprint", &self.fingerprint_hex)
            .finish()
    }
}

impl GlinerClassifier {
    /// Load from an operator-provided model directory. Returns
    /// [`ExtractorError::ModelNotFound`] when `model_path` is unset or
    /// the directory is missing; load failures (corrupt weights, bad
    /// tokenizer, missing `[ENT]` token) map to
    /// [`ExtractorError::InferenceFailed`].
    pub fn load(config: &ClassifierConfig) -> Result<Self, ExtractorError> {
        if !matches!(config.device, Device::Cpu) {
            return Err(ExtractorError::InferenceFailed {
                reason: "only Device::Cpu is supported in v1".into(),
            });
        }

        let dir = config
            .model_path
            .as_ref()
            .ok_or_else(|| ExtractorError::ModelNotFound {
                id: "model_path unset".into(),
            })?;
        if !dir.is_dir() {
            return Err(ExtractorError::ModelNotFound {
                id: format!("not a directory: {}", dir.display()),
            });
        }

        let gliner_config = GlinerConfig {
            max_len: config.max_seq_len,
            threshold: config.threshold,
            device: config.device.clone(),
            dtype: config.dtype,
            ..GlinerConfig::default()
        };

        let model = GlinerModel::load(dir, gliner_config).map_err(map_gliner_load_err)?;
        let fingerprint_hex = fingerprint_weights(&dir.join(WEIGHTS_FILE))?;

        tracing::info!(
            target: "brain_extractors::classifier",
            model_dir = %dir.display(),
            fingerprint = %fingerprint_hex,
            "loaded gliner classifier",
        );

        Ok(Self {
            model: Arc::new(model),
            fingerprint_hex,
        })
    }
}

impl ClassifierModel for GlinerClassifier {
    fn predict(&self, text: &str, labels: &[&str]) -> Result<Vec<ClassifiedSpan>, ExtractorError> {
        let spans = self.model.predict(text, labels).map_err(|e| match e {
            GlinerError::TooManyLabels { .. } | GlinerError::InputTooLong { .. } => {
                ExtractorError::InferenceFailed {
                    reason: e.to_string(),
                }
            }
            other => ExtractorError::InferenceFailed {
                reason: format!("gliner predict: {other}"),
            },
        })?;
        Ok(spans
            .into_iter()
            .map(|s| ClassifiedSpan {
                label: s.label,
                text: s.text,
                char_start: s.char_start,
                char_end: s.char_end,
                confidence: s.score,
            })
            .collect())
    }

    fn predict_batch(
        &self,
        inputs: &[(&str, &[&str])],
    ) -> Result<Vec<Vec<ClassifiedSpan>>, ExtractorError> {
        let raw = self.model.predict_batch(inputs).map_err(|e| match e {
            GlinerError::TooManyLabels { .. } | GlinerError::InputTooLong { .. } => {
                ExtractorError::InferenceFailed {
                    reason: e.to_string(),
                }
            }
            other => ExtractorError::InferenceFailed {
                reason: format!("gliner predict_batch: {other}"),
            },
        })?;
        Ok(raw
            .into_iter()
            .map(|spans| {
                spans
                    .into_iter()
                    .map(|s| ClassifiedSpan {
                        label: s.label,
                        text: s.text,
                        char_start: s.char_start,
                        char_end: s.char_end,
                        confidence: s.score,
                    })
                    .collect()
            })
            .collect())
    }

    fn version(&self) -> &str {
        &self.fingerprint_hex
    }
}

fn map_gliner_load_err(e: GlinerError) -> ExtractorError {
    match e {
        GlinerError::MissingFile(p) => ExtractorError::ModelNotFound { id: p },
        other => ExtractorError::InferenceFailed {
            reason: other.to_string(),
        },
    }
}

/// BLAKE3 fingerprint of the weights file truncated to 16 bytes,
/// rendered as hex. Bumps the `ClassifierModel::version` value
/// whenever the operator swaps in fresh weights so downstream audit
/// rows can tell two extractor outputs apart.
fn fingerprint_weights(path: &std::path::Path) -> Result<String, ExtractorError> {
    let bytes = std::fs::read(path).map_err(|e| ExtractorError::ModelNotFound {
        id: format!("read {} failed: {e}", path.display()),
    })?;
    let hash = blake3::hash(&bytes);
    let bytes16: [u8; 16] = hash.as_bytes()[..16]
        .try_into()
        .expect("blake3 >= 16 bytes");
    let mut hex = String::with_capacity(32);
    for b in &bytes16 {
        hex.push_str(&format!("{b:02x}"));
    }
    Ok(hex)
}

// ---------------------------------------------------------------------------
// ClassifierExtractor — implements Extractor.
// ---------------------------------------------------------------------------

/// Wires a [`ClassifierModel`] to the extraction pipeline. Carries the
/// active schema's entity-type qnames (`target_labels`) as the label
/// set to pass on every `predict()` call.
pub struct ClassifierExtractor {
    id: ExtractorId,
    name: String,
    target: ExtractorTarget,
    extractor_version: u32,
    confidence_threshold: f32,
    model: Option<Arc<dyn ClassifierModel>>,
    /// Label set passed to `ClassifierModel::predict` on every
    /// dispatch. Snapshotted at shard startup from the schema's
    /// entity-type registry. Empty → degraded (no labels = nothing
    /// to classify against).
    target_labels: Arc<Vec<String>>,
    /// Reason captured at construction time when the model couldn't
    /// load — surfaces in every degraded dispatch.
    degraded_reason: Option<String>,
}

impl ClassifierExtractor {
    /// Fully-wired extractor with a loaded model + non-empty label
    /// snapshot.
    pub fn new(
        id: ExtractorId,
        name: String,
        target: ExtractorTarget,
        extractor_version: u32,
        confidence_threshold: f32,
        model: Arc<dyn ClassifierModel>,
        target_labels: Arc<Vec<String>>,
    ) -> Self {
        Self {
            id,
            name,
            target,
            extractor_version,
            confidence_threshold,
            model: Some(model),
            target_labels,
            degraded_reason: None,
        }
    }

    /// Degraded extractor — no model loaded. Every dispatch writes a
    /// `SkippedDisabled` audit row with the captured reason.
    pub fn degraded(
        id: ExtractorId,
        name: String,
        target: ExtractorTarget,
        extractor_version: u32,
        confidence_threshold: f32,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            id,
            name,
            target,
            extractor_version,
            confidence_threshold,
            model: None,
            target_labels: Arc::new(Vec::new()),
            degraded_reason: Some(reason.into()),
        }
    }

    /// True iff a model is wired in.
    pub fn is_loaded(&self) -> bool {
        self.model.is_some()
    }

    /// Snapshot of the labels passed to every `predict()` call.
    pub fn target_labels(&self) -> &[String] {
        &self.target_labels
    }

    /// Build the (`simple_labels`, `qname_by_simple`) pair the classifier
    /// path uses to feed GLiNER plain labels and then remap them back
    /// to qnames on the way out. Pulled out of `run` so the batched
    /// path can share it without copy-paste.
    fn resolve_labels(&self) -> (Vec<String>, HashMap<String, String>) {
        let mut seen = std::collections::HashSet::new();
        let mut collision = false;
        for q in self.target_labels.iter() {
            if !seen.insert(simple_label(q.as_str())) {
                collision = true;
                break;
            }
        }
        if collision {
            tracing::warn!(
                target: "brain_extractors::classifier",
                "simple-label collision across namespaces; passing underscore-encoded qnames to GLiNER — accuracy degraded"
            );
            let simples: Vec<String> = self
                .target_labels
                .iter()
                .map(|q| q.replace(':', "_"))
                .collect();
            let map: HashMap<String, String> = self
                .target_labels
                .iter()
                .zip(simples.iter())
                .map(|(q, s)| (s.clone(), q.clone()))
                .collect();
            (simples, map)
        } else {
            let simples: Vec<String> = self
                .target_labels
                .iter()
                .map(|q| simple_label(q.as_str()).to_string())
                .collect();
            let map: HashMap<String, String> = self
                .target_labels
                .iter()
                .map(|q| (simple_label(q.as_str()).to_string(), q.clone()))
                .collect();
            (simples, map)
        }
    }

    /// Project a vector of GLiNER spans for one memory into the
    /// extractor's `ExtractedItem` output, applying confidence
    /// threshold and the simple→qname remap. Shared between the
    /// single-input `run` and batched `run_batch` paths.
    fn project_spans(
        &self,
        spans: Vec<ClassifiedSpan>,
        qname_by_label: &HashMap<String, String>,
    ) -> Vec<ExtractedItem> {
        let mut items = Vec::new();
        for mut span in spans {
            if span.confidence < self.confidence_threshold {
                continue;
            }
            match qname_by_label.get(span.label.as_str()) {
                Some(qname) => span.label = qname.clone(),
                None => continue,
            }
            if let Some(item) = self.project(span) {
                items.push(item);
            }
        }
        items
    }

    fn project(&self, span: ClassifiedSpan) -> Option<ExtractedItem> {
        match &self.target {
            ExtractorTarget::Entity { .. } | ExtractorTarget::EntityOrStatement => {
                // `span.label` is already the fully-qualified qname — the
                // run() loop remaps the model's simple label back before
                // calling us.
                Some(ExtractedItem::EntityMention(EntityMention {
                    entity_type_qname: span.label,
                    text: span.text,
                    start: span.char_start,
                    end: span.char_end,
                    confidence: span.confidence,
                    extractor_id: self.id.raw(),
                    extractor_version: self.extractor_version,
                }))
            }
            // Statement / Relation classifier targets are not the
            // classifier tier's job — extractors targeting those kinds
            // emit nothing without failing.
            _ => None,
        }
    }
}

impl Extractor for ClassifierExtractor {
    fn id(&self) -> ExtractorId {
        self.id
    }

    fn kind(&self) -> ExtractorKind {
        ExtractorKind::Classifier
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn extractor_version(&self) -> u32 {
        self.extractor_version
    }

    fn is_wired(&self) -> bool {
        self.model.is_some()
    }

    fn run<'a>(&'a self, ctx: &'a ExtractionContext<'a>, mem: &'a Memory) -> ExtractionFuture<'a> {
        Box::pin(async move {
            let at = ctx.now_unix_nanos;
            let text = mem.text.as_deref().unwrap_or("");

            let Some(model) = self.model.as_ref() else {
                let reason = self
                    .degraded_reason
                    .as_deref()
                    .unwrap_or("classifier model not loaded");
                return ExtractionResult::skipped(ExtractionStatus::SkippedDisabled, reason, at);
            };

            if self.target_labels.is_empty() {
                return ExtractionResult::skipped(
                    ExtractionStatus::SkippedDisabled,
                    "no entity-type labels declared by the active schema",
                    at,
                );
            }

            let (label_owned, qname_by_label) = self.resolve_labels();
            let label_refs: Vec<&str> = label_owned.iter().map(String::as_str).collect();
            let spans = match model.predict(text, &label_refs) {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!(
                        target: "brain_extractors::classifier",
                        error = %e,
                        "gliner predict failed"
                    );
                    return ExtractionResult::failure(e.to_string(), at, at);
                }
            };

            for s in &spans {
                tracing::debug!(
                    target: "brain_extractors::classifier",
                    label = %s.label,
                    text = %s.text,
                    score = s.confidence,
                    threshold = self.confidence_threshold,
                    accepted = s.confidence >= self.confidence_threshold,
                    "gliner span"
                );
            }

            let items = self.project_spans(spans, &qname_by_label);
            ExtractionResult::success(items, at, at)
        })
    }

    fn run_batch<'a>(
        &'a self,
        ctx: &'a ExtractionContext<'a>,
        mems: &'a [brain_core::Memory],
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<ExtractionResult>> + Send + 'a>>
    {
        Box::pin(async move {
            let at = ctx.now_unix_nanos;
            if mems.is_empty() {
                return Vec::new();
            }

            // No model / no labels: every row gets the same skipped
            // result, no model work.
            let Some(model) = self.model.as_ref() else {
                let reason = self
                    .degraded_reason
                    .as_deref()
                    .unwrap_or("classifier model not loaded");
                return mems
                    .iter()
                    .map(|_| {
                        ExtractionResult::skipped(ExtractionStatus::SkippedDisabled, reason, at)
                    })
                    .collect();
            };
            if self.target_labels.is_empty() {
                return mems
                    .iter()
                    .map(|_| {
                        ExtractionResult::skipped(
                            ExtractionStatus::SkippedDisabled,
                            "no entity-type labels declared by the active schema",
                            at,
                        )
                    })
                    .collect();
            }

            let (label_owned, qname_by_label) = self.resolve_labels();
            let label_refs: Vec<&str> = label_owned.iter().map(String::as_str).collect();

            // Hold an owned String for each memory's text so the
            // `&str` refs we hand to predict_batch outlive the call.
            let texts: Vec<&str> = mems
                .iter()
                .map(|m| m.text.as_deref().unwrap_or(""))
                .collect();
            let batch_inputs: Vec<(&str, &[&str])> =
                texts.iter().map(|t| (*t, label_refs.as_slice())).collect();

            let batched = match model.predict_batch(&batch_inputs) {
                Ok(b) => b,
                Err(e) => {
                    tracing::error!(
                        target: "brain_extractors::classifier",
                        error = %e,
                        batch_size = mems.len(),
                        "gliner predict_batch failed"
                    );
                    let msg = e.to_string();
                    return mems
                        .iter()
                        .map(|_| ExtractionResult::failure(msg.clone(), at, at))
                        .collect();
                }
            };

            debug_assert_eq!(batched.len(), mems.len());
            let mut out = Vec::with_capacity(mems.len());
            for spans in batched {
                for s in &spans {
                    tracing::debug!(
                        target: "brain_extractors::classifier",
                        label = %s.label,
                        text = %s.text,
                        score = s.confidence,
                        threshold = self.confidence_threshold,
                        accepted = s.confidence >= self.confidence_threshold,
                        "gliner span"
                    );
                }
                let items = self.project_spans(spans, &qname_by_label);
                out.push(ExtractionResult::success(items, at, at));
            }
            out
        })
    }
}

// ---------------------------------------------------------------------------
// Statement-kind pattern classifier.
// ---------------------------------------------------------------------------

/// Default confidence threshold above which the pattern classifier's
/// kind decision is treated as authoritative. The pipeline accepts any
/// `Some((kind, conf))` where `conf >= STATEMENT_KIND_PATTERN_THRESHOLD`
/// and otherwise defers to downstream (LLM or stored default).
pub const STATEMENT_KIND_PATTERN_THRESHOLD: f32 = 0.7;

/// Classify a sentence into Fact / Preference / Event using
/// deterministic patterns. Returns `Some((kind, confidence))` for clean
/// matches and `None` when no pattern fires — the caller should defer
/// to the LLM tier in that case.
///
/// The function is the cheap pre-filter that keeps the LLM tier off the
/// 70-90 % of statements whose kind is unambiguous from surface cues.
/// It runs in microseconds and is allocation-free on the hot path
/// (lowercased text is borrowed once; everything else is byte-scan).
///
/// Order matters: Preference cues beat Event cues which beat Fact cues.
/// A sentence carrying both "I prefer" and a date is treated as a
/// Preference — preference statements naturally embed temporal context
/// ("I prefer my coffee black since 2019") and the preference framing
/// dominates the truth-condition.
pub fn classify_statement_kind_pattern(text: &str) -> Option<(StatementKind, f32)> {
    let lower = text.to_ascii_lowercase();
    let lower = lower.as_str();

    if let Some(score) = score_preference(lower) {
        return Some((StatementKind::Preference, score));
    }
    if let Some(score) = score_event(lower) {
        return Some((StatementKind::Event, score));
    }
    if let Some(score) = score_fact(lower) {
        return Some((StatementKind::Fact, score));
    }
    None
}

/// First-person preference and dispreference cues. We anchor on the
/// "I" pronoun + a preference verb so the pattern doesn't fire on
/// third-person reports of someone else's preference (which read more
/// like Facts: "Alice prefers tea" → Fact about Alice's preference).
fn score_preference(text: &str) -> Option<f32> {
    const STRONG_FIRST_PERSON: &[&str] = &[
        "i prefer",
        "i'd prefer",
        "i would prefer",
        "i like",
        "i love",
        "i hate",
        "i dislike",
        "i don't like",
        "i do not like",
        "i don't want",
        "i do not want",
        "i want",
        "i wish",
        "i'd rather",
        "i would rather",
        "i enjoy",
        "i can't stand",
        "i cannot stand",
        "my favorite",
        "my favourite",
        "my preference",
    ];
    if STRONG_FIRST_PERSON.iter().any(|cue| text.contains(cue)) {
        return Some(0.9);
    }
    // Weaker third-person preference cues. Lower confidence — the
    // sentence might be a Fact reporting someone else's preference,
    // but if the predicate noun is unambiguous we still call it.
    const SOFT_PREFERENCE: &[&str] = &[
        "favorite ",
        "favourite ",
        "preferred ",
        "preferences ",
        "preference ",
    ];
    if SOFT_PREFERENCE.iter().any(|cue| text.contains(cue)) {
        return Some(0.75);
    }
    None
}

/// Event cues: a temporal anchor (explicit date/time or relative time
/// word) AND either an event verb or a scheduled-action noun. Either
/// alone is insufficient — "she works in 2024" is a Fact, "the meeting
/// happened" without a date may be a Fact about a past event. The
/// combination is the discriminator.
fn score_event(text: &str) -> Option<f32> {
    let has_temporal = has_explicit_date(text)
        || has_clock_time(text)
        || has_relative_time(text)
        || has_year_anchor(text);
    if !has_temporal {
        return None;
    }
    const EVENT_VERBS: &[&str] = &[
        "happened",
        "occurred",
        "took place",
        "is scheduled",
        "scheduled for",
        "scheduled on",
        "scheduled at",
        "will happen",
        "will occur",
        "will take place",
        "starts at",
        "starts on",
        "begins at",
        "begins on",
        "ends at",
        "ends on",
        "is at ",
        "is on ",
        " at ",
        " on ",
    ];
    const EVENT_NOUNS: &[&str] = &[
        "meeting",
        "all-hands",
        "all hands",
        "standup",
        "stand-up",
        "kickoff",
        "kick-off",
        "release",
        "launch",
        "demo",
        "review",
        "deadline",
        "conference",
        "summit",
        "workshop",
        "ceremony",
        "appointment",
        "event",
        "interview",
        "call",
        "sync",
        "1:1",
        "one-on-one",
        "deploy",
        "deployment",
        "outage",
        "incident",
        "milestone",
        "anniversary",
        "birthday",
        "wedding",
        "flight",
        "trip",
        "visit",
    ];
    let has_verb = EVENT_VERBS.iter().any(|v| text.contains(v));
    let has_noun = EVENT_NOUNS.iter().any(|n| text.contains(n));
    if has_verb && has_noun {
        return Some(0.9);
    }
    if has_verb || has_noun {
        return Some(0.8);
    }
    None
}

/// Fact cues: a copula or attribution verb without preference / event
/// markers. We've already ruled the preference / event branches out by
/// the time we get here, so the bar is lower — any plausible
/// declarative sentence anchored on "X is/are/has/works/lives/owns" is
/// a Fact.
fn score_fact(text: &str) -> Option<f32> {
    const COPULA: &[&str] = &[
        " is ",
        " are ",
        " was ",
        " were ",
        " has ",
        " have ",
        " had ",
        " works at",
        " works for",
        " works on",
        " works in",
        " lives in",
        " lives at",
        " lives on",
        " owns ",
        " runs ",
        " manages ",
        " leads ",
        " founded ",
        " co-founded",
        " reports to",
        " belongs to",
        " contains ",
        " consists of",
        " includes ",
    ];
    if COPULA.iter().any(|c| text.contains(c)) {
        return Some(0.75);
    }
    None
}

/// Returns true if `text` contains an ISO-ish date (`2024-05-16`, `5/16/2024`,
/// `16-05-2024`).
fn has_explicit_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    // YYYY-MM-DD or DD-MM-YYYY or YYYY/MM/DD with 1-4 digits per group.
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let group_len = i - start;
            if (group_len == 4 || group_len == 1 || group_len == 2)
                && i < bytes.len()
                && (bytes[i] == b'-' || bytes[i] == b'/')
            {
                // Look ahead: second group of digits, separator, third group.
                let sep = bytes[i];
                i += 1;
                let g2 = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let g2_len = i - g2;
                if (1..=4).contains(&g2_len)
                    && i < bytes.len()
                    && bytes[i] == sep
                    && i + 1 < bytes.len()
                    && bytes[i + 1].is_ascii_digit()
                {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Returns true if `text` contains a clock-style time like `3pm`,
/// `10am`, `3:30pm`, `15:00`.
fn has_clock_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    // h(h):mm or h(h)am/pm.
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let dlen = i - start;
            if dlen == 0 || dlen > 2 {
                continue;
            }
            // `:mm`
            if i + 2 < bytes.len()
                && bytes[i] == b':'
                && bytes[i + 1].is_ascii_digit()
                && bytes[i + 2].is_ascii_digit()
            {
                return true;
            }
            // `am` / `pm`
            if i + 1 < bytes.len() {
                let suffix = &bytes[i..(i + 2).min(bytes.len())];
                if suffix == b"am" || suffix == b"pm" {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Relative-time tokens: "yesterday", "tomorrow", "next monday",
/// "last friday", "this week", weekday names following "on".
fn has_relative_time(text: &str) -> bool {
    const REL: &[&str] = &[
        "yesterday",
        "tomorrow",
        "tonight",
        "this morning",
        "this afternoon",
        "this evening",
        "next week",
        "next month",
        "next year",
        "last week",
        "last month",
        "last year",
        "next monday",
        "next tuesday",
        "next wednesday",
        "next thursday",
        "next friday",
        "next saturday",
        "next sunday",
        "last monday",
        "last tuesday",
        "last wednesday",
        "last thursday",
        "last friday",
        "last saturday",
        "last sunday",
        "on monday",
        "on tuesday",
        "on wednesday",
        "on thursday",
        "on friday",
        "on saturday",
        "on sunday",
        " jan ",
        " feb ",
        " mar ",
        " apr ",
        " may ",
        " jun ",
        " jul ",
        " aug ",
        " sep ",
        " oct ",
        " nov ",
        " dec ",
        " january ",
        " february ",
        " march ",
        " april ",
        " june ",
        " july ",
        " august ",
        " september ",
        " october ",
        " november ",
        " december ",
    ];
    REL.iter().any(|cue| text.contains(cue))
}

/// Standalone four-digit year (1900-2099) preceded by " in " — covers
/// "in 2024", "in 1999". Excluded from `has_explicit_date` which
/// requires a date separator.
fn has_year_anchor(text: &str) -> bool {
    let bytes = text.as_bytes();
    // Walk byte by byte; not Unicode-aware but every relevant ASCII
    // year token survives lowercasing.
    let needle = b" in ";
    let mut i = 0;
    while i + needle.len() + 4 <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let y = i + needle.len();
            if bytes[y..y + 4].iter().all(|b| b.is_ascii_digit())
                && (bytes[y] == b'1' || bytes[y] == b'2')
            {
                // Trailing boundary: end of string or non-digit.
                if y + 4 == bytes.len() || !bytes[y + 4].is_ascii_digit() {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ExtractorRegistry;
    use brain_core::{AgentId, ContextId, MemoryId, MemoryKind, Salience};

    fn entity_target() -> ExtractorTarget {
        ExtractorTarget::Entity {
            entity_type: "brain:Person".into(),
        }
    }

    fn memory(text: &str) -> Memory {
        Memory {
            id: MemoryId::pack(0, 1, 0),
            agent: AgentId::new(),
            context: ContextId(0),
            kind: MemoryKind::Episodic,
            salience: Salience::default(),
            text: Some(text.into()),
            created_at_unix_ms: 0,
            last_accessed_at_unix_ms: 0,
        }
    }

    fn ctx<'a>(reg: &'a ExtractorRegistry) -> ExtractionContext<'a> {
        ExtractionContext {
            schema_version: 1,
            now_unix_nanos: 0,
            registry: reg,
            prior_tier_items: None,
            extractor_context: None,
        }
    }

    fn default_labels() -> Arc<Vec<String>> {
        Arc::new(vec![
            "brain:Person".into(),
            "brain:Organization".into(),
            "brain:Project".into(),
            "brain:Event".into(),
            "brain:Place".into(),
            "brain:Concept".into(),
        ])
    }

    // ----- ClassifierConfig defaults ----------------------------------------

    #[test]
    fn config_default_disables_classifier() {
        let c = ClassifierConfig::default();
        assert!(c.model_path.is_none());
        assert!(matches!(c.device, Device::Cpu));
        assert_eq!(c.max_seq_len, DEFAULT_MAX_SEQ_LEN);
        assert!((c.threshold - DEFAULT_GLINER_THRESHOLD).abs() < 1e-6);
    }

    #[test]
    fn config_with_model_path_keeps_defaults() {
        let c = ClassifierConfig::with_model_path("/tmp/ner".into());
        assert_eq!(
            c.model_path.as_deref(),
            Some(std::path::Path::new("/tmp/ner"))
        );
        assert_eq!(c.max_seq_len, DEFAULT_MAX_SEQ_LEN);
    }

    // ----- GlinerClassifier::load error paths -------------------------------

    #[test]
    fn load_returns_error_when_path_is_none() {
        let cfg = ClassifierConfig::unloaded();
        let err = GlinerClassifier::load(&cfg).unwrap_err();
        assert!(
            matches!(err, ExtractorError::ModelNotFound { ref id }
                if id.contains("model_path unset")),
            "got {err:?}"
        );
    }

    #[test]
    fn load_returns_error_when_directory_missing() {
        let cfg = ClassifierConfig::with_model_path("/this/does/not/exist/420".into());
        let err = GlinerClassifier::load(&cfg).unwrap_err();
        assert!(matches!(err, ExtractorError::ModelNotFound { .. }));
    }

    #[test]
    fn load_returns_error_when_required_files_missing() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = ClassifierConfig::with_model_path(dir.path().to_path_buf());
        let err = GlinerClassifier::load(&cfg).unwrap_err();
        assert!(matches!(err, ExtractorError::ModelNotFound { .. }));
    }

    // ----- Degraded extractor dispatch --------------------------------------

    fn degraded_ext() -> ClassifierExtractor {
        ClassifierExtractor::degraded(
            ExtractorId::from(42),
            "brain:gliner".into(),
            entity_target(),
            1,
            0.5,
            "classifier model not loaded",
        )
    }

    #[test]
    fn degraded_extractor_dispatch_writes_skipped_disabled() {
        let reg = ExtractorRegistry::new();
        let ext = degraded_ext();
        assert!(!ext.is_loaded());
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("Alice met Bob")));
        assert_eq!(
            r.status,
            crate::extractor::ExtractionStatus::SkippedDisabled
        );
        assert!(r.status_reason.contains("not loaded"));
    }

    #[test]
    fn degraded_extractor_returns_zero_items() {
        let reg = ExtractorRegistry::new();
        let r = futures_lite::future::block_on(degraded_ext().run(&ctx(&reg), &memory("anything")));
        assert!(r.items.is_empty());
    }

    // ----- Label snapshotting & projection ---------------------------------

    /// Records the labels passed to `predict()` so tests can assert
    /// the snapshot wired through correctly.
    struct LabelCaptureModel {
        seen: parking_lot::Mutex<Vec<Vec<String>>>,
        spans_per_call: Vec<ClassifiedSpan>,
    }

    impl LabelCaptureModel {
        fn new(spans: Vec<ClassifiedSpan>) -> Self {
            Self {
                seen: parking_lot::Mutex::new(Vec::new()),
                spans_per_call: spans,
            }
        }
    }

    impl ClassifierModel for LabelCaptureModel {
        fn predict(
            &self,
            _text: &str,
            labels: &[&str],
        ) -> Result<Vec<ClassifiedSpan>, ExtractorError> {
            self.seen
                .lock()
                .push(labels.iter().map(|s| (*s).to_string()).collect());
            Ok(self.spans_per_call.clone())
        }
        fn version(&self) -> &str {
            "label-capture"
        }
    }

    #[test]
    fn simple_label_strips_namespace_prefix() {
        let cases = [
            ("brain:Person", "Person"),
            ("acme:Customer", "Customer"),
            ("Person", "Person"),
            ("a:b:c", "b:c"),
            ("", ""),
            (":Leading", "Leading"),
        ];
        for (input, want) in cases {
            assert_eq!(simple_label(input), want, "input={input:?}");
        }
    }

    #[test]
    fn predict_passes_stripped_labels_to_model() {
        let model = Arc::new(LabelCaptureModel::new(Vec::new()));
        let labels = default_labels();
        let ext = ClassifierExtractor::new(
            ExtractorId::from(1),
            "brain:gliner".into(),
            entity_target(),
            1,
            0.5,
            model.clone(),
            labels.clone(),
        );
        let reg = ExtractorRegistry::new();
        let _ = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("text")));
        let seen = model.seen.lock();
        assert_eq!(seen.len(), 1);
        let want: Vec<String> = labels.iter().map(|q| simple_label(q).to_string()).collect();
        assert_eq!(seen[0], want);
        // Belt-and-braces: none of the labels carry a colon.
        assert!(
            seen[0].iter().all(|l| !l.contains(':')),
            "labels={:?}",
            seen[0]
        );
    }

    #[test]
    fn predict_remaps_label_back_to_qname_before_projection() {
        let spans = vec![ClassifiedSpan {
            label: "Person".into(),
            text: "Alice".into(),
            char_start: 0,
            char_end: 5,
            confidence: 0.97,
        }];
        let model = Arc::new(LabelCaptureModel::new(spans));
        let ext = ClassifierExtractor::new(
            ExtractorId::from(1),
            "brain:gliner".into(),
            entity_target(),
            1,
            0.5,
            model,
            default_labels(),
        );
        let reg = ExtractorRegistry::new();
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("Alice ...")));
        assert_eq!(r.items.len(), 1);
        match &r.items[0] {
            ExtractedItem::EntityMention(em) => {
                assert_eq!(em.entity_type_qname, "brain:Person");
                assert_eq!(em.text, "Alice");
            }
            other => panic!("expected EntityMention, got {other:?}"),
        }
    }

    #[test]
    fn predict_drops_spans_whose_label_did_not_match_a_known_simple() {
        let spans = vec![
            ClassifiedSpan {
                label: "Animal".into(),
                text: "Hedwig".into(),
                char_start: 0,
                char_end: 6,
                confidence: 0.95,
            },
            ClassifiedSpan {
                label: "Person".into(),
                text: "Harry".into(),
                char_start: 10,
                char_end: 15,
                confidence: 0.95,
            },
        ];
        let model = Arc::new(LabelCaptureModel::new(spans));
        let ext = ClassifierExtractor::new(
            ExtractorId::from(1),
            "brain:gliner".into(),
            entity_target(),
            1,
            0.5,
            model,
            default_labels(),
        );
        let reg = ExtractorRegistry::new();
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("...")));
        assert_eq!(r.items.len(), 1, "unknown labels must be dropped");
        match &r.items[0] {
            ExtractedItem::EntityMention(em) => {
                assert_eq!(em.entity_type_qname, "brain:Person");
                assert_eq!(em.text, "Harry");
            }
            other => panic!("expected EntityMention, got {other:?}"),
        }
    }

    #[test]
    fn empty_label_snapshot_yields_skipped() {
        let model = Arc::new(LabelCaptureModel::new(Vec::new()));
        let ext = ClassifierExtractor::new(
            ExtractorId::from(1),
            "brain:gliner".into(),
            entity_target(),
            1,
            0.5,
            model.clone(),
            Arc::new(Vec::new()),
        );
        let reg = ExtractorRegistry::new();
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("hi")));
        assert_eq!(
            r.status,
            crate::extractor::ExtractionStatus::SkippedDisabled
        );
        assert!(model.seen.lock().is_empty(), "model.predict should not run");
    }

    #[test]
    fn project_emits_brain_qname_verbatim_from_span_label() {
        let ext = ClassifierExtractor::new(
            ExtractorId::from(1),
            "brain:gliner".into(),
            entity_target(),
            1,
            0.5,
            Arc::new(LabelCaptureModel::new(Vec::new())),
            default_labels(),
        );
        let span = ClassifiedSpan {
            label: "brain:Organization".into(),
            text: "Acme".into(),
            char_start: 0,
            char_end: 4,
            confidence: 0.9,
        };
        let item = ext.project(span).expect("entity span projects");
        match item {
            ExtractedItem::EntityMention(em) => {
                assert_eq!(em.entity_type_qname, "brain:Organization");
                assert_eq!(em.text, "Acme");
            }
            other => panic!("expected EntityMention, got {other:?}"),
        }
    }

    #[test]
    fn run_filters_below_confidence_threshold() {
        // Spans carry the *simple* label GLiNER would emit — the extractor
        // remaps them back to the full qname before projection.
        let spans = vec![
            ClassifiedSpan {
                label: "Person".into(),
                text: "Alice".into(),
                char_start: 0,
                char_end: 5,
                confidence: 0.95,
            },
            ClassifiedSpan {
                label: "Person".into(),
                text: "Bob".into(),
                char_start: 10,
                char_end: 13,
                confidence: 0.4,
            },
        ];
        let model = Arc::new(LabelCaptureModel::new(spans));
        let ext = ClassifierExtractor::new(
            ExtractorId::from(1),
            "brain:gliner".into(),
            entity_target(),
            1,
            0.6,
            model,
            default_labels(),
        );
        let reg = ExtractorRegistry::new();
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("Alice met Bob")));
        assert_eq!(r.status, crate::extractor::ExtractionStatus::Success);
        assert_eq!(r.items.len(), 1);
        match &r.items[0] {
            ExtractedItem::EntityMention(em) => assert_eq!(em.text, "Alice"),
            other => panic!("expected EntityMention, got {other:?}"),
        }
    }

    // ----- ClassifierConfig accessors --------------------------------------

    #[test]
    fn config_has_path_and_model_path_accessors() {
        let unloaded = ClassifierConfig::unloaded();
        assert!(!unloaded.has_path());
        assert_eq!(unloaded.model_path(), std::path::Path::new(""));

        let loaded = ClassifierConfig::with_model_path("/srv/ner".into());
        assert!(loaded.has_path());
        assert_eq!(loaded.model_path(), std::path::Path::new("/srv/ner"));
    }

    // ----- Auto-discovery cascade ------------------------------------------

    /// Builds a deterministic env reader from a vector of owned
    /// `(key, value)` pairs so each test can describe exactly the env
    /// it cares about without touching the global process env. Owning
    /// the strings sidesteps lifetime fights with `tempfile::TempDir`
    /// paths that don't outlive the test scope.
    fn env_fn(pairs: Vec<(String, String)>) -> impl Fn(&str) -> Option<String> {
        move |k| {
            pairs
                .iter()
                .find_map(|(name, value)| (name == k).then(|| value.clone()))
        }
    }

    fn env_pair(k: &str, v: &str) -> (String, String) {
        (k.to_string(), v.to_string())
    }

    /// Materialise a directory containing every required GLiNER file as
    /// a one-byte stub so `Path::is_file` succeeds on each entry.
    fn write_fake_gliner_dir(dir: &std::path::Path) {
        std::fs::create_dir_all(dir).expect("mkdir model dir");
        for f in NER_MODEL_REQUIRED_FILES {
            std::fs::write(dir.join(f), b"x").expect("write stub file");
        }
    }

    #[test]
    fn auto_discover_returns_unloaded_when_env_unset_and_default_path_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env_fn(vec![env_pair(
            "XDG_DATA_HOME",
            tmp.path().to_str().unwrap(),
        )]);
        let is_file = |p: &Path| p.is_file();
        let cfg = ClassifierConfig::auto_discover_with(&env, &is_file);
        assert!(
            !cfg.has_path(),
            "expected unloaded; got {:?}",
            cfg.model_path
        );
    }

    #[test]
    fn auto_discover_returns_with_path_when_env_unset_and_default_path_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let model_dir = tmp
            .path()
            .join("brain")
            .join("models")
            .join(NER_MODEL_DIR_NAME);
        write_fake_gliner_dir(&model_dir);

        let env = env_fn(vec![env_pair(
            "XDG_DATA_HOME",
            tmp.path().to_str().unwrap(),
        )]);
        let is_file = |p: &Path| p.is_file();
        let cfg = ClassifierConfig::auto_discover_with(&env, &is_file);
        assert_eq!(cfg.model_path.as_deref(), Some(model_dir.as_path()));
    }

    #[test]
    fn auto_discover_prefers_env_var_over_default_path() {
        let tmp_xdg = tempfile::tempdir().unwrap();
        let xdg_model_dir = tmp_xdg
            .path()
            .join("brain")
            .join("models")
            .join(NER_MODEL_DIR_NAME);
        write_fake_gliner_dir(&xdg_model_dir);

        let tmp_explicit = tempfile::tempdir().unwrap();
        let explicit = tmp_explicit.path().to_path_buf();

        let env = env_fn(vec![
            env_pair(NER_MODEL_PATH_ENV, explicit.to_str().unwrap()),
            env_pair("XDG_DATA_HOME", tmp_xdg.path().to_str().unwrap()),
        ]);
        let is_file = |p: &Path| p.is_file();
        let cfg = ClassifierConfig::auto_discover_with(&env, &is_file);
        assert_eq!(
            cfg.model_path.as_deref(),
            Some(explicit.as_path()),
            "env-var path should win over XDG even when XDG also has a valid install"
        );
    }

    #[test]
    fn auto_discover_falls_through_to_default_when_env_var_empty_string() {
        let tmp = tempfile::tempdir().unwrap();
        let model_dir = tmp
            .path()
            .join("brain")
            .join("models")
            .join(NER_MODEL_DIR_NAME);
        write_fake_gliner_dir(&model_dir);

        let env = env_fn(vec![
            env_pair(NER_MODEL_PATH_ENV, ""),
            env_pair("XDG_DATA_HOME", tmp.path().to_str().unwrap()),
        ]);
        let is_file = |p: &Path| p.is_file();
        let cfg = ClassifierConfig::auto_discover_with(&env, &is_file);
        assert_eq!(
            cfg.model_path.as_deref(),
            Some(model_dir.as_path()),
            "empty BRAIN_NER_MODEL_PATH must be treated as unset"
        );
    }

    #[test]
    fn auto_discover_skips_default_when_one_required_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let model_dir = tmp
            .path()
            .join("brain")
            .join("models")
            .join(NER_MODEL_DIR_NAME);
        write_fake_gliner_dir(&model_dir);
        // Remove one of the required files to simulate a broken install.
        std::fs::remove_file(model_dir.join("tokenizer.json")).unwrap();

        let env = env_fn(vec![env_pair(
            "XDG_DATA_HOME",
            tmp.path().to_str().unwrap(),
        )]);
        let is_file = |p: &Path| p.is_file();
        let cfg = ClassifierConfig::auto_discover_with(&env, &is_file);
        assert!(
            !cfg.has_path(),
            "missing tokenizer.json should drop the config back to unloaded"
        );
    }

    #[test]
    fn default_xdg_model_dir_prefers_xdg_data_home() {
        let env = env_fn(vec![
            env_pair("XDG_DATA_HOME", "/srv/data"),
            env_pair("HOME", "/home/dev"),
        ]);
        let dir = default_xdg_model_dir_with(&env).expect("env supplies both");
        assert_eq!(
            dir,
            PathBuf::from("/srv/data/brain/models/gliner-small-v2.1")
        );
    }

    #[test]
    fn default_xdg_model_dir_falls_back_to_home_local_share() {
        let env = env_fn(vec![env_pair("HOME", "/home/dev")]);
        let dir = default_xdg_model_dir_with(&env).expect("HOME is set");
        assert_eq!(
            dir,
            PathBuf::from("/home/dev/.local/share/brain/models/gliner-small-v2.1")
        );
    }

    // ----- Statement-kind pattern classifier --------------------------------

    #[test]
    fn pattern_kind_first_person_preference() {
        let cases = [
            "I prefer dark roast coffee.",
            "I like async meetings.",
            "I love this team.",
            "I hate flaky tests.",
            "I'd rather skip the call.",
            "I don't like long meetings.",
            "My favorite editor is helix.",
        ];
        for text in cases {
            let got = classify_statement_kind_pattern(text);
            assert!(
                matches!(got, Some((StatementKind::Preference, c)) if c >= 0.7),
                "text={text:?} got={got:?}"
            );
        }
    }

    #[test]
    fn pattern_kind_event_with_date_and_verb() {
        let cases = [
            "The all-hands is Friday at 10am.",
            "The release is scheduled for 2026-06-15.",
            "Demo happened on Tuesday.",
            "The standup is at 9:30am.",
            "Our deploy occurred at 15:00.",
        ];
        for text in cases {
            let got = classify_statement_kind_pattern(text);
            assert!(
                matches!(got, Some((StatementKind::Event, c)) if c >= 0.7),
                "text={text:?} got={got:?}"
            );
        }
    }

    #[test]
    fn pattern_kind_fact_with_copula() {
        let cases = [
            "Alice works at Acme Corp.",
            "Bob lives in Berlin.",
            "The capital of France is Paris.",
            "Acme has 200 employees.",
        ];
        for text in cases {
            let got = classify_statement_kind_pattern(text);
            assert!(
                matches!(got, Some((StatementKind::Fact, c)) if c >= 0.7),
                "text={text:?} got={got:?}"
            );
        }
    }

    #[test]
    fn pattern_kind_none_for_ambiguous() {
        // No copula, no preference cue, no event cue — caller must
        // defer to LLM.
        let got = classify_statement_kind_pattern("Whatever happens.");
        assert!(got.is_none(), "got={got:?}");
    }

    #[test]
    fn pattern_kind_preference_beats_event_when_both_fire() {
        // "I prefer ... by Friday" — preference wins; the deadline is
        // context, not the statement's truth condition.
        let got = classify_statement_kind_pattern("I prefer to ship the review by Friday at 3pm.");
        assert!(
            matches!(got, Some((StatementKind::Preference, _))),
            "got={got:?}"
        );
    }

    #[test]
    fn pattern_kind_year_anchor_alone_is_not_an_event() {
        // "founded in 2024" is a Fact, not an Event — no event noun /
        // verb fires.
        let got = classify_statement_kind_pattern("Acme was founded in 2024.");
        assert!(matches!(got, Some((StatementKind::Fact, _))), "got={got:?}");
    }

    // ----- Real-inference smoke (operator-gated) ----------------------------
    //
    // Run with `BRAIN_NER_MODEL_PATH=/path/to/gliner cargo test \
    //     -p brain-extractors --lib classifier::tests::real_inference -- \
    //     --ignored --nocapture`.

    #[test]
    #[ignore = "requires BRAIN_NER_MODEL_PATH and an operator-provided GLiNER model"]
    fn real_inference_returns_brain_qnames_for_alice() {
        let path = match std::env::var("BRAIN_NER_MODEL_PATH") {
            Ok(p) => p,
            Err(_) => {
                eprintln!("BRAIN_NER_MODEL_PATH unset — skipping");
                return;
            }
        };
        let cfg = ClassifierConfig::with_model_path(path.into());
        let model = GlinerClassifier::load(&cfg).expect("load");
        let labels = ["brain:Person", "brain:Place"];
        let spans = model
            .predict("Alice met Bob in Paris.", &labels)
            .expect("predict");
        // GLiNER emits the qnames we passed in verbatim.
        assert!(spans
            .iter()
            .any(|s| s.label == "brain:Person" && s.text.contains("Alice")));
        assert!(spans
            .iter()
            .any(|s| s.label == "brain:Place" && s.text.contains("Paris")));
    }
}
