//! Write-time HyPE generation: hypothetical-question embeddings.
//!
//! For each freshly-encoded memory the generator asks an LLM for several
//! diverse questions whose answer is the memory, embeds each locally, and
//! both persists the vectors (`hype_question_vectors` table) and inserts
//! them into the live per-shard [`HypeHnswIndex`]. At read time the user's
//! query vector probes that pool — a hit on a stored question maps back to
//! the owning memory, bridging the query↔memory phrasing gap that the
//! direct passage embedding misses.
//!
//! Cost lands entirely here, at write time: the generation LLM call is
//! cached (keyed on the memory text) so a re-ingest is free, and it
//! respects the extractor worker's per-cycle LLM budget. The embedding is
//! the same local BGE model the rest of the write path uses.

use std::sync::Arc;
use std::time::Duration;

use brain_core::MemoryId;
use brain_embed::Dispatcher;
use brain_index::HypeHnswIndex;
use brain_llm::types::SystemBlock;
use brain_llm::{LlmClient, LlmMessage, LlmRequest, LlmRole};
use brain_metadata::llm_cache::{LlmResponse as CachedResponse, LLM_RESPONSES_TABLE};
use brain_metadata::{hype_vector_put, LlmCacheDb, MetadataDb};
use parking_lot::{Mutex, RwLock};

/// Cache namespace for HyPE generations — ASCII "HYPE". Distinct from
/// any extraction tier's id so the two never collide in the shared
/// `llm_cache.redb`.
const HYPE_CACHE_EXTRACTOR_ID: u32 = 0x4859_5045;
/// Bump to invalidate cached generations after a prompt change.
const HYPE_CACHE_VERSION: u32 = 1;
/// Cached generation lifetime — long, since the memory text is immutable.
const HYPE_CACHE_TTL: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Hard cap on questions stored per memory. The 17-byte storage key's
/// index byte allows 256; we generate far fewer but clamp defensively so
/// a runaway LLM reply can't overflow the `u8` index.
const MAX_QUESTIONS_PER_MEMORY: usize = 16;

/// Write-time HyPE generator. One per shard; cheap to hold (`Arc`
/// handles). Constructed only when the LLM tier is provisioned and HyPE
/// is enabled — otherwise the extractor worker leaves its slot `None` and
/// skips generation entirely.
#[derive(Clone)]
pub struct HypeGenerator {
    client: Arc<dyn LlmClient>,
    model: String,
    embedder: Arc<dyn Dispatcher>,
    index: Arc<RwLock<HypeHnswIndex>>,
    metadata: Arc<MetadataDb>,
    cache: Arc<Mutex<LlmCacheDb>>,
    /// How many questions to ask for per memory (clamped to
    /// [`MAX_QUESTIONS_PER_MEMORY`]).
    num_questions: usize,
}

/// Outcome of one memory's generation, folded into the worker's
/// per-cycle bookkeeping.
#[derive(Debug, Default, Clone, Copy)]
pub struct HypeGenOutcome {
    /// Questions embedded + persisted + inserted.
    pub questions_written: usize,
    /// LLM micro-USD spent (0 on a cache hit).
    pub cost_micro_usd: u64,
}

impl HypeGenerator {
    pub fn new(
        client: Arc<dyn LlmClient>,
        model: String,
        embedder: Arc<dyn Dispatcher>,
        index: Arc<RwLock<HypeHnswIndex>>,
        metadata: Arc<MetadataDb>,
        cache: Arc<Mutex<LlmCacheDb>>,
        num_questions: usize,
    ) -> Self {
        Self {
            client,
            model,
            embedder,
            index,
            metadata,
            cache,
            num_questions: num_questions.clamp(1, MAX_QUESTIONS_PER_MEMORY),
        }
    }

    /// Generate, embed, persist, and index hypothetical questions for one
    /// memory. Best-effort: any failure (LLM transport, empty reply,
    /// embed error, redb error) is logged and yields a zero outcome
    /// rather than failing the surrounding extraction cycle — HyPE is a
    /// recall enhancement, never a correctness dependency.
    pub async fn generate_for(&self, memory_id: MemoryId, text: &str) -> HypeGenOutcome {
        if text.trim().is_empty() {
            return HypeGenOutcome::default();
        }

        let (questions, cost_micro) = match self.questions_for(text).await {
            Ok(qs) => qs,
            Err(e) => {
                tracing::warn!(
                    target: "brain_workers::hype",
                    memory_id = ?memory_id,
                    error = %e,
                    "HyPE question generation failed; skipping",
                );
                return HypeGenOutcome::default();
            }
        };
        if questions.is_empty() {
            return HypeGenOutcome {
                questions_written: 0,
                cost_micro_usd: cost_micro,
            };
        }

        // Embed each question with the query transform — at read time the
        // user query is embedded the same way, so question↔query cosine
        // is in-distribution.
        let mut vectors: Vec<[f32; 384]> = Vec::with_capacity(questions.len());
        for q in &questions {
            match self.embedder.embed_query(q) {
                Ok(v) => vectors.push(v),
                Err(e) => {
                    tracing::warn!(
                        target: "brain_workers::hype",
                        memory_id = ?memory_id,
                        error = %e,
                        "HyPE question embed failed; skipping this question",
                    );
                }
            }
        }
        if vectors.is_empty() {
            return HypeGenOutcome {
                questions_written: 0,
                cost_micro_usd: cost_micro,
            };
        }

        // Persist first (durable source of truth), then publish to the
        // live index. A crash between the two is harmless: boot rebuilds
        // the index from the persisted rows.
        match self.persist(memory_id, &vectors) {
            Ok(()) => {}
            Err(e) => {
                tracing::warn!(
                    target: "brain_workers::hype",
                    memory_id = ?memory_id,
                    error = %e,
                    "HyPE vector persist failed; not indexing",
                );
                return HypeGenOutcome {
                    questions_written: 0,
                    cost_micro_usd: cost_micro,
                };
            }
        }
        {
            let mut idx = self.index.write();
            for v in &vectors {
                idx.insert(memory_id, v);
            }
        }

        HypeGenOutcome {
            questions_written: vectors.len(),
            cost_micro_usd: cost_micro,
        }
    }

    /// Persist all question vectors for a memory in one write transaction.
    fn persist(&self, memory_id: MemoryId, vectors: &[[f32; 384]]) -> Result<(), String> {
        let wtxn = self
            .metadata
            .write_txn()
            .map_err(|e| format!("hype write_txn: {e}"))?;
        for (i, v) in vectors.iter().enumerate() {
            let idx = u8::try_from(i).map_err(|_| "hype question index overflow".to_string())?;
            hype_vector_put(&wtxn, memory_id, idx, v).map_err(|e| format!("hype_vector_put: {e}"))?;
        }
        wtxn.commit().map_err(|e| format!("hype commit: {e}"))?;
        Ok(())
    }

    /// Return the generated questions and the LLM cost (0 on a cache
    /// hit). Reads the shared LLM cache first; on a miss it calls the
    /// model and writes the reply back.
    async fn questions_for(&self, text: &str) -> Result<(Vec<String>, u64), String> {
        let input_hash: [u8; 32] = *blake3::hash(text.as_bytes()).as_bytes();
        let model_id_hash = self.client.model_id_hash();

        if let Some(blob) = self.cache_get(input_hash, model_id_hash) {
            let raw = String::from_utf8_lossy(&blob);
            return Ok((parse_questions(&raw, self.num_questions), 0));
        }

        let req = LlmRequest {
            model: self.model.clone(),
            system_blocks: vec![SystemBlock::cached(Self::system_prompt())],
            messages: vec![LlmMessage {
                role: LlmRole::User,
                content: Self::user_prompt(text, self.num_questions),
            }],
            response_schema: None,
            temperature: 0.3,
            max_tokens: 256,
            timeout: Duration::from_secs(30),
        };
        let resp = self
            .client
            .complete(req)
            .await
            .map_err(|e| format!("hype llm transport: {e}"))?;

        let questions = parse_questions(&resp.content, self.num_questions);
        if !questions.is_empty() {
            // Cache the raw reply so a re-ingest re-parses the same text
            // for free. A parse-empty reply is not cached (it may be a
            // transient model hiccup worth retrying next time).
            self.cache_put(
                input_hash,
                model_id_hash,
                resp.content.clone().into_bytes(),
                resp.tokens_out as u32,
            );
        }
        Ok((questions, resp.cost_micro_usd))
    }

    fn system_prompt() -> String {
        // Stable wording: Anthropic prompt caching keys on byte-identical
        // blocks, so any drift wipes the cache.
        "You generate retrieval questions for a memory database. Given a \
short piece of text, output diverse questions that this text answers — \
the questions a user might later ask to find this exact fact. Vary the \
phrasing and angle (who/what/when/where/why/how). Output ONE question \
per line, nothing else: no numbering, no preamble, no blank lines. Each \
line must end with a question mark."
            .to_owned()
    }

    fn user_prompt(text: &str, n: usize) -> String {
        format!("Write {n} diverse questions that the following text answers.\n\nText:\n{text}")
    }

    fn cache_get(&self, input_hash: [u8; 32], model_id_hash: u64) -> Option<Vec<u8>> {
        let db = self.cache.lock();
        let rtxn = db.read_txn().ok()?;
        let t = rtxn.open_table(LLM_RESPONSES_TABLE).ok()?;
        let key = (
            input_hash,
            HYPE_CACHE_EXTRACTOR_ID,
            HYPE_CACHE_VERSION,
            model_id_hash,
        );
        let row = t.get(&key).ok().flatten()?;
        Some(row.value().response_blob)
    }

    fn cache_put(&self, input_hash: [u8; 32], model_id_hash: u64, blob: Vec<u8>, tokens: u32) {
        let now_nanos = now_unix_nanos();
        let expires = now_nanos.saturating_add(HYPE_CACHE_TTL.as_nanos() as u64);
        let mut db = self.cache.lock();
        let Ok(wtxn) = db.write_txn() else {
            return;
        };
        let key = (
            input_hash,
            HYPE_CACHE_EXTRACTOR_ID,
            HYPE_CACHE_VERSION,
            model_id_hash,
        );
        let value = CachedResponse::new(blob, now_nanos, expires, tokens, model_id_hash);
        {
            let Ok(mut t) = wtxn.open_table(LLM_RESPONSES_TABLE) else {
                return;
            };
            if t.insert(&key, &value).is_err() {
                return;
            }
        }
        let _ = wtxn.commit();
    }
}

/// Parse a newline-delimited question list, stripping common list noise
/// (numbering, bullets, surrounding quotes), dropping non-questions and
/// blanks, deduping, and clamping to `num_questions`. Newline-delimited
/// output is far more robust to parse than JSON from a small model.
fn parse_questions(raw: &str, num_questions: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for line in raw.lines() {
        let mut s = line.trim();
        // Strip leading "1.", "1)", "-", "*", "•" list markers.
        s = s.trim_start_matches(|c: char| {
            c.is_ascii_digit() || matches!(c, '.' | ')' | '-' | '*' | '•' | ' ' | '\t')
        });
        let s = s.trim().trim_matches('"').trim();
        if s.len() < 8 || !s.contains('?') {
            continue;
        }
        let key = s.to_lowercase();
        if seen.insert(key) {
            out.push(s.to_string());
        }
        if out.len() >= num_questions {
            break;
        }
    }
    out
}

fn now_unix_nanos() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::parse_questions;

    #[test]
    fn parse_strips_numbering_and_bullets() {
        let raw =
            "1. Where did Caroline move from?\n- What country is Caroline from?\n* Who is Caroline?";
        let qs = parse_questions(raw, 6);
        assert_eq!(qs.len(), 3);
        assert_eq!(qs[0], "Where did Caroline move from?");
        assert_eq!(qs[1], "What country is Caroline from?");
        assert_eq!(qs[2], "Who is Caroline?");
    }

    #[test]
    fn parse_drops_non_questions_and_blanks() {
        let raw = "Here are some questions:\n\nWhat is the capital?\nshort\nThis is a statement.";
        let qs = parse_questions(raw, 6);
        assert_eq!(qs, vec!["What is the capital?".to_string()]);
    }

    #[test]
    fn parse_dedups_and_caps() {
        let raw = "What is X?\nWhat is X?\nWho is Y?\nWhen is Z?";
        let qs = parse_questions(raw, 2);
        assert_eq!(qs.len(), 2, "capped at num_questions");
        assert_eq!(qs[0], "What is X?");
        assert_eq!(qs[1], "Who is Y?", "duplicate skipped, next distinct kept");
    }
}
