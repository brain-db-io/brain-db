//! # brain-extractors
//!
//! Extractor framework (pattern / classifier / LLM) for the Brain
//! knowledge layer. Phase 20.1 lands the trait + registry + output
//! types; pattern / classifier impls follow in 20.2 / 20.3.
//!
//! See `spec/22_extractors/` for the authoritative design.

#![allow(
    clippy::module_name_repetitions,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]
#![forbid(unsafe_code)]

pub mod classifier;
pub mod enricher_hook;
pub mod extractor;
pub mod gliner;
pub mod idempotency;
pub mod item;
pub mod llm;
pub mod materialize;
pub mod options;
pub mod pattern;
pub mod registry;
pub mod resolver;
pub mod supersede_source;

pub use classifier::{
    classify_statement_kind_pattern, default_xdg_model_dir, ClassifiedSpan, ClassifierConfig,
    ClassifierExtractor, ClassifierModel, GlinerClassifier, NER_MODEL_DIR_NAME, NER_MODEL_PATH_ENV,
    NER_MODEL_REQUIRED_FILES, STATEMENT_KIND_PATTERN_THRESHOLD,
};
pub use enricher_hook::{run_pipeline_enrichers, EnricherHook, EnricherHookOutcome};
pub use extractor::{
    ExtractionContext, ExtractionFuture, ExtractionResult, ExtractionStatus, Extractor,
    ExtractorContext, ExtractorError, NeighborMemory,
};
pub use gliner::{GlinerConfig, GlinerError, GlinerModel, Span as GlinerSpan};
pub use idempotency::{hash_memory_text, IdempotencyKey};
pub use item::{EntityMention, ExtractedItem, RelationMention, StatementMention};
pub use llm::{estimate_cost, CostBudget, LlmExtractor, LlmExtractorInner, Pricing};
pub use materialize::{
    build_registry_from_definitions, materialize_classifier_extractor, materialize_llm_extractor,
    materialize_pattern_extractor, MaterializeDeps,
};
pub use options::ExtractorRunOptions;
pub use pattern::{CompiledRegex, PatternExtractor};
pub use registry::ExtractorRegistry;
pub use resolver::{resolve_or_create, Resolution, ResolutionTier, ResolverError};
pub use supersede_source::StatementHnswSource;
