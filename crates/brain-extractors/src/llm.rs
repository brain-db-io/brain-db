//! LLM extractor — third extractor tier. Spec §22/09.
//!
//! Wraps an [`LlmClient`] (Anthropic / OpenAI) behind the
//! [`Extractor`] trait. Adds:
//!
//! - Per-shard response cache ([`LlmCacheDb`], spec §15.4 /
//!   §26).
//! - Pre-call cost budget (`CostBudget { per_call_micro_usd }`).
//! - JSON-schema validation on the response, with one retry on
//!   schema failure (validation error fed back into the prompt
//!   per §22/09 §4).
//! - Output projection to `EntityMention` / `StatementMention` /
//!   `RelationMention` per the extractor's [`ExtractorTarget`].
//!
//! ## Degraded mode
//!
//! Mirrors [`crate::classifier::ClassifierExtractor`]: when no
//! `LlmClient` is configured (env keys unset, unknown model
//! prefix, etc.) the materializer constructs a degraded
//! extractor that returns `SkippedDisabled(reason)` on every
//! dispatch. Unconfigured ≠ failure: the tier never tried, so
//! the pipeline classifier does not count it toward
//! `PARTIAL_FAILURE`. The captured reason still surfaces in the
//! audit row so operators can see why no LLM ran.

use std::sync::Arc;
use std::time::Duration;

use brain_core::knowledge::{
    ExtractorKind, Statement, StatementObject, StatementValue, SubjectRef,
};
use brain_core::{ExtractorId, Memory};
use brain_llm::types::SystemBlock;
use brain_llm::{LlmClient, LlmError, LlmMessage, LlmRequest, LlmRole};
use brain_metadata::entity::ops::entity_get;
use brain_metadata::llm_cache::{LlmResponse as CachedResponse, LLM_RESPONSES_TABLE};
use brain_metadata::schema::predicate::predicate_get;
use brain_metadata::statement::{JudgeError, JudgeFuture, JudgeVerdict, StatementJudge};
use brain_metadata::LlmCacheDb;
use brain_protocol::schema::ast::StatementKindAst;
use brain_protocol::schema::ExtractorTarget;
use jsonschema::JSONSchema;
use parking_lot::Mutex;
use redb::ReadTransaction;
use serde_json::Value;

use crate::extractor::{
    ExtractionContext, ExtractionFuture, ExtractionResult, ExtractionStatus, Extractor,
    ExtractorContext, NeighborMemory,
};
use crate::idempotency::hash_memory_text;
use crate::item::{EntityMention, ExtractedItem, RelationMention, StatementMention};

const DEFAULT_CACHE_TTL_SECS: u64 = 7 * 24 * 60 * 60; // 7 days.

/// Hard cap on per-call input tokens for the LLM extractor. Above this
/// the prompt builder trims sections — first the rolling summary, then
/// the lowest-similarity neighbors — until the request fits. The cap
/// is conservative on purpose: Haiku-class models stay sub-second
/// under ~4k input tokens; pushing past that trades latency for
/// signal that the bounded-context worker already filtered for.
const LLM_INPUT_TOKEN_BUDGET: u64 = 4_000;

/// Per-neighbor character cap in the rendered prompt section. The
/// fetch helper truncates at the storage layer (200 chars); this is
/// the same number for self-documentation and a defensive cap in
/// case the helper's cap changes.
const NEIGHBOR_TEXT_CHAR_CAP_IN_PROMPT: usize = 200;

/// Hard cap on the rolling summary's chars in the prompt — the
/// summarizer worker (when wired) is expected to honour this, but
/// we enforce it here too so a misbehaving summarizer can't blow the
/// prompt budget.
const SUMMARY_CHAR_CAP: usize = 500;

/// Per-call cost ceiling. Phase 21 ships per-call only; the
/// per-deployment global budget is post-v1 (§22/09 §5 + §22/07).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostBudget {
    pub per_call_micro_usd: u64,
}

/// Pricing for a single model in dollar micro-units per token.
/// Operator-overridable; v1 ships a small embedded default table
/// for the common models (§22/09 §5). Unknown models fall back
/// to the conservative default `100 µ$/1K input + 300 µ$/1K
/// output` ⇒ `0.1 µ$ / 0.3 µ$` per token.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pricing {
    pub input_micro_usd_per_token: f64,
    pub output_micro_usd_per_token: f64,
}

impl Pricing {
    /// Spec §22/09 §5 default — used when no model-specific entry
    /// is registered.
    #[must_use]
    pub const fn conservative_default() -> Self {
        Self {
            input_micro_usd_per_token: 0.1,
            output_micro_usd_per_token: 0.3,
        }
    }

    /// Lookup pricing by model prefix. Embedded table covers the
    /// models referenced in spec §22/09 §5; operators override
    /// via the pricing config in phase 21.5+.
    #[must_use]
    pub fn for_model(model: &str) -> Self {
        if model.starts_with("claude-haiku") {
            Self {
                input_micro_usd_per_token: 1.0,
                output_micro_usd_per_token: 5.0,
            }
        } else if model.starts_with("claude-sonnet") {
            Self {
                input_micro_usd_per_token: 3.0,
                output_micro_usd_per_token: 15.0,
            }
        } else if model.starts_with("gpt-4o-mini") {
            Self {
                input_micro_usd_per_token: 0.15,
                output_micro_usd_per_token: 0.6,
            }
        } else {
            Self::conservative_default()
        }
    }
}

/// Estimated dollar-micro cost of issuing `request` against
/// `pricing`. Uses `LlmRequest::approx_input_tokens()` for the
/// input side and `max_tokens` as the worst-case output.
#[must_use]
pub fn estimate_cost(request: &LlmRequest, pricing: &Pricing) -> u64 {
    let in_tokens = request.approx_input_tokens() as f64;
    let out_tokens = f64::from(request.max_tokens);
    (in_tokens * pricing.input_micro_usd_per_token
        + out_tokens * pricing.output_micro_usd_per_token)
        .round() as u64
}

// ---------------------------------------------------------------------------
// LlmExtractor.
// ---------------------------------------------------------------------------

/// LLM-tier extractor. Constructed by the materializer
/// (`materialize_llm_extractor`, phase 21.4) from an
/// `ExtractorDefinition` row.
pub struct LlmExtractor {
    id: ExtractorId,
    name: String,
    target: ExtractorTarget,
    extractor_version: u32,
    confidence_threshold: f32,
    cost_budget: Option<CostBudget>,
    cache_ttl: Duration,
    inner: Option<Arc<LlmExtractorInner>>,
    degraded_reason: Option<String>,
}

/// Fully-wired inner state. Held behind `Option` so degraded
/// extractors can carry just `degraded_reason`.
pub struct LlmExtractorInner {
    pub client: Arc<dyn LlmClient>,
    pub cache: Option<Arc<Mutex<LlmCacheDb>>>,
    pub prompt: String,
    pub examples: Option<Value>,
    pub response_schema: Option<Value>,
    /// Compiled draft-7 validator. `None` when `response_schema`
    /// is absent (free-form response mode per §22/09 §6).
    pub schema_compiled: Option<JSONSchema>,
    pub pricing: Pricing,
    pub max_tokens: u32,
    pub temperature: f32,
    pub timeout: Duration,
}

impl LlmExtractor {
    /// Fully-wired extractor.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: ExtractorId,
        name: String,
        target: ExtractorTarget,
        extractor_version: u32,
        confidence_threshold: f32,
        cost_budget: Option<CostBudget>,
        cache_ttl: Duration,
        inner: LlmExtractorInner,
    ) -> Self {
        Self {
            id,
            name,
            target,
            extractor_version,
            confidence_threshold,
            cost_budget,
            cache_ttl,
            inner: Some(Arc::new(inner)),
            degraded_reason: None,
        }
    }

    /// Flat constructor used by `materialize_llm_extractor`. All
    /// of `LlmExtractorInner`'s fields are passed in directly so
    /// tests can build instances without round-tripping through
    /// the AST.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        id: ExtractorId,
        name: String,
        target: ExtractorTarget,
        extractor_version: u32,
        client: Arc<dyn LlmClient>,
        cache: Option<Arc<Mutex<LlmCacheDb>>>,
        prompt: String,
        examples: Option<Value>,
        response_schema: Option<Value>,
        schema_compiled: Option<JSONSchema>,
        confidence_threshold: f32,
        cost_budget: Option<CostBudget>,
        cache_ttl: Duration,
    ) -> Self {
        let pricing = Pricing::for_model(client.model());
        Self::new(
            id,
            name,
            target,
            extractor_version,
            confidence_threshold,
            cost_budget,
            cache_ttl,
            LlmExtractorInner {
                client,
                cache,
                prompt,
                examples,
                response_schema,
                schema_compiled,
                pricing,
                max_tokens: 1024,
                temperature: 0.0,
                timeout: Duration::from_secs(30),
            },
        )
    }

    /// Degraded extractor — no LLM client wired. Every dispatch
    /// returns `SkippedDisabled(reason)` with the captured cause.
    /// The tier never attempted work, so its status must not feed
    /// the pipeline's partial-failure accounting.
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
            cost_budget: None,
            cache_ttl: Duration::from_secs(DEFAULT_CACHE_TTL_SECS),
            inner: None,
            degraded_reason: Some(reason.into()),
        }
    }

    /// True iff a real LLM client is wired in.
    #[must_use]
    pub fn is_wired(&self) -> bool {
        self.inner.is_some()
    }

    /// Compile the operator-declared JSON schema. Returns `None`
    /// when `schema` is `None`; otherwise either the compiled
    /// validator or `Err` describing why compilation failed.
    pub fn compile_schema(schema: Option<&Value>) -> Result<Option<JSONSchema>, String> {
        match schema {
            None => Ok(None),
            Some(v) => match JSONSchema::options().compile(v) {
                Ok(c) => Ok(Some(c)),
                Err(e) => Err(format!("schema compile failed: {e}")),
            },
        }
    }

    fn build_request(
        &self,
        inner: &LlmExtractorInner,
        memory_id: brain_core::MemoryId,
        memory_text: &str,
        prior_entities: &[&EntityMention],
        extractor_context: Option<&ExtractorContext>,
        now_unix_nanos: u64,
    ) -> (LlmRequest, BuildRequestStats) {
        let (user_body, stats) = render_prompt_with_context(
            &inner.prompt,
            memory_text,
            prior_entities,
            extractor_context,
            now_unix_nanos,
            memory_id,
        );
        // Split the system context into two cacheable blocks: the role
        // block (stable across every call) and the schema block (stable
        // per schema version). Both are eligible for Anthropic's
        // ephemeral prompt cache. The per-call body lives in the user
        // message and is never cached — that's the surface area W2.3
        // extends with bounded inferential context.
        let request = LlmRequest {
            model: inner.client.model().to_string(),
            system_blocks: build_extractor_system_blocks(inner.examples.as_ref()),
            messages: vec![brain_llm::LlmMessage {
                role: brain_llm::LlmRole::User,
                content: user_body,
            }],
            response_schema: inner.response_schema.clone(),
            temperature: inner.temperature,
            max_tokens: inner.max_tokens,
            timeout: inner.timeout,
        };
        (request, stats)
    }

    fn project_value(&self, parsed: &Value) -> Vec<ExtractedItem> {
        // Three response shapes are supported:
        //   1. `[item, item, ...]` — flat array. Each item is projected
        //      through the extractor's declared target.
        //   2. `{ "statements": [...], "relations": [...] }` — the
        //      schema prompt's structured shape. Each section is
        //      projected through the matching projector regardless of
        //      the extractor's declared target so a single LLM call can
        //      cover both kinds.
        //   3. A single item — wrapped in a one-element vec, same as
        //      shape 1.
        let mut out = Vec::new();
        match parsed {
            Value::Object(obj)
                if obj.contains_key("statements") || obj.contains_key("relations") =>
            {
                if let Some(Value::Array(arr)) = obj.get("statements") {
                    for v in arr {
                        if let Some(item) =
                            project_statement_open(v, self.id.raw(), self.extractor_version)
                        {
                            if item.confidence() >= self.confidence_threshold {
                                out.push(item);
                            }
                        }
                    }
                }
                if let Some(Value::Array(arr)) = obj.get("relations") {
                    for v in arr {
                        if let Some(item) =
                            project_relation_open(v, self.id.raw(), self.extractor_version)
                        {
                            if item.confidence() >= self.confidence_threshold {
                                out.push(item);
                            }
                        }
                    }
                }
            }
            Value::Array(arr) => {
                for v in arr {
                    if let Some(item) = self.project_one(v) {
                        if item.confidence() >= self.confidence_threshold {
                            out.push(item);
                        }
                    }
                }
            }
            v => {
                if let Some(item) = self.project_one(v) {
                    if item.confidence() >= self.confidence_threshold {
                        out.push(item);
                    }
                }
            }
        }
        out
    }

    fn project_one(&self, v: &Value) -> Option<ExtractedItem> {
        match &self.target {
            ExtractorTarget::Entity { entity_type } => {
                project_entity(v, entity_type, self.id.raw(), self.extractor_version)
            }
            ExtractorTarget::Statement { kind } => project_statement(
                v,
                kind_to_byte(*kind),
                self.id.raw(),
                self.extractor_version,
            ),
            ExtractorTarget::Relation { relation_type } => {
                project_relation(v, relation_type, self.id.raw(), self.extractor_version)
            }
            ExtractorTarget::EntityOrStatement => {
                if v.get("predicate").is_some() {
                    project_statement(v, 1, self.id.raw(), self.extractor_version)
                } else if v.is_string() || v.get("name").is_some() {
                    project_entity(v, "brain:Entity", self.id.raw(), self.extractor_version)
                } else {
                    None
                }
            }
        }
    }

    /// Run the supersession judge over a pair of statements. Tier 2 of
    /// the [`brain_metadata::statement::TieredSupersedeDecider`] ladder
    /// calls this when a candidate's cosine sits in the ambiguity band
    /// (typically `[0.82, 0.92)`). Returns `Supersedes` /
    /// `Contradicts` / `Coexists` per the prompt below.
    ///
    /// The judge reuses the LLM extractor's wired client + cache + cost
    /// budget so deployments don't need a second transport — the
    /// per-call budget guards against runaway costs, and the role
    /// block ships through the same Anthropic prompt-cache breakpoint
    /// as the extractor's role block so the steady-state input-token
    /// bill collapses on repeated judge invocations.
    pub async fn judge_supersedes_call(
        &self,
        new: &Statement,
        existing: &Statement,
        rtxn: &ReadTransaction,
    ) -> Result<JudgeVerdict, JudgeError> {
        let Some(inner) = self.inner.as_ref() else {
            return Err(JudgeError::Transport(
                "judge unavailable: LLM extractor is degraded (no client wired)".into(),
            ));
        };

        let new_human = render_statement_human_readable(new, rtxn)
            .map_err(|e| JudgeError::Transport(format!("render new statement: {e}")))?;
        let old_human = render_statement_human_readable(existing, rtxn)
            .map_err(|e| JudgeError::Transport(format!("render existing statement: {e}")))?;

        let user_body = format!(
            "Two extracted statements share the same subject. Decide their relationship:\n\
             - SUPERSEDES if the new statement replaces the old (same attribute, new value).\n\
             - CONTRADICTS if both can't be true at once.\n\
             - COEXISTS if both can be true simultaneously (e.g. multiple skills, sequential roles).\n\n\
             Existing: {old_human}\n\
             New: {new_human}\n\n\
             Respond with exactly one word: SUPERSEDES, CONTRADICTS, or COEXISTS."
        );

        let request = LlmRequest {
            model: inner.client.model().to_string(),
            system_blocks: vec![SystemBlock::cached(JUDGE_ROLE_BLOCK)],
            messages: vec![LlmMessage {
                role: LlmRole::User,
                content: user_body,
            }],
            response_schema: None,
            temperature: 0.0,
            // Single-token verdict; cap budget tightly so a runaway
            // model can't blow the per-call ceiling.
            max_tokens: 16,
            timeout: inner.timeout,
        };

        if let Some(budget) = self.cost_budget {
            let est = estimate_cost(&request, &inner.pricing);
            if est > budget.per_call_micro_usd {
                return Err(JudgeError::Budget(format!(
                    "estimated {} µ$ exceeds per-call budget {} µ$",
                    est, budget.per_call_micro_usd
                )));
            }
        }

        let resp = inner
            .client
            .complete(request)
            .await
            .map_err(|e| JudgeError::Transport(llm_error_reason(&e)))?;
        parse_verdict(&resp.content)
    }
}

/// Stable role block sent on every judge call. The prompt cache reads
/// it on repeated calls so the steady-state input cost is bounded by
/// the per-call statement render only.
const JUDGE_ROLE_BLOCK: &str = "You are a knowledge-graph supersession judge for Brain. \
     Given two extracted statements about the same subject, decide whether the new statement \
     supersedes the old (same attribute, new value), contradicts it (both cannot be true at \
     once), or coexists with it (both can be true simultaneously — multiple skills, sequential \
     roles, parallel facts). Be conservative: if uncertain, prefer COEXISTS. Respond with one \
     word.";

/// Parse the judge's reply. Tolerant to surrounding punctuation,
/// trailing periods, and case — but rejects anything that doesn't
/// match one of the three verdicts so a model that drifted into
/// free-form explanation surfaces as a Parse error rather than
/// silently picking a default.
fn parse_verdict(s: &str) -> Result<JudgeVerdict, JudgeError> {
    let cleaned: String = s
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .collect::<String>()
        .to_ascii_uppercase();
    match cleaned.as_str() {
        x if x.starts_with("SUPERSEDES") => Ok(JudgeVerdict::Supersedes),
        x if x.starts_with("CONTRADICTS") => Ok(JudgeVerdict::Contradicts),
        x if x.starts_with("COEXISTS") => Ok(JudgeVerdict::Coexists),
        _ => Err(JudgeError::Parse(format!("unrecognized verdict: {s:?}"))),
    }
}

/// Render a [`Statement`] into a one-line human-readable phrase using
/// the subject's canonical name and the predicate's qname. Used by
/// the judge to give the LLM a stable, terse rendering of the pair.
fn render_statement_human_readable(
    s: &Statement,
    rtxn: &ReadTransaction,
) -> Result<String, String> {
    let subject = match s.subject {
        SubjectRef::Entity(eid) => match entity_get(rtxn, eid) {
            Ok(Some(e)) => e.canonical_name,
            Ok(None) => format!("entity:{eid:?}"),
            Err(e) => return Err(format!("entity_get: {e}")),
        },
        SubjectRef::Pending(audit) => format!("pending:{audit:?}"),
    };
    let predicate = match predicate_get(rtxn, s.predicate) {
        Ok(Some(p)) => p.canonical(),
        Ok(None) => format!("predicate:{}", s.predicate.raw()),
        Err(e) => return Err(format!("predicate_get: {e}")),
    };
    let object = match &s.object {
        StatementObject::Entity(eid) => match entity_get(rtxn, *eid) {
            Ok(Some(e)) => e.canonical_name,
            Ok(None) => format!("entity:{eid:?}"),
            Err(e) => return Err(format!("entity_get(object): {e}")),
        },
        StatementObject::Value(v) => render_value(v),
        StatementObject::Memory(mid) => format!("memory:{mid:?}"),
        StatementObject::Statement(sid) => format!("statement:{sid:?}"),
    };
    Ok(format!("{subject} {predicate} {object}"))
}

fn render_value(v: &StatementValue) -> String {
    match v {
        StatementValue::Text(s) => s.clone(),
        StatementValue::Integer(i) => i.to_string(),
        StatementValue::Float(f) => f.to_string(),
        StatementValue::Bool(b) => b.to_string(),
        StatementValue::UnixNanos(n) => format!("unix_ns:{n}"),
        StatementValue::Blob(b) => format!("<blob:{}>", b.len()),
    }
}

impl StatementJudge for LlmExtractor {
    fn judge_supersedes<'a>(
        &'a self,
        new_stmt: &'a Statement,
        existing_stmt: &'a Statement,
        rtxn: &'a ReadTransaction,
    ) -> JudgeFuture<'a> {
        Box::pin(async move {
            self.judge_supersedes_call(new_stmt, existing_stmt, rtxn)
                .await
        })
    }
}

/// Pull `EntityMention`s for `memory_id` out of `ctx.prior_tier_items`.
/// Statement / relation mentions are filtered out — the LLM tier owns
/// predicate and relation extraction, and forwarding lower-tier guesses
/// would muddy its prompt. Returns an empty slice when no prior tier
/// ran (e.g. first tier in the pipeline) or no entities were extracted
/// for this memory.
fn collect_prior_entities<'a>(
    ctx: &'a ExtractionContext<'a>,
    memory_id: brain_core::MemoryId,
) -> Vec<&'a EntityMention> {
    let Some(map) = ctx.prior_tier_items else {
        return Vec::new();
    };
    let Some(items) = map.get(&memory_id) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| match item {
            ExtractedItem::EntityMention(em) => Some(em),
            _ => None,
        })
        .collect()
}

/// Constant role block sent on every LLM extractor call. Stable text
/// — Anthropic's ephemeral prompt cache will serve it as a cache hit
/// after the first call within the 5-minute window, slashing the
/// input-token cost of high-volume extraction.
const ROLE_BLOCK: &str = "You are an entity, statement, and relation extractor for Brain. \
Read the user's text and emit a JSON response that conforms to the response schema. \
Be conservative: only emit items you are confident about, and include a confidence score in [0.0, 1.0] on each. \
Do not invent entities that are not present in the text.";

/// Build the system blocks for an extractor call. Returns up to two
/// blocks, both marked `cache: true` so Anthropic's prompt cache
/// amortises the role + schema bytes across repeated calls. When the
/// operator did not declare examples we emit just the role block.
fn build_extractor_system_blocks(examples: Option<&Value>) -> Vec<SystemBlock> {
    let mut blocks = Vec::with_capacity(2);
    blocks.push(SystemBlock::cached(ROLE_BLOCK));
    if let Some(ex) = examples {
        let schema_text = format!(
            "Schema examples (the active schema declares these as canonical shapes):\n{}",
            serde_json::to_string_pretty(ex).unwrap_or_default(),
        );
        blocks.push(SystemBlock::cached(schema_text));
    }
    blocks
}

/// Side-channel stats returned alongside the rendered prompt so the
/// worker can publish `llm_neighbors_included` + `llm_tokens_per_query`
/// metrics without re-parsing the prompt body.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuildRequestStats {
    /// Number of neighbor entries that survived the per-call token
    /// budget and made it into the rendered prompt. Distinct from
    /// the fetch's `top_m` because the budget may have trimmed some.
    pub neighbors_included: usize,
    /// Whether the rolling summary made it into the prompt. False
    /// when the summary was absent, when the worker chose not to
    /// pass one, or when the budget gate dropped it.
    pub summary_included: bool,
    /// Approximate input-token count of the final request, computed
    /// via `LlmRequest::approx_input_tokens()` semantics
    /// (`chars / 4`). The worker observes this on the
    /// `llm_tokens_per_query` histogram.
    pub approx_input_tokens: u64,
}

/// Render the complete user-message body: the operator's template
/// (with `{TEXT}` + `{PRIOR_ENTITIES}` substituted) plus the W2.3
/// bounded-context sections (`Recent context`, optional
/// `Rolling summary`). The context sections precede the memory text
/// so a reader (and the LLM) sees the history first, then the
/// current memory.
fn render_full_prompt(
    template: &str,
    memory_text: &str,
    prior_entities: &[&EntityMention],
    neighbors: &[&NeighborMemory],
    summary: Option<&str>,
    now_unix_nanos: u64,
    memory_id: brain_core::MemoryId,
) -> String {
    let base = render_prompt(template, memory_text, prior_entities, memory_id);
    let mut bounded = String::new();
    if !neighbors.is_empty() {
        bounded.push_str(&format_recent_context_section(neighbors, now_unix_nanos));
    }
    if let Some(s) = summary {
        if !s.is_empty() {
            bounded.push_str(&format_rolling_summary_section(s));
        }
    }
    if bounded.is_empty() {
        return base;
    }
    // Prepend bounded-context sections so the LLM reads "what came
    // before" → "what's now" in source order.
    let mut out = String::with_capacity(bounded.len() + base.len() + 4);
    out.push_str(&bounded);
    out.push_str("\n\n");
    out.push_str(&base);
    out
}

/// Bullet-list of neighbor memories ranked by similarity, with a
/// truncated text body and a "T-Nh" relative-time hint so the LLM
/// can tell recent context from stale priors.
fn format_recent_context_section(neighbors: &[&NeighborMemory], now_unix_nanos: u64) -> String {
    let mut s = String::from("## Recent context\n\n");
    s.push_str(
        "The following are the most similar prior memories from the same \
         context, ranked by similarity. Treat them as background — only \
         emit statements / relations that follow from the current memory \
         text below.\n\n",
    );
    for (i, n) in neighbors.iter().enumerate() {
        let rel = relative_time_hint(now_unix_nanos, n.created_at_unix_nanos);
        let body = truncate_chars(&n.text, NEIGHBOR_TEXT_CHAR_CAP_IN_PROMPT);
        s.push_str(&format!(
            "{}. (similarity={:.2}, {}) \"{}\"\n",
            i + 1,
            n.similarity_score,
            rel,
            body
        ));
    }
    s.push('\n');
    s
}

/// Optional rolling-summary block — single paragraph, capped at
/// [`SUMMARY_CHAR_CAP`] chars so a misbehaving summarizer can't blow
/// the prompt budget single-handedly.
fn format_rolling_summary_section(summary: &str) -> String {
    let mut s = String::from("## Rolling summary\n\n");
    s.push_str(&truncate_chars(summary, SUMMARY_CHAR_CAP));
    s.push_str("\n\n");
    s
}

/// Render a "T-Nh" / "T-Nm" / "T-Ns" relative-time hint between two
/// monotonic timestamps. Both arguments are unix nanos; an `older >
/// now` case (clock skew) collapses to "T+0".
fn relative_time_hint(now_unix_nanos: u64, older_unix_nanos: u64) -> String {
    let Some(delta_ns) = now_unix_nanos.checked_sub(older_unix_nanos) else {
        return "T+0".into();
    };
    let delta_secs = delta_ns / 1_000_000_000;
    if delta_secs == 0 {
        return "T-0s".into();
    }
    let days = delta_secs / 86_400;
    if days > 0 {
        return format!("T-{days}d");
    }
    let hours = delta_secs / 3_600;
    if hours > 0 {
        return format!("T-{hours}h");
    }
    let minutes = delta_secs / 60;
    if minutes > 0 {
        return format!("T-{minutes}m");
    }
    format!("T-{delta_secs}s")
}

/// UTF-8-safe char-count truncation with an ellipsis suffix on
/// overflow. Mirrors the storage-layer helper of the same name in
/// `brain_ops::apply::encode_helpers`.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out = String::with_capacity(max_chars + 1);
    for ch in s.chars().take(max_chars) {
        out.push(ch);
    }
    out.push('…');
    out
}

/// Render the operator-declared prompt template with bounded LLM
/// context — the W2.3 surface. Builds the standard prompt body then
/// inserts `Recent context` + `Rolling summary` sections before the
/// memory text. Enforces the per-call token budget: if the rendered
/// request exceeds [`LLM_INPUT_TOKEN_BUDGET`] the function drops the
/// summary first, then trims neighbors from the lowest-similarity
/// end until the request fits.
fn render_prompt_with_context(
    template: &str,
    memory_text: &str,
    prior_entities: &[&EntityMention],
    extractor_context: Option<&ExtractorContext>,
    now_unix_nanos: u64,
    memory_id: brain_core::MemoryId,
) -> (String, BuildRequestStats) {
    // No context wired (or empty) → fall back to the no-context render
    // and skip budget enforcement (the no-context path is what the LLM
    // saw before W2.3; it's already inside the budget).
    let Some(ec) = extractor_context.filter(|c| !c.is_empty()) else {
        let body = render_prompt(template, memory_text, prior_entities, memory_id);
        let approx_input_tokens = approx_tokens_of(&body);
        return (
            body,
            BuildRequestStats {
                neighbors_included: 0,
                summary_included: false,
                approx_input_tokens,
            },
        );
    };

    // Order neighbors by descending similarity so trimming from the
    // tail drops the lowest-signal ones first. The fetch helper
    // returns retriever-order which is the same thing in practice,
    // but we re-sort defensively in case a future helper changes
    // ordering.
    let mut neighbors: Vec<&NeighborMemory> = ec.neighbors.iter().collect();
    neighbors.sort_by(|a, b| {
        b.similarity_score
            .partial_cmp(&a.similarity_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut include_summary = ec.summary.is_some();
    if ec.summary.is_none() {
        tracing::debug!(
            target: "brain_extractors::llm",
            memory_id = ?memory_id,
            "no rolling summary available; passing None to LLM extractor",
        );
    }

    // Render-loop: start with all neighbors and (when present) the
    // summary; if we're over budget, drop the summary; if still over,
    // peel neighbors off the tail until we fit. The memory text +
    // prior-entity block is the inviolable floor — neighbors[0..0]
    // and no summary still leaves the prompt the LLM needs to do its
    // primary job.
    let mut included = neighbors.len();
    loop {
        let body = render_full_prompt(
            template,
            memory_text,
            prior_entities,
            &neighbors[..included],
            if include_summary {
                ec.summary.as_deref()
            } else {
                None
            },
            now_unix_nanos,
            memory_id,
        );
        let tokens = approx_tokens_of(&body);
        if tokens <= LLM_INPUT_TOKEN_BUDGET {
            return (
                body,
                BuildRequestStats {
                    neighbors_included: included,
                    summary_included: include_summary,
                    approx_input_tokens: tokens,
                },
            );
        }
        if include_summary {
            include_summary = false;
            continue;
        }
        if included == 0 {
            // Floor hit. Return what we have — the prompt is still
            // the canonical memory + prior-entity block, the budget
            // just couldn't be met. Operators see this via the
            // `llm_tokens_per_query` p99.
            return (
                body,
                BuildRequestStats {
                    neighbors_included: 0,
                    summary_included: false,
                    approx_input_tokens: tokens,
                },
            );
        }
        included -= 1;
    }
}

/// Approximate input tokens for `s` using the same `chars / 4`
/// heuristic [`LlmRequest::approx_input_tokens`] uses. The system
/// blocks contribute a fixed cost across every call (cached after
/// the first); the per-call budget is dominated by the user message
/// so estimating off the user body alone is conservative enough for
/// gating.
fn approx_tokens_of(s: &str) -> u64 {
    (s.chars().count() as u64) / 4
}

/// Render the operator-declared prompt template against the runtime
/// values for this invocation. Two placeholders are supported:
///
/// - `{TEXT}` — the memory's text.
/// - `{PRIOR_ENTITIES}` — bullet list of entity mentions produced by
///   earlier tiers (pattern + classifier) for this memory. Empty
///   when no prior entities exist.
///
/// Templates that don't include `{TEXT}` get the standard input
/// section appended so prompts authored before the templating layer
/// landed still see the memory text. `{PRIOR_ENTITIES}` is silently
/// no-op when absent — when an operator's prompt doesn't anchor on
/// prior tier output, the LLM falls back to its own extraction.
fn render_prompt(
    template: &str,
    memory_text: &str,
    prior_entities: &[&EntityMention],
    memory_id: brain_core::MemoryId,
) -> String {
    let prior_section = if prior_entities.is_empty() {
        String::new()
    } else {
        format_prior_entities_section(prior_entities)
    };
    let mut out = template.to_string();
    let has_text_placeholder = out.contains("{TEXT}");
    out = out.replace("{TEXT}", memory_text);
    out = out.replace("{PRIOR_ENTITIES}", prior_section.trim_end());
    // Detect placeholders the runtime didn't fill. We log+strip rather
    // than panic — a stuck `{FOO}` in a prompt is a configuration bug
    // an operator can fix without losing the in-flight extraction.
    if let Some(unfilled) = find_unfilled_placeholder(&out) {
        tracing::warn!(
            target: "brain_extractors::llm",
            memory_id = ?memory_id,
            placeholder = %unfilled,
            "llm prompt template contains an unfilled placeholder; leaving literal text in place",
        );
    }
    if !has_text_placeholder {
        // Fallback: append the standard text section so prompts that
        // predate the templating layer still receive the memory body.
        out.push_str("\n\nInput text:\n```\n");
        out.push_str(memory_text);
        out.push_str("\n```");
    }
    out
}

fn format_prior_entities_section(prior_entities: &[&EntityMention]) -> String {
    let mut s = String::from("\n\nPreviously extracted entities for this text:\n");
    for em in prior_entities {
        s.push_str(&format!(
            "- \"{}\" -> {} (confidence {:.2})\n",
            em.text, em.entity_type_qname, em.confidence
        ));
    }
    s.push_str(
        "\nUse these entities verbatim as subjects/objects in your output. \
         Do not re-extract them as new entities; only emit statements \
         and relations that involve them.\n",
    );
    s
}

/// Return the first `{IDENT}` placeholder in `s` whose ident is
/// uppercase + underscores only — that's the convention the templating
/// layer reserves. Lowercase / mixed-case curly fragments are left
/// alone so JSON examples in the prompt body don't get flagged.
fn find_unfilled_placeholder(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'}' {
                j += 1;
            }
            if j < bytes.len() && j > start {
                let ident = &s[start..j];
                let looks_reserved = !ident.is_empty()
                    && ident
                        .chars()
                        .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit());
                if looks_reserved {
                    return Some(ident.to_string());
                }
            }
            i = j + 1;
        } else {
            i += 1;
        }
    }
    None
}

fn kind_to_byte(k: StatementKindAst) -> u8 {
    match k {
        StatementKindAst::Fact => 1,
        StatementKindAst::Preference => 2,
        StatementKindAst::Event => 3,
        // `Any` only appears in query AST; extractors targeting
        // `Any` default to Fact (1).
        StatementKindAst::Any => 1,
    }
}

fn read_str(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(String::from)
}

fn read_conf(v: &Value) -> f32 {
    v.get("confidence")
        .and_then(Value::as_f64)
        .map(|f| f as f32)
        .unwrap_or(1.0)
}

fn project_entity(
    v: &Value,
    entity_type: &str,
    extractor_id: u32,
    extractor_version: u32,
) -> Option<ExtractedItem> {
    let text = if let Some(s) = v.as_str() {
        s.to_string()
    } else {
        read_str(v, "name").or_else(|| read_str(v, "text"))?
    };
    Some(ExtractedItem::EntityMention(EntityMention {
        entity_type_qname: entity_type.to_string(),
        text,
        start: 0,
        end: 0,
        confidence: read_conf(v),
        extractor_id,
        extractor_version,
    }))
}

fn project_statement(
    v: &Value,
    kind: u8,
    extractor_id: u32,
    extractor_version: u32,
) -> Option<ExtractedItem> {
    let predicate = read_str(v, "predicate")?;
    // LLM may emit `"is_stateful": true` to mark a per-extraction
    // statefulness signal. Default false: most LLM-coined facts are
    // cumulative observations, not stateful settings.
    let is_stateful = v
        .get("is_stateful")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(ExtractedItem::StatementMention(StatementMention {
        kind,
        subject_text: read_str(v, "subject"),
        predicate_qname: predicate,
        object_text: read_str(v, "object"),
        confidence: read_conf(v),
        extractor_id,
        extractor_version,
        is_stateful,
    }))
}

/// Project a statement entry from the structured-output shape
/// `{ "statements": [...] }`. Reads the predicate qname directly off
/// the LLM's emission so a single call can produce statements across
/// any declared predicate; the schema-gating in the worker takes care
/// of routing unknown predicates to `brain:fact`.
fn project_statement_open(
    v: &Value,
    extractor_id: u32,
    extractor_version: u32,
) -> Option<ExtractedItem> {
    let predicate = read_str(v, "predicate")?;
    let is_stateful = v
        .get("is_stateful")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(ExtractedItem::StatementMention(StatementMention {
        kind: 1, // Fact by default; predicate registry's declared kind wins downstream.
        subject_text: read_str(v, "subject"),
        predicate_qname: predicate,
        object_text: read_str(v, "object"),
        confidence: read_conf(v),
        extractor_id,
        extractor_version,
        is_stateful,
    }))
}

/// Project a relation entry from the structured-output shape
/// `{ "relations": [...] }`. Reads the relation_type qname directly
/// off the LLM's emission.
fn project_relation_open(
    v: &Value,
    extractor_id: u32,
    extractor_version: u32,
) -> Option<ExtractedItem> {
    let relation_type = read_str(v, "relation_type").or_else(|| read_str(v, "type"))?;
    let subject = read_str(v, "from").or_else(|| read_str(v, "subject"))?;
    let object = read_str(v, "to").or_else(|| read_str(v, "object"))?;
    Some(ExtractedItem::RelationMention(RelationMention {
        relation_type_qname: relation_type,
        subject_text: subject,
        object_text: object,
        confidence: read_conf(v),
        extractor_id,
        extractor_version,
    }))
}

fn project_relation(
    v: &Value,
    relation_type: &str,
    extractor_id: u32,
    extractor_version: u32,
) -> Option<ExtractedItem> {
    let subject = read_str(v, "from").or_else(|| read_str(v, "subject"))?;
    let object = read_str(v, "to").or_else(|| read_str(v, "object"))?;
    Some(ExtractedItem::RelationMention(RelationMention {
        relation_type_qname: relation_type.to_string(),
        subject_text: subject,
        object_text: object,
        confidence: read_conf(v),
        extractor_id,
        extractor_version,
    }))
}

// ---------------------------------------------------------------------------
// Cache helpers.
// ---------------------------------------------------------------------------

fn cache_get(
    cache: &Arc<Mutex<LlmCacheDb>>,
    input_hash: [u8; 32],
    extractor_id: u32,
    extractor_version: u32,
    model_id_hash: u64,
) -> Option<CachedResponse> {
    let db = cache.lock();
    let rtxn = db.read_txn().ok()?;
    let t = rtxn.open_table(LLM_RESPONSES_TABLE).ok()?;
    let key = (input_hash, extractor_id, extractor_version, model_id_hash);
    let row = t.get(&key).ok().flatten()?;
    Some(row.value())
}

#[allow(clippy::too_many_arguments)]
fn cache_put(
    cache: &Arc<Mutex<LlmCacheDb>>,
    input_hash: [u8; 32],
    extractor_id: u32,
    extractor_version: u32,
    model_id_hash: u64,
    response_blob: Vec<u8>,
    token_count: u32,
    now_nanos: u64,
    ttl: Duration,
) -> Result<(), String> {
    let mut db = cache.lock();
    let wtxn = db
        .write_txn()
        .map_err(|e| format!("cache write_txn: {e}"))?;
    let key = (input_hash, extractor_id, extractor_version, model_id_hash);
    let expires_at_nanos = now_nanos.saturating_add(ttl.as_nanos() as u64);
    let value = CachedResponse::new(
        response_blob,
        now_nanos,
        expires_at_nanos,
        token_count,
        model_id_hash,
    );
    {
        let mut tbl = wtxn
            .open_table(LLM_RESPONSES_TABLE)
            .map_err(|e| format!("cache open_table: {e}"))?;
        tbl.insert(&key, &value)
            .map_err(|e| format!("cache insert: {e}"))?;
    }
    // Index entry for the periodic sweep worker. The TTL table is keyed by
    // (expiry_unix_secs, input_hash) so the sweeper can range-scan
    // `expiry <= now` cheaply. Without this insert the main table would
    // grow unbounded — the sweeper has no other way to find expired rows.
    {
        let expiry_secs = expires_at_nanos / 1_000_000_000;
        let ttl_key: brain_metadata::llm_cache::LlmTtlKey = (expiry_secs, input_hash);
        let mut ttl_tbl = wtxn
            .open_table(brain_metadata::llm_cache::LLM_RESPONSE_TTL_TABLE)
            .map_err(|e| format!("cache ttl open_table: {e}"))?;
        ttl_tbl
            .insert(&ttl_key, &())
            .map_err(|e| format!("cache ttl insert: {e}"))?;
    }
    wtxn.commit().map_err(|e| format!("cache commit: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Validation + retry.
// ---------------------------------------------------------------------------

fn validate_against(schema: &JSONSchema, content: &str) -> Result<Value, String> {
    let parsed: Value =
        serde_json::from_str(content).map_err(|e| format!("response is not valid JSON: {e}"))?;
    if let Err(mut errs) = schema.validate(&parsed) {
        let msg = match errs.next() {
            Some(e) => e.to_string(),
            None => "unknown validation failure".into(),
        };
        return Err(msg);
    }
    Ok(parsed)
}

// ---------------------------------------------------------------------------
// Extractor impl.
// ---------------------------------------------------------------------------

impl Extractor for LlmExtractor {
    fn id(&self) -> ExtractorId {
        self.id
    }

    fn kind(&self) -> ExtractorKind {
        ExtractorKind::Llm
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn extractor_version(&self) -> u32 {
        self.extractor_version
    }

    fn is_wired(&self) -> bool {
        self.inner.is_some()
    }

    fn run<'a>(&'a self, ctx: &'a ExtractionContext<'a>, mem: &'a Memory) -> ExtractionFuture<'a> {
        Box::pin(async move {
            let started = ctx.now_unix_nanos;
            let Some(inner) = self.inner.as_ref() else {
                let reason = self
                    .degraded_reason
                    .as_deref()
                    .unwrap_or("llm extractor not wired");
                return ExtractionResult::skipped(
                    ExtractionStatus::SkippedDisabled,
                    reason,
                    started,
                );
            };
            let inner = inner.clone();
            let text = mem.text.as_deref().unwrap_or("");
            let input_hash = hash_memory_text(text);
            let model_id_hash = inner.client.model_id_hash();
            let extractor_id_raw = self.id.raw();
            let extractor_version = self.extractor_version;

            // ----- 1. Cache lookup ---------------------------------------------
            if let Some(cache) = inner.cache.as_ref() {
                if let Some(row) = cache_get(
                    cache,
                    input_hash,
                    extractor_id_raw,
                    extractor_version,
                    model_id_hash,
                ) {
                    return match decode_cached(&row.response_blob, inner.response_schema.as_ref()) {
                        Ok(parsed) => {
                            let items = self.project_value(&parsed);
                            ExtractionResult::success(items, started, started)
                        }
                        Err(e) => ExtractionResult::failure(
                            format!("cache decode failed: {e}"),
                            started,
                            started,
                        ),
                    };
                }
            }

            // ----- 2. Build request + cost budget ------------------------------
            // Pull prior-tier entity mentions for this memory so the
            // LLM gets canonical anchors without re-extracting them.
            // Statement / relation mentions from earlier tiers are
            // intentionally NOT forwarded: the LLM owns predicate /
            // relation extraction, and feeding back the classifier's
            // guesses would just confuse it.
            let prior_entities = collect_prior_entities(ctx, mem.id);
            // W2.3 added bounded inferential context to the per-call
            // user message. The context map is keyed by memory id and
            // may be `None` (no neighbours computed) or absent for
            // this memory (no relevant neighbours found).
            let extractor_context = ctx.extractor_context.and_then(|map| map.get(&mem.id));
            let (mut request, _stats) = self.build_request(
                &inner,
                mem.id,
                text,
                &prior_entities,
                extractor_context,
                started,
            );
            if let Some(budget) = self.cost_budget {
                let est = estimate_cost(&request, &inner.pricing);
                if est > budget.per_call_micro_usd {
                    let reason = format!(
                        "estimated {} µ$ exceeds per-call budget {} µ$",
                        est, budget.per_call_micro_usd
                    );
                    return ExtractionResult {
                        items: Vec::new(),
                        status: ExtractionStatus::SkippedBudget,
                        status_reason: reason,
                        started_at_unix_nanos: started,
                        completed_at_unix_nanos: started,
                    };
                }
            }

            // ----- 3. First LLM call -------------------------------------------
            let resp1 = match inner.client.complete(request.clone()).await {
                Ok(r) => r,
                Err(e) => {
                    return ExtractionResult::failure(llm_error_reason(&e), started, started);
                }
            };

            // ----- 4. Validate + retry-once ------------------------------------
            let parsed = match inner.schema_compiled.as_ref() {
                None => match serde_json::from_str::<Value>(&resp1.content) {
                    Ok(v) => v,
                    Err(_) => Value::String(resp1.content.clone()),
                },
                Some(schema) => match validate_against(schema, &resp1.content) {
                    Ok(v) => v,
                    Err(err1) => {
                        // Retry with the validation error in the prompt.
                        request.messages.push(brain_llm::LlmMessage {
                            role: brain_llm::LlmRole::Assistant,
                            content: resp1.content.clone(),
                        });
                        request.messages.push(brain_llm::LlmMessage {
                            role: brain_llm::LlmRole::User,
                            content: format!(
                                "Your previous response did not match the expected schema. \
                                 Error: {err1}. Please retry with valid JSON."
                            ),
                        });
                        let resp2 = match inner.client.complete(request).await {
                            Ok(r) => r,
                            Err(e) => {
                                return ExtractionResult::failure(
                                    llm_error_reason(&e),
                                    started,
                                    started,
                                );
                            }
                        };
                        match validate_against(schema, &resp2.content) {
                            Ok(v) => v,
                            Err(_) => {
                                return ExtractionResult::failure(
                                    "schema validation failed twice",
                                    started,
                                    started,
                                );
                            }
                        }
                    }
                },
            };

            // ----- 5. Cache write ----------------------------------------------
            if let Some(cache) = inner.cache.as_ref() {
                let blob = parsed.to_string().into_bytes();
                let token_count = (resp1.tokens_in + resp1.tokens_out)
                    .try_into()
                    .unwrap_or(u32::MAX);
                if let Err(e) = cache_put(
                    cache,
                    input_hash,
                    extractor_id_raw,
                    extractor_version,
                    model_id_hash,
                    blob,
                    token_count,
                    started,
                    self.cache_ttl,
                ) {
                    tracing::warn!(
                        target: "brain_extractors::llm",
                        extractor_id = extractor_id_raw,
                        error = %e,
                        "llm cache write failed; continuing",
                    );
                }
            }

            // ----- 6. Project to ExtractedItem[] -------------------------------
            let items = self.project_value(&parsed);
            ExtractionResult::success(items, started, started)
        })
    }
}

fn decode_cached(blob: &[u8], _schema: Option<&Value>) -> Result<Value, String> {
    let s = std::str::from_utf8(blob).map_err(|e| format!("non-UTF8 blob: {e}"))?;
    serde_json::from_str::<Value>(s).map_err(|e| format!("blob json parse: {e}"))
}

fn llm_error_reason(e: &LlmError) -> String {
    if let Some(retry_after) = e.retry_after_ms() {
        format!("rate limited: retry after {retry_after} ms")
    } else {
        e.to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::ExtractorRegistry;
    use brain_core::{AgentId, ContextId, MemoryId, MemoryKind, Salience};
    use brain_llm::client::LlmFuture;
    use brain_llm::LlmResponse;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // ------------------------------------------------------------------- mock

    struct MockClient {
        model: String,
        responses: parking_lot::Mutex<Vec<Result<LlmResponse, LlmError>>>,
        calls: Arc<AtomicUsize>,
    }

    impl MockClient {
        fn new(model: &str, responses: Vec<Result<LlmResponse, LlmError>>) -> Self {
            Self {
                model: model.into(),
                responses: parking_lot::Mutex::new(responses),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl LlmClient for MockClient {
        fn complete<'a>(&'a self, _request: LlmRequest) -> LlmFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let next = self.responses.lock().drain(..1).next();
            Box::pin(async move {
                next.unwrap_or_else(|| {
                    Err(LlmError::ProviderError {
                        status: 500,
                        message: "mock: no more responses queued".into(),
                    })
                })
            })
        }

        fn model(&self) -> &str {
            &self.model
        }

        fn model_id_hash(&self) -> u64 {
            brain_llm::client::model_id_hash(&self.model)
        }
    }

    fn ok_response(json: &str, tokens: u64) -> LlmResponse {
        LlmResponse {
            content: json.into(),
            tokens_in: tokens / 2,
            tokens_out: tokens / 2,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
            cost_micro_usd: tokens * 2,
            model_version: "mock-model-v1".into(),
        }
    }

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
            now_unix_nanos: 100,
            registry: reg,
            prior_tier_items: None,
            extractor_context: None,
        }
    }

    fn build_ext(
        client: Arc<dyn LlmClient>,
        cache: Option<Arc<Mutex<LlmCacheDb>>>,
        schema: Option<Value>,
        budget: Option<CostBudget>,
    ) -> LlmExtractor {
        let schema_compiled = LlmExtractor::compile_schema(schema.as_ref()).unwrap();
        LlmExtractor::new(
            ExtractorId::from(99),
            "acme:llm_test".into(),
            entity_target(),
            1,
            0.5,
            budget,
            Duration::from_secs(60),
            LlmExtractorInner {
                client,
                cache,
                prompt: "Extract people".into(),
                examples: None,
                response_schema: schema,
                schema_compiled,
                pricing: Pricing::for_model("claude-haiku-4-5"),
                max_tokens: 1024,
                temperature: 0.0,
                timeout: Duration::from_secs(30),
            },
        )
    }

    // ------------------------------------------------------------------- tests

    #[test]
    fn degraded_dispatch_writes_skipped_disabled() {
        let reg = ExtractorRegistry::new();
        let ext = LlmExtractor::degraded(
            ExtractorId::from(1),
            "acme:degraded".into(),
            entity_target(),
            1,
            0.5,
            "no client configured for model unknown-x",
        );
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("anything")));
        // An unconfigured LLM tier is *not* a failure — it never
        // tried. Reporting `Failure` here cascades into the pipeline
        // classifier and produces a misleading "partially applied"
        // audit on otherwise-clean runs.
        assert_eq!(r.status, ExtractionStatus::SkippedDisabled);
        assert!(r.status_reason.contains("no client configured"));
        assert!(r.items.is_empty());
    }

    #[test]
    fn cost_budget_skips_call() {
        let client = Arc::new(MockClient::new(
            "claude-haiku-4-5",
            vec![Ok(ok_response("[\"Alice\"]", 200))],
        ));
        let calls = client.calls.clone();
        let ext = build_ext(
            client,
            None,
            None,
            Some(CostBudget {
                per_call_micro_usd: 1,
            }),
        );
        let reg = ExtractorRegistry::new();
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("Alice met Bob")));
        assert_eq!(r.status, ExtractionStatus::SkippedBudget);
        assert!(r.status_reason.contains("exceeds per-call budget"));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "no LLM call when over budget"
        );
    }

    #[test]
    fn success_no_schema_parses_json_array() {
        let client = Arc::new(MockClient::new(
            "claude-haiku-4-5",
            vec![Ok(ok_response("[\"Alice\",\"Bob\"]", 50))],
        ));
        let ext = build_ext(client, None, None, None);
        let reg = ExtractorRegistry::new();
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("Alice met Bob")));
        assert_eq!(r.status, ExtractionStatus::Success);
        assert_eq!(r.items.len(), 2);
        match &r.items[0] {
            ExtractedItem::EntityMention(m) => {
                assert_eq!(m.text, "Alice");
                assert_eq!(m.entity_type_qname, "brain:Person");
            }
            other => panic!("expected entity, got {other:?}"),
        }
    }

    #[test]
    fn confidence_below_threshold_filtered() {
        let body = "[{\"name\":\"Alice\",\"confidence\":0.9}, \
                   {\"name\":\"X\",\"confidence\":0.1}]";
        let client = Arc::new(MockClient::new(
            "claude-haiku-4-5",
            vec![Ok(ok_response(body, 50))],
        ));
        let ext = build_ext(client, None, None, None);
        let reg = ExtractorRegistry::new();
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("Alice")));
        assert_eq!(r.status, ExtractionStatus::Success);
        // Only the high-confidence one survives the 0.5 threshold.
        assert_eq!(r.items.len(), 1);
    }

    #[test]
    fn schema_validation_failure_retries_once() {
        // First response is not an array of objects with `name`;
        // second response is the well-formed one.
        let schema = serde_json::json!({
            "type": "array",
            "items": {
                "type": "object",
                "properties": {"name": {"type": "string"}},
                "required": ["name"],
            },
        });
        let bad = ok_response("[\"plain string\"]", 50);
        let good = ok_response("[{\"name\":\"Alice\"}]", 50);
        let client = Arc::new(MockClient::new("claude-haiku-4-5", vec![Ok(bad), Ok(good)]));
        let calls = client.calls.clone();
        let ext = build_ext(client, None, Some(schema), None);
        let reg = ExtractorRegistry::new();
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("Alice")));
        assert_eq!(r.status, ExtractionStatus::Success);
        assert_eq!(r.items.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2, "retried exactly once");
    }

    #[test]
    fn schema_validation_failure_twice_returns_failure() {
        let schema = serde_json::json!({
            "type": "array",
            "items": {"type": "object", "required": ["name"]},
        });
        let bad1 = ok_response("[\"x\"]", 50);
        let bad2 = ok_response("[\"y\"]", 50);
        let client = Arc::new(MockClient::new(
            "claude-haiku-4-5",
            vec![Ok(bad1), Ok(bad2)],
        ));
        let ext = build_ext(client, None, Some(schema), None);
        let reg = ExtractorRegistry::new();
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("hello")));
        assert_eq!(r.status, ExtractionStatus::Failure);
        assert!(r.status_reason.contains("schema validation failed twice"));
    }

    #[test]
    fn rate_limit_error_surfaces_retry_after() {
        let client = Arc::new(MockClient::new(
            "claude-haiku-4-5",
            vec![Err(LlmError::RateLimit {
                retry_after_ms: 1500,
            })],
        ));
        let ext = build_ext(client, None, None, None);
        let reg = ExtractorRegistry::new();
        let r = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("hi")));
        assert_eq!(r.status, ExtractionStatus::Failure);
        assert!(r.status_reason.contains("1500"));
    }

    #[test]
    fn cache_hit_skips_llm_call() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Mutex::new(
            LlmCacheDb::open(dir.path().join("llm_cache.redb")).unwrap(),
        ));

        // Round 1: real call populates cache.
        let client = Arc::new(MockClient::new(
            "claude-haiku-4-5",
            vec![Ok(ok_response("[\"Alice\"]", 50))],
        ));
        let calls = client.calls.clone();
        let ext = build_ext(client.clone(), Some(cache.clone()), None, None);
        let reg = ExtractorRegistry::new();
        let _ = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("Alice")));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        // Round 2: same input, new client with no responses queued.
        // Cache hit must short-circuit.
        let client2 = Arc::new(MockClient::new("claude-haiku-4-5", vec![]));
        let calls2 = client2.calls.clone();
        let ext2 = build_ext(client2, Some(cache.clone()), None, None);
        let r = futures_lite::future::block_on(ext2.run(&ctx(&reg), &memory("Alice")));
        assert_eq!(r.status, ExtractionStatus::Success);
        assert_eq!(r.items.len(), 1);
        assert_eq!(
            calls2.load(Ordering::SeqCst),
            0,
            "cache hit: zero LLM calls"
        );
    }

    #[test]
    fn cache_miss_writes_through() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Arc::new(Mutex::new(
            LlmCacheDb::open(dir.path().join("llm_cache.redb")).unwrap(),
        ));
        let client = Arc::new(MockClient::new(
            "claude-haiku-4-5",
            vec![Ok(ok_response("[\"Alice\"]", 50))],
        ));
        let ext = build_ext(client, Some(cache.clone()), None, None);
        let reg = ExtractorRegistry::new();
        let _ = futures_lite::future::block_on(ext.run(&ctx(&reg), &memory("Alice")));

        // Verify the row landed.
        let db = cache.lock();
        let rtxn = db.read_txn().unwrap();
        let t = rtxn.open_table(LLM_RESPONSES_TABLE).unwrap();
        let key = (
            hash_memory_text("Alice"),
            99u32,
            1u32,
            brain_llm::client::model_id_hash("claude-haiku-4-5"),
        );
        assert!(t.get(&key).unwrap().is_some(), "cache row present");
    }

    #[test]
    fn estimate_cost_honours_pricing() {
        let mut req = LlmRequest::new("claude-haiku-4-5", "abcdefgh"); // 8 chars → 2 in-tokens
        req.max_tokens = 100;
        let p = Pricing::for_model("claude-haiku-4-5");
        // 2 * 1 + 100 * 5 = 502
        assert_eq!(estimate_cost(&req, &p), 502);
    }

    #[test]
    fn pricing_unknown_model_uses_conservative() {
        let p = Pricing::for_model("custom-llm");
        assert_eq!(p, Pricing::conservative_default());
    }

    // ------------------------------------------------------------------
    // Prior-tier prompt injection.
    // ------------------------------------------------------------------

    fn em_fixture(text: &str, kind: &str, confidence: f32) -> EntityMention {
        EntityMention {
            entity_type_qname: kind.into(),
            text: text.into(),
            start: 0,
            end: text.len(),
            confidence,
            extractor_id: 1,
            extractor_version: 1,
        }
    }

    fn sm_fixture() -> StatementMention {
        StatementMention {
            kind: 1,
            subject_text: Some("X".into()),
            predicate_qname: "brain:fact".into(),
            object_text: Some("Y".into()),
            confidence: 0.9,
            extractor_id: 1,
            extractor_version: 1,
            is_stateful: false,
        }
    }

    fn ext_with_prompt(prompt: &str) -> LlmExtractor {
        let client = Arc::new(MockClient::new("claude-haiku-4-5", vec![]));
        let schema_compiled = LlmExtractor::compile_schema(None).unwrap();
        LlmExtractor::new(
            ExtractorId::from(42),
            "acme:prompt_test".into(),
            entity_target(),
            1,
            0.0,
            None,
            Duration::from_secs(60),
            LlmExtractorInner {
                client,
                cache: None,
                prompt: prompt.into(),
                examples: None,
                response_schema: None,
                schema_compiled,
                pricing: Pricing::for_model("claude-haiku-4-5"),
                max_tokens: 1024,
                temperature: 0.0,
                timeout: Duration::from_secs(30),
            },
        )
    }

    #[test]
    fn build_request_injects_prior_entities_into_prompt() {
        let prompt = "DO YOUR JOB.\n{PRIOR_ENTITIES}\nText: {TEXT}";
        let ext = ext_with_prompt(prompt);
        let inner = ext.inner.as_ref().unwrap().clone();
        let priors = vec![
            em_fixture("Alice Wong", "brain:Person", 0.96),
            em_fixture("Acme Corp", "brain:Organization", 0.94),
            em_fixture("Bengaluru", "brain:Place", 0.91),
        ];
        let prior_refs: Vec<&EntityMention> = priors.iter().collect();
        let (req, _) = ext.build_request(
            &inner,
            brain_core::MemoryId::pack(0, 1, 0),
            "Alice Wong works at Acme Corp from Bengaluru.",
            &prior_refs,
            None,
            0,
        );
        let body = &req.messages[0].content;
        assert!(
            body.contains("Previously extracted entities for this text:"),
            "prompt missing prior-entities heading: {body}",
        );
        for em in &priors {
            assert!(
                body.contains(&em.text),
                "prompt missing entity surface {}: {body}",
                em.text,
            );
            assert!(
                body.contains(&em.entity_type_qname),
                "prompt missing entity type {}: {body}",
                em.entity_type_qname,
            );
        }
        assert!(
            body.contains("verbatim"),
            "prompt missing reuse-anchor instruction: {body}",
        );
        assert!(
            body.contains("Alice Wong works at Acme Corp from Bengaluru."),
            "prompt missing memory text: {body}",
        );
    }

    #[test]
    fn build_request_with_empty_prior_entities_omits_section() {
        let prompt = "DO YOUR JOB.\n{PRIOR_ENTITIES}\nText: {TEXT}";
        let ext = ext_with_prompt(prompt);
        let inner = ext.inner.as_ref().unwrap().clone();
        let priors: Vec<&EntityMention> = Vec::new();
        let (req, _) = ext.build_request(
            &inner,
            brain_core::MemoryId::pack(0, 2, 0),
            "Plain text.",
            &priors,
            None,
            0,
        );
        let body = &req.messages[0].content;
        assert!(
            !body.contains("Previously extracted entities for this text:"),
            "no priors => no heading; got: {body}",
        );
        assert!(body.contains("Plain text."));
    }

    #[test]
    fn build_request_filters_non_entity_items_from_prior() {
        let prompt = "{PRIOR_ENTITIES}\n{TEXT}";
        let ext = ext_with_prompt(prompt);
        let inner = ext.inner.as_ref().unwrap().clone();
        // Two entities + one statement; only the entities should land in
        // the prompt section because the LLM tier owns predicate
        // extraction and should not see lower-tier predicate guesses.
        let em1 = em_fixture("Alice", "brain:Person", 0.8);
        let em2 = em_fixture("Acme", "brain:Organization", 0.7);
        let priors: Vec<&EntityMention> = vec![&em1, &em2];
        let (req, _) = ext.build_request(
            &inner,
            brain_core::MemoryId::pack(0, 3, 0),
            "Alice met Acme.",
            &priors,
            None,
            0,
        );
        let body = &req.messages[0].content;
        assert!(body.contains("\"Alice\""));
        assert!(body.contains("\"Acme\""));
        // A StatementMention's `brain:fact` predicate should not have
        // been forwarded — it isn't in the prior_refs slice at all.
        assert!(
            !body.contains("brain:fact"),
            "statements must not appear in the prior-entities section: {body}",
        );

        // Independently verify the filter at the data layer: a
        // collect_prior_entities call with a HashMap containing a
        // StatementMention must drop it.
        let mut map = HashMap::new();
        let mid = brain_core::MemoryId::pack(0, 9, 0);
        map.insert(
            mid,
            vec![
                ExtractedItem::EntityMention(em1.clone()),
                ExtractedItem::EntityMention(em2.clone()),
                ExtractedItem::StatementMention(sm_fixture()),
            ],
        );
        let reg = ExtractorRegistry::new();
        let ctx = ExtractionContext {
            schema_version: 1,
            now_unix_nanos: 1,
            registry: &reg,
            prior_tier_items: Some(&map),
            extractor_context: None,
        };
        let collected = collect_prior_entities(&ctx, mid);
        assert_eq!(collected.len(), 2, "statement must be filtered out");
    }

    #[test]
    fn build_request_splits_into_cached_blocks() {
        // Operator declared examples ⇒ schema block is populated.
        // Both blocks must be flagged for prompt caching so Anthropic
        // amortises their input-token cost across calls.
        let examples = serde_json::json!({
            "entity_types": [
                "brain:Person",
                "brain:Organization",
                "brain:Place",
                "brain:Event",
                "brain:Product",
                "brain:Topic",
            ],
            "predicates": ["brain:fact", "brain:prefers", "brain:knows"],
        });
        let client = Arc::new(MockClient::new("claude-haiku-4-5", vec![]));
        let schema_compiled = LlmExtractor::compile_schema(None).unwrap();
        let ext = LlmExtractor::new(
            ExtractorId::from(7),
            "acme:llm_split".into(),
            entity_target(),
            1,
            0.0,
            None,
            Duration::from_secs(60),
            LlmExtractorInner {
                client,
                cache: None,
                prompt: "Extract things.\n{TEXT}".into(),
                examples: Some(examples),
                response_schema: None,
                schema_compiled,
                pricing: Pricing::for_model("claude-haiku-4-5"),
                max_tokens: 1024,
                temperature: 0.0,
                timeout: Duration::from_secs(30),
            },
        );
        let inner = ext.inner.as_ref().unwrap().clone();
        let (req, _) = ext.build_request(
            &inner,
            brain_core::MemoryId::pack(0, 1, 0),
            "Alice met Bob.",
            &[],
            None,
            0,
        );
        assert_eq!(
            req.system_blocks.len(),
            2,
            "schema declared => role + schema blocks",
        );
        assert!(
            req.system_blocks.iter().all(|b| b.cache),
            "both blocks must be cache-tagged",
        );
        assert!(
            req.system_blocks[0].text.contains("Brain"),
            "first block is the role block",
        );
        assert!(
            req.system_blocks[1].text.contains("brain:Person"),
            "second block carries the schema/examples payload",
        );
        // Per-call body (memory text) is in the user message, NOT in
        // a system block — keeping it out of the cached prefix is what
        // makes the cache hit ratio meaningful.
        assert_eq!(req.messages.len(), 1);
        assert!(req.messages[0].content.contains("Alice met Bob."));
        // The dynamic body must not leak into either cached block.
        for b in &req.system_blocks {
            assert!(
                !b.text.contains("Alice met Bob."),
                "per-call text must not appear in cached system block: {}",
                b.text,
            );
        }
    }

    #[test]
    fn build_request_without_examples_emits_role_block_only() {
        // No schema declared ⇒ just the constant role block, still
        // cached so the LLM's "you are a Brain extractor" preamble
        // amortises across calls even in degraded schema mode.
        let client = Arc::new(MockClient::new("claude-haiku-4-5", vec![]));
        let schema_compiled = LlmExtractor::compile_schema(None).unwrap();
        let ext = LlmExtractor::new(
            ExtractorId::from(8),
            "acme:llm_noschema".into(),
            entity_target(),
            1,
            0.0,
            None,
            Duration::from_secs(60),
            LlmExtractorInner {
                client,
                cache: None,
                prompt: "Extract.\n{TEXT}".into(),
                examples: None,
                response_schema: None,
                schema_compiled,
                pricing: Pricing::for_model("claude-haiku-4-5"),
                max_tokens: 1024,
                temperature: 0.0,
                timeout: Duration::from_secs(30),
            },
        );
        let inner = ext.inner.as_ref().unwrap().clone();
        let (req, _) = ext.build_request(
            &inner,
            brain_core::MemoryId::pack(0, 1, 0),
            "Hello.",
            &[],
            None,
            0,
        );
        assert_eq!(req.system_blocks.len(), 1);
        assert!(req.system_blocks[0].cache);
    }

    #[test]
    fn render_prompt_falls_back_to_appending_text_when_no_text_placeholder() {
        let prompt = "EXTRACT NOW.";
        let memory_id = brain_core::MemoryId::pack(0, 5, 0);
        let out = render_prompt(prompt, "the body", &[], memory_id);
        assert!(out.starts_with("EXTRACT NOW."));
        assert!(
            out.contains("Input text:\n```\nthe body\n```"),
            "legacy prompts without {{TEXT}} must still see the memory text: {out}",
        );
    }

    #[test]
    fn find_unfilled_placeholder_flags_reserved_idents() {
        assert_eq!(
            find_unfilled_placeholder("nothing here"),
            None,
            "plain text => no placeholder",
        );
        assert_eq!(
            find_unfilled_placeholder("{FOO}"),
            Some("FOO".into()),
            "uppercase ident => flagged",
        );
        assert_eq!(
            find_unfilled_placeholder("{lower}"),
            None,
            "lowercase ident => not flagged (likely json example)",
        );
    }

    // ----- Judge -----

    #[test]
    fn parse_verdict_accepts_three_words() {
        assert_eq!(
            parse_verdict("SUPERSEDES").unwrap(),
            JudgeVerdict::Supersedes
        );
        assert_eq!(
            parse_verdict("supersedes.").unwrap(),
            JudgeVerdict::Supersedes
        );
        assert_eq!(
            parse_verdict(" Contradicts! ").unwrap(),
            JudgeVerdict::Contradicts
        );
        assert_eq!(parse_verdict("coexists").unwrap(), JudgeVerdict::Coexists);
    }

    #[test]
    fn parse_verdict_rejects_unknown() {
        let err = parse_verdict("maybe").unwrap_err();
        matches!(err, JudgeError::Parse(_))
            .then_some(())
            .expect("expected Parse error");
    }

    fn extractor_with_mock(mock: Arc<MockClient>) -> LlmExtractor {
        LlmExtractor::build(
            ExtractorId::from(1),
            "judge-test".into(),
            ExtractorTarget::Statement {
                kind: StatementKindAst::Fact,
            },
            1,
            mock,
            None,
            "ignored prompt".into(),
            None,
            None,
            None,
            0.0,
            None,
            Duration::from_secs(60),
        )
    }

    fn entity_id() -> brain_core::EntityId {
        brain_core::EntityId::new()
    }

    fn fact_pair(
        subj: brain_core::EntityId,
        pred: brain_core::PredicateId,
    ) -> (Statement, Statement) {
        let old = Statement::new_root(
            brain_core::StatementId::new(),
            brain_core::StatementKind::Fact,
            SubjectRef::Entity(subj),
            pred,
            StatementObject::Value(StatementValue::Text("old".into())),
            0.9,
            brain_core::knowledge::EvidenceRef::default(),
            ExtractorId::from(0),
            1_700_000_000_000_000_000,
            1,
        );
        let new = Statement::new_root(
            brain_core::StatementId::new(),
            brain_core::StatementKind::Fact,
            SubjectRef::Entity(subj),
            pred,
            StatementObject::Value(StatementValue::Text("new".into())),
            0.9,
            brain_core::knowledge::EvidenceRef::default(),
            ExtractorId::from(0),
            1_700_000_000_000_000_001,
            1,
        );
        (old, new)
    }

    fn open_md(tmp: &tempfile::TempDir) -> brain_metadata::MetadataDb {
        let mut db = brain_metadata::MetadataDb::open(tmp.path().join("md.redb")).unwrap();
        // Touch the tables the judge's renderer reads so a read txn
        // on a fresh DB doesn't error with "Table does not exist".
        let wtxn = db.write_txn().unwrap();
        let _ = wtxn
            .open_table(brain_metadata::tables::entity::ENTITIES_TABLE)
            .unwrap();
        let _ = wtxn
            .open_table(brain_metadata::tables::predicate::PREDICATES_TABLE)
            .unwrap();
        wtxn.commit().unwrap();
        db
    }

    #[test]
    fn judge_supersedes_returns_supersede_verdict() {
        let mock = Arc::new(MockClient::new(
            "claude-haiku-test",
            vec![Ok(ok_response("SUPERSEDES", 50))],
        ));
        let calls = mock.calls.clone();
        let ext = extractor_with_mock(mock);

        let tmp = tempfile::tempdir().unwrap();
        let md = open_md(&tmp);
        let rtxn = md.read_txn().unwrap();

        let (old, new) = fact_pair(entity_id(), brain_core::PredicateId::from(1));
        let verdict =
            futures_lite::future::block_on(ext.judge_supersedes_call(&new, &old, &rtxn)).unwrap();
        assert_eq!(verdict, JudgeVerdict::Supersedes);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn judge_supersedes_degraded_extractor_errors() {
        let ext = LlmExtractor::degraded(
            ExtractorId::from(2),
            "judge-degraded".into(),
            ExtractorTarget::Statement {
                kind: StatementKindAst::Fact,
            },
            1,
            0.0,
            "no API key",
        );
        let tmp = tempfile::tempdir().unwrap();
        let md = open_md(&tmp);
        let rtxn = md.read_txn().unwrap();
        let (old, new) = fact_pair(entity_id(), brain_core::PredicateId::from(1));
        let err = futures_lite::future::block_on(ext.judge_supersedes_call(&new, &old, &rtxn))
            .unwrap_err();
        matches!(err, JudgeError::Transport(_))
            .then_some(())
            .expect("expected Transport error from degraded extractor");
    }

    #[test]
    fn judge_supersedes_budget_blocks_call() {
        let mock = Arc::new(MockClient::new("claude-sonnet-test", vec![]));
        let calls = mock.calls.clone();
        let ext = LlmExtractor::build(
            ExtractorId::from(3),
            "judge-budget".into(),
            ExtractorTarget::Statement {
                kind: StatementKindAst::Fact,
            },
            1,
            mock,
            None,
            "ignored".into(),
            None,
            None,
            None,
            0.0,
            // 1 micro-USD budget — sonnet at 3 µ$/1k will exceed.
            Some(CostBudget {
                per_call_micro_usd: 1,
            }),
            Duration::from_secs(60),
        );
        let tmp = tempfile::tempdir().unwrap();
        let md = open_md(&tmp);
        let rtxn = md.read_txn().unwrap();
        let (old, new) = fact_pair(entity_id(), brain_core::PredicateId::from(1));
        let err = futures_lite::future::block_on(ext.judge_supersedes_call(&new, &old, &rtxn))
            .unwrap_err();
        matches!(err, JudgeError::Budget(_))
            .then_some(())
            .expect("expected Budget error");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            0,
            "budget rejection must not reach the client"
        );
    }

    #[test]
    fn judge_supersedes_unparseable_response_errors() {
        let mock = Arc::new(MockClient::new(
            "claude-haiku-test",
            vec![Ok(ok_response("I'm not sure", 50))],
        ));
        let ext = extractor_with_mock(mock);
        let tmp = tempfile::tempdir().unwrap();
        let md = open_md(&tmp);
        let rtxn = md.read_txn().unwrap();
        let (old, new) = fact_pair(entity_id(), brain_core::PredicateId::from(1));
        let err = futures_lite::future::block_on(ext.judge_supersedes_call(&new, &old, &rtxn))
            .unwrap_err();
        matches!(err, JudgeError::Parse(_))
            .then_some(())
            .expect("expected Parse error");
    }

    // ------------------------------------------------------------------
    // W2.3 — bounded inferential context in the prompt.
    // ------------------------------------------------------------------

    fn neighbor(text: &str, score: f32, created_at_unix_nanos: u64) -> NeighborMemory {
        NeighborMemory {
            memory_id: brain_core::MemoryId::pack(0, 999, 0),
            text: text.into(),
            similarity_score: score,
            created_at_unix_nanos,
        }
    }

    #[test]
    fn extract_with_context_includes_neighbors_in_prompt() {
        let prompt = "Extract from: {TEXT}";
        let ext = ext_with_prompt(prompt);
        let inner = ext.inner.as_ref().unwrap().clone();
        let mid = brain_core::MemoryId::pack(0, 1, 0);
        let now = 10 * 86_400 * 1_000_000_000u64; // 10 days after epoch
        let day_ns = 86_400 * 1_000_000_000u64;
        let ec = ExtractorContext {
            neighbors: vec![
                neighbor(
                    "Alice and Bob discussed the new payment service architecture.",
                    0.84,
                    now - 3 * 3_600 * 1_000_000_000u64, // 3h ago
                ),
                neighbor(
                    "Alice mentioned a 3pm meeting with the platform team.",
                    0.78,
                    now - 12 * 3_600 * 1_000_000_000u64, // 12h ago
                ),
                neighbor(
                    "Bob shipped the auth-rewrite branch yesterday.",
                    0.71,
                    now - day_ns,
                ),
            ],
            summary: None,
        };
        let (req, stats) = ext.build_request(
            &inner,
            mid,
            "Alice approved the design.",
            &[],
            Some(&ec),
            now,
        );
        let body = &req.messages[0].content;
        assert!(
            body.contains("## Recent context"),
            "prompt missing recent-context heading: {body}",
        );
        assert!(body.contains("Alice and Bob discussed the new payment service architecture."));
        assert!(body.contains("Alice mentioned a 3pm meeting"));
        assert!(body.contains("Bob shipped the auth-rewrite branch"));
        assert!(body.contains("similarity=0.84"));
        assert!(body.contains("similarity=0.78"));
        assert!(body.contains("T-3h"));
        assert!(body.contains("T-12h"));
        assert!(body.contains("T-1d"));
        let context_pos = body.find("## Recent context").unwrap();
        let memory_pos = body.find("Alice approved the design.").unwrap();
        assert!(
            context_pos < memory_pos,
            "recent-context section must precede the memory text: {body}",
        );
        assert_eq!(stats.neighbors_included, 3);
        assert!(!stats.summary_included, "no summary in this fixture");
    }

    #[test]
    fn extract_with_context_drops_summary_when_over_budget() {
        let prompt = "{TEXT}";
        let ext = ext_with_prompt(prompt);
        let inner = ext.inner.as_ref().unwrap().clone();
        let mid = brain_core::MemoryId::pack(0, 1, 0);
        // Memory text sized so that adding the summary tips us over
        // the 4k-token cap but dropping it brings us back. The
        // enforcer drops summary first (per spec) — neighbors fit
        // even after the summary is gone.
        let memory_text = "m".repeat(15_400); // ~3850 tokens
        let ec = ExtractorContext {
            neighbors: vec![neighbor("a recent prior", 0.9, 1_000)],
            // 600-char summary (truncated to 500 in the render).
            summary: Some("Summary that pushes us over the cap. ".repeat(20)),
        };
        let (_req, stats) = ext.build_request(&inner, mid, &memory_text, &[], Some(&ec), 0);
        assert!(
            !stats.summary_included,
            "summary must be dropped when over budget (got {} tokens)",
            stats.approx_input_tokens,
        );
        assert!(
            stats.neighbors_included >= 1,
            "neighbor must survive after summary drops (got {} neighbors)",
            stats.neighbors_included,
        );
        assert!(
            stats.approx_input_tokens <= LLM_INPUT_TOKEN_BUDGET,
            "post-trim prompt must fit in 4k tokens: {} tokens",
            stats.approx_input_tokens,
        );
    }

    #[test]
    fn extract_with_context_drops_lowest_similarity_neighbors_when_over_budget() {
        let prompt = "{TEXT}";
        let ext = ext_with_prompt(prompt);
        let inner = ext.inner.as_ref().unwrap().clone();
        let mid = brain_core::MemoryId::pack(0, 1, 0);
        // ~2750 tokens of memory + 30 neighbors each at the 200-char
        // render cap → ~9k chars of neighbor section → ~2250 tokens.
        // Total ~5000 tokens — well past the 4k budget. Trimming
        // peels neighbors from the tail (lowest similarity) until
        // the prompt fits.
        let memory_text = "m".repeat(11_000);
        let long_body = "x".repeat(220); // exceeds 200-char cap → truncates
        let neighbors: Vec<NeighborMemory> = (0..30)
            .map(|i| {
                neighbor(
                    &format!("neighbor-{i:02}-{long_body}"),
                    1.0 - (i as f32) * 0.02,
                    1_000_000_000 * (i as u64 + 1),
                )
            })
            .collect();
        let ec = ExtractorContext {
            neighbors,
            summary: None,
        };
        let (req, stats) = ext.build_request(&inner, mid, &memory_text, &[], Some(&ec), 0);
        let body = &req.messages[0].content;
        assert!(
            stats.neighbors_included < 30,
            "some must have been dropped (kept {})",
            stats.neighbors_included,
        );
        assert!(
            stats.neighbors_included > 0,
            "highest-similarity prefix must survive",
        );
        assert!(
            body.contains("neighbor-00-"),
            "the highest-similarity neighbor must survive trimming",
        );
        assert!(
            stats.approx_input_tokens <= LLM_INPUT_TOKEN_BUDGET,
            "post-trim prompt must fit in 4k tokens: {} tokens",
            stats.approx_input_tokens,
        );
        assert!(body.contains("## Recent context"));
    }

    #[test]
    fn extract_with_context_includes_summary_when_under_budget() {
        let prompt = "{TEXT}";
        let ext = ext_with_prompt(prompt);
        let inner = ext.inner.as_ref().unwrap().clone();
        let mid = brain_core::MemoryId::pack(0, 1, 0);
        let ec = ExtractorContext {
            neighbors: vec![neighbor("a recent prior", 0.9, 1_000)],
            summary: Some("Last week Alice shipped the auth rewrite.".into()),
        };
        let (req, stats) = ext.build_request(&inner, mid, "today", &[], Some(&ec), 1_000_000_000);
        let body = &req.messages[0].content;
        assert!(body.contains("## Rolling summary"));
        assert!(body.contains("Last week Alice shipped"));
        assert!(stats.summary_included);
    }

    #[test]
    fn extract_with_context_skips_sections_when_context_is_empty() {
        let prompt = "{TEXT}";
        let ext = ext_with_prompt(prompt);
        let inner = ext.inner.as_ref().unwrap().clone();
        let mid = brain_core::MemoryId::pack(0, 1, 0);
        let ec = ExtractorContext::empty();
        let (req, stats) = ext.build_request(&inner, mid, "first memory ever", &[], Some(&ec), 0);
        let body = &req.messages[0].content;
        assert!(
            !body.contains("## Recent context"),
            "empty context must not render any section: {body}",
        );
        assert!(!body.contains("## Rolling summary"));
        assert_eq!(stats.neighbors_included, 0);
        assert!(!stats.summary_included);
    }

    #[test]
    fn relative_time_hint_renders_days_hours_minutes_seconds() {
        let now = 100 * 86_400 * 1_000_000_000u64;
        assert_eq!(
            relative_time_hint(now, now - 5 * 86_400 * 1_000_000_000),
            "T-5d"
        );
        assert_eq!(
            relative_time_hint(now, now - 3 * 3_600 * 1_000_000_000),
            "T-3h"
        );
        assert_eq!(
            relative_time_hint(now, now - 7 * 60 * 1_000_000_000),
            "T-7m"
        );
        assert_eq!(relative_time_hint(now, now - 45 * 1_000_000_000), "T-45s");
        assert_eq!(relative_time_hint(now, now), "T-0s");
        assert_eq!(relative_time_hint(0, 1), "T+0");
    }

    #[test]
    fn truncate_chars_caps_long_strings_utf8_safe() {
        assert_eq!(truncate_chars("hello", 10), "hello");
        let long = "héllo🌍".repeat(50); // 6 chars × 50 = 300 chars
        let out = truncate_chars(&long, 10);
        assert_eq!(out.chars().count(), 11); // 10 + ellipsis
        assert!(out.ends_with('…'));
    }
}
