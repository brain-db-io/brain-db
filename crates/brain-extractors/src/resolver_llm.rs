//! Resolver tier-4 implementation backed by brain-llm.
//!
//! Bridges the async [`brain_llm::LlmClient::complete`] call into the
//! sync [`ResolverLlm`] trait expected by the brain-core resolver. The
//! prompt is shaped so the model replies in a tiny machine-parseable
//! grammar (`PICK <n> <conf>` / `NONE` / `AMBIGUOUS`) — keeps the
//! per-call output token count near the floor and avoids JSON-schema
//! validation overhead for what is essentially a one-line decision.
//!
//! The caller (typically the extractor pipeline in brain-ops) is
//! responsible for fetching candidate views from storage and handing
//! them to the disambiguator at construction time; this struct stays
//! storage-agnostic.

use std::sync::Arc;

use brain_core::resolution::{ResolverError, ResolverLlm, ResolverLlmDecision};
use brain_core::EntityId;
use brain_llm::{LlmClient, LlmMessage, LlmRequest, LlmRole};

/// Read-side projection of one candidate, fetched by the caller and
/// passed to the disambiguator so it can build a useful prompt.
#[derive(Debug, Clone)]
pub struct LlmCandidateView {
    pub entity_id: EntityId,
    pub canonical_name: String,
    pub aliases: Vec<String>,
    pub entity_type_name: String,
}

/// [`ResolverLlm`] impl that asks an LLM to pick among candidates.
///
/// Construct once per resolve call with the views fetched for the
/// candidate set. The struct holds an `Arc<dyn LlmClient>` so multiple
/// disambiguators can share the same provider client (and per-shard
/// rate-limit / cache state) without re-instantiation.
pub struct BrainLlmDisambiguator {
    client: Arc<dyn LlmClient>,
    model: String,
    max_tokens: u32,
    views: Vec<LlmCandidateView>,
}

impl BrainLlmDisambiguator {
    /// Construct a new disambiguator. `views` should contain one entry
    /// per candidate the resolver might surface; the disambiguator
    /// matches on `EntityId` and silently skips ids without a view.
    pub fn new(
        client: Arc<dyn LlmClient>,
        model: impl Into<String>,
        views: Vec<LlmCandidateView>,
    ) -> Self {
        Self {
            client,
            model: model.into(),
            // 256 tokens leaves headroom for the one-line decision +
            // any provider-injected formatting; bigger budgets just
            // burn cost without changing the parse target.
            max_tokens: 256,
            views,
        }
    }

    /// Override the per-call token cap. Default is 256 — only raise
    /// if the operator's prompt template grew beyond a single line.
    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}

impl ResolverLlm for BrainLlmDisambiguator {
    fn disambiguate(
        &self,
        candidate: &str,
        context: &str,
        candidates: &[(EntityId, f32)],
    ) -> Result<ResolverLlmDecision, ResolverError> {
        // 1. Filter views to the provided candidate set, preserving the
        //    score order. Views without a matching id are dropped —
        //    we'd rather skip an unrepresented candidate than crash
        //    on a stale id.
        let pool: Vec<&LlmCandidateView> = candidates
            .iter()
            .filter_map(|(id, _)| self.views.iter().find(|v| v.entity_id == *id))
            .collect();
        if pool.len() < 2 {
            // Defensive: caller should have screened this. If we got
            // here, there's nothing to disambiguate.
            return Ok(ResolverLlmDecision::Ambiguous);
        }

        // 2. Build the prompt — system describes the task and reply
        //    grammar; user gives the candidate + context + numbered
        //    candidate list.
        let system = build_system_prompt();
        let user = build_user_prompt(candidate, context, &pool);

        let req = LlmRequest {
            model: self.model.clone(),
            system_blocks: vec![brain_llm::types::SystemBlock::cached(system)],
            messages: vec![LlmMessage {
                role: LlmRole::User,
                content: user,
            }],
            response_schema: None,
            temperature: 0.0,
            max_tokens: self.max_tokens,
            timeout: std::time::Duration::from_secs(30),
        };

        // 3. Block on the async call. The resolver itself is sync
        //    because it runs inside a redb write txn; the
        //    LlmClient::complete future is `Send + 'a`, so a single-
        //    threaded block_on is safe here.
        let resp = futures_lite::future::block_on(self.client.complete(req))
            .map_err(|e| ResolverError::Llm(e.to_string()))?;

        // 4. Parse the single-line reply. Anything we can't parse is
        //    an error — surface it so the resolver can fall back to
        //    the ambiguity check rather than silently picking nothing.
        parse_decision(&resp.content, &pool)
            .ok_or_else(|| ResolverError::Llm(format!("unparseable LLM reply: {:?}", resp.content)))
    }
}

fn build_system_prompt() -> String {
    // Cacheable: stable across calls in a session. Keep this
    // verbatim — Anthropic prompt caching keys on byte-identical
    // blocks, so any drift wipes the cache.
    "You disambiguate a candidate surface name against a fixed list of \
known entities. Reply with EXACTLY ONE of: `PICK <index> <confidence>` \
where index is 1-based and confidence is a decimal in [0.0, 1.0]; \
`NONE` if none of the candidates is the correct match; or `AMBIGUOUS` \
if the candidates are indistinguishable from the given context. Do \
not explain. Do not add any other text. Reply on a single line."
        .to_owned()
}

fn build_user_prompt(candidate: &str, context: &str, pool: &[&LlmCandidateView]) -> String {
    let mut out = String::new();
    out.push_str("Candidate text: ");
    out.push_str(candidate);
    out.push('\n');
    if !context.is_empty() {
        out.push_str("Context: ");
        out.push_str(context);
        out.push('\n');
    }
    out.push_str("\nCandidates:\n");
    for (i, v) in pool.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} (type: {}",
            i + 1,
            v.canonical_name,
            v.entity_type_name
        ));
        if !v.aliases.is_empty() {
            out.push_str("; aliases: ");
            out.push_str(&v.aliases.join(", "));
        }
        out.push_str(")\n");
    }
    out.push_str("\nReply with one of: PICK <n> <confidence>, NONE, AMBIGUOUS.\n");
    out
}

fn parse_decision(content: &str, pool: &[&LlmCandidateView]) -> Option<ResolverLlmDecision> {
    let line = content.trim().lines().next()?.trim();
    if line.eq_ignore_ascii_case("NONE") {
        return Some(ResolverLlmDecision::None);
    }
    if line.eq_ignore_ascii_case("AMBIGUOUS") {
        return Some(ResolverLlmDecision::Ambiguous);
    }
    // `PICK <index> <confidence>` (trailing tokens ignored).
    let mut parts = line.split_whitespace();
    if !parts.next()?.eq_ignore_ascii_case("PICK") {
        return None;
    }
    let idx: usize = parts.next()?.parse().ok()?;
    let conf: f32 = parts.next()?.parse().ok()?;
    if idx >= 1 && idx <= pool.len() && (0.0..=1.0).contains(&conf) {
        return Some(ResolverLlmDecision::Pick {
            entity: pool[idx - 1].entity_id,
            confidence: conf,
        });
    }
    None
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use brain_llm::client::LlmFuture;
    use brain_llm::error::LlmError;
    use brain_llm::types::LlmResponse;
    use std::sync::Mutex;

    /// Fake [`LlmClient`] that returns a canned response (or error).
    /// `LlmError` isn't Clone, so we stash the next reply in an
    /// `Option` and `take()` it per call. Tests that need multiple
    /// calls re-fill the slot.
    enum Reply {
        Ok(String),
        Err(LlmError),
    }

    struct FakeLlm {
        next: Mutex<Option<Reply>>,
        // Persisted Ok-text used to refill `next` on each call — keeps
        // the call-count test simple without expanding the surface.
        sticky_ok: Mutex<Option<String>>,
        calls: Mutex<u32>,
        model: String,
    }

    impl FakeLlm {
        fn ok(reply: impl Into<String>) -> Arc<Self> {
            let s = reply.into();
            Arc::new(Self {
                next: Mutex::new(Some(Reply::Ok(s.clone()))),
                sticky_ok: Mutex::new(Some(s)),
                calls: Mutex::new(0),
                model: "test-model".into(),
            })
        }
        fn err(e: LlmError) -> Arc<Self> {
            Arc::new(Self {
                next: Mutex::new(Some(Reply::Err(e))),
                sticky_ok: Mutex::new(None),
                calls: Mutex::new(0),
                model: "test-model".into(),
            })
        }
        fn call_count(&self) -> u32 {
            *self.calls.lock().unwrap()
        }
    }

    impl LlmClient for FakeLlm {
        fn complete<'a>(&'a self, _request: LlmRequest) -> LlmFuture<'a> {
            *self.calls.lock().unwrap() += 1;
            let mut slot = self.next.lock().unwrap();
            let reply = slot.take();
            // Refill from sticky_ok so the same Ok reply can be used
            // across multiple calls.
            if let Some(s) = self.sticky_ok.lock().unwrap().clone() {
                *slot = Some(Reply::Ok(s));
            }
            drop(slot);
            Box::pin(async move {
                match reply {
                    Some(Reply::Ok(content)) => Ok(LlmResponse {
                        content,
                        tokens_in: 0,
                        tokens_out: 0,
                        cache_creation_input_tokens: 0,
                        cache_read_input_tokens: 0,
                        cost_micro_usd: 0,
                        model_version: "test-model".to_owned(),
                    }),
                    Some(Reply::Err(e)) => Err(e),
                    None => Err(LlmError::InvalidRequest {
                        reason: "FakeLlm exhausted".into(),
                    }),
                }
            })
        }
        fn model(&self) -> &str {
            &self.model
        }
        fn model_id_hash(&self) -> u64 {
            0
        }
    }

    fn views_pair() -> (EntityId, EntityId, Vec<LlmCandidateView>) {
        let a = EntityId::new();
        let b = EntityId::new();
        let views = vec![
            LlmCandidateView {
                entity_id: a,
                canonical_name: "Priya Patel".into(),
                aliases: vec!["Priya".into()],
                entity_type_name: "Person".into(),
            },
            LlmCandidateView {
                entity_id: b,
                canonical_name: "Priya Kumar".into(),
                aliases: vec![],
                entity_type_name: "Person".into(),
            },
        ];
        (a, b, views)
    }

    #[test]
    fn parse_decision_pick_ok() {
        let (a, _b, views) = views_pair();
        let pool: Vec<&LlmCandidateView> = views.iter().collect();
        let out = parse_decision("PICK 1 0.92\n", &pool).unwrap();
        assert_eq!(
            out,
            ResolverLlmDecision::Pick {
                entity: a,
                confidence: 0.92
            }
        );
    }

    #[test]
    fn parse_decision_pick_case_insensitive_with_trailing_noise() {
        let (a, _b, views) = views_pair();
        let pool: Vec<&LlmCandidateView> = views.iter().collect();
        let out = parse_decision("  pick 1 0.5 ignored-tail\n", &pool).unwrap();
        assert_eq!(
            out,
            ResolverLlmDecision::Pick {
                entity: a,
                confidence: 0.5
            }
        );
    }

    #[test]
    fn parse_decision_none_and_ambiguous() {
        let (_a, _b, views) = views_pair();
        let pool: Vec<&LlmCandidateView> = views.iter().collect();
        assert_eq!(
            parse_decision("NONE", &pool).unwrap(),
            ResolverLlmDecision::None
        );
        assert_eq!(
            parse_decision("none\n", &pool).unwrap(),
            ResolverLlmDecision::None
        );
        assert_eq!(
            parse_decision("AMBIGUOUS", &pool).unwrap(),
            ResolverLlmDecision::Ambiguous
        );
        assert_eq!(
            parse_decision("ambiguous", &pool).unwrap(),
            ResolverLlmDecision::Ambiguous
        );
    }

    #[test]
    fn parse_decision_rejects_out_of_range_index() {
        let (_a, _b, views) = views_pair();
        let pool: Vec<&LlmCandidateView> = views.iter().collect();
        assert!(parse_decision("PICK 0 0.5", &pool).is_none());
        assert!(parse_decision("PICK 3 0.5", &pool).is_none());
    }

    #[test]
    fn parse_decision_rejects_out_of_range_confidence() {
        let (_a, _b, views) = views_pair();
        let pool: Vec<&LlmCandidateView> = views.iter().collect();
        assert!(parse_decision("PICK 1 -0.1", &pool).is_none());
        assert!(parse_decision("PICK 1 1.5", &pool).is_none());
    }

    #[test]
    fn parse_decision_rejects_garbage() {
        let (_a, _b, views) = views_pair();
        let pool: Vec<&LlmCandidateView> = views.iter().collect();
        assert!(parse_decision("", &pool).is_none());
        assert!(parse_decision("HELLO", &pool).is_none());
        assert!(parse_decision("PICK", &pool).is_none());
        assert!(parse_decision("PICK one 0.5", &pool).is_none());
        assert!(parse_decision("PICK 1", &pool).is_none());
    }

    #[test]
    fn build_user_prompt_renders_candidate_and_context() {
        let (_a, _b, views) = views_pair();
        let pool: Vec<&LlmCandidateView> = views.iter().collect();
        let out = build_user_prompt("Priya", "with Bob at lunch", &pool);
        assert!(out.contains("Candidate text: Priya"));
        assert!(out.contains("Context: with Bob at lunch"));
        assert!(out.contains("1. Priya Patel (type: Person; aliases: Priya)"));
        assert!(out.contains("2. Priya Kumar (type: Person)"));
        assert!(out.contains("PICK"));
    }

    #[test]
    fn build_user_prompt_omits_empty_context() {
        let (_a, _b, views) = views_pair();
        let pool: Vec<&LlmCandidateView> = views.iter().collect();
        let out = build_user_prompt("Priya", "", &pool);
        assert!(!out.contains("Context:"));
        assert!(out.contains("Candidate text: Priya"));
    }

    #[test]
    fn disambiguate_roundtrip_pick() {
        let (a, b, views) = views_pair();
        let client = FakeLlm::ok("PICK 2 0.81");
        let dis = BrainLlmDisambiguator::new(client.clone(), "test-model", views);
        let out = dis
            .disambiguate("Priya", "ctx", &[(a, 0.90), (b, 0.89)])
            .unwrap();
        assert_eq!(
            out,
            ResolverLlmDecision::Pick {
                entity: b,
                confidence: 0.81
            }
        );
        assert_eq!(client.call_count(), 1);
    }

    #[test]
    fn disambiguate_short_pool_returns_ambiguous_without_call() {
        let (a, _b, views) = views_pair();
        let client = FakeLlm::ok("PICK 1 0.99");
        let dis = BrainLlmDisambiguator::new(client.clone(), "test-model", views);
        // Only one candidate id provided -> short-circuit before
        // hitting the LLM.
        let out = dis.disambiguate("Priya", "ctx", &[(a, 0.90)]).unwrap();
        assert_eq!(out, ResolverLlmDecision::Ambiguous);
        assert_eq!(client.call_count(), 0);
    }

    #[test]
    fn disambiguate_unparseable_reply_is_llm_error() {
        let (a, b, views) = views_pair();
        let client = FakeLlm::ok("I think it's the first one, probably.");
        let dis = BrainLlmDisambiguator::new(client, "test-model", views);
        let err = dis
            .disambiguate("Priya", "ctx", &[(a, 0.90), (b, 0.89)])
            .unwrap_err();
        assert!(matches!(err, ResolverError::Llm(_)));
    }

    #[test]
    fn disambiguate_propagates_transport_error() {
        let (a, b, views) = views_pair();
        let client = FakeLlm::err(LlmError::Timeout);
        let dis = BrainLlmDisambiguator::new(client, "test-model", views);
        let err = dis
            .disambiguate("Priya", "ctx", &[(a, 0.90), (b, 0.89)])
            .unwrap_err();
        assert!(matches!(err, ResolverError::Llm(_)));
    }
}
