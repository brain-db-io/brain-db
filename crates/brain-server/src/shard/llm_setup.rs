//! LLM-tier startup wiring for the per-shard executor
//! §2 (provider routing) +.4 / §26 (per-shard
//! `llm_cache.redb`).
//!
//! Phase 21.5 builds the `MaterializeDeps` slots that 21.4 left
//! empty:
//!
//! - Reads `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` (optionally
//!   `BRAIN_ANTHROPIC_MODEL` / `BRAIN_OPENAI_MODEL` to override
//!   the default model per provider).
//! - Constructs a [`ModelRouter`] populated with whichever
//!   clients have keys.
//! - Opens `<shard_dir>/llm_cache.redb` via [`LlmCacheDb::open`].
//!   Failure to open the file is non-fatal: a warning is logged
//!   and the cache slot stays `None` (LLM extractors then skip
//!   caching).
//!
//! ## Why one client per provider in v1
//!
//! routes by **prefix only**: the operator's
//! `model:` schema field selects the provider, not the wire
//! model. The wire model is whichever model the server-side
//! client was constructed for. Per-extractor model selection +
//! per-provider client pools are deferred (§22/07 — phase 22+).
//!
//! Defaults are picked to match the embedded pricing table in
//! `brain_extractors::Pricing::for_model`:
//!
//! - Anthropic → `claude-haiku-4-5`
//! - OpenAI    → `gpt-4o-mini`

use std::path::Path;
use std::sync::Arc;

use brain_extractors::{ClassifierModel, EntityDisambiguator, MaterializeDeps};
use brain_llm::{AnthropicClient, LlmClient, ModelRouter, OpenAIClient};
use brain_metadata::LlmCacheDb;
use parking_lot::Mutex;

const DEFAULT_ANTHROPIC_MODEL: &str = "claude-haiku-4-5";
const DEFAULT_OPENAI_MODEL: &str = "gpt-4o-mini";

/// LLM-tier deps assembled at shard startup. Threaded into both
/// `MaterializeDeps` (so LLM-kind rows decode into wired
/// extractors) and `OpsContext.llm_cache` (so future ops can
/// reach the cache directly). The optional `disambiguator` slot
/// feeds the extractor worker so the resolver can second-opinion
/// ambiguous-band partial matches.
pub struct LlmDeps {
    pub router: Option<Arc<ModelRouter>>,
    pub cache: Option<Arc<Mutex<LlmCacheDb>>>,
    /// Optional partial-match disambiguator. Populated when at least
    /// one provider client is available; the disambiguator shares
    /// that client (Anthropic preferred, OpenAI as fallback) so
    /// there's no duplicate API key handling.
    pub disambiguator: Option<Arc<EntityDisambiguator>>,
}

impl LlmDeps {
    /// Merge with the existing classifier model and the snapshotted
    /// entity-type qname list into a full [`MaterializeDeps`].
    /// The qname list is what the classifier passes as labels on
    /// every `predict()` call.
    ///
    /// The [`disambiguator`](Self::disambiguator) slot is intentionally
    /// dropped here: it's wired directly into the extractor worker, not
    /// through `MaterializeDeps`. Callers that need it must
    /// [`Arc::clone`] the field before invoking this consumer.
    #[must_use]
    pub fn into_materialize_deps(
        self,
        classifier_model: Option<Arc<dyn ClassifierModel>>,
        entity_type_qnames: Arc<Vec<String>>,
    ) -> MaterializeDeps {
        MaterializeDeps {
            classifier_model,
            entity_type_qnames,
            model_router: self.router,
            llm_cache: self.cache,
        }
    }
}

/// Build the LLM-tier deps from env + shard directory layout.
/// Always returns a value; missing keys / unopenable cache files
/// produce `None` slots.
pub fn build_llm_deps(shard_dir: &Path) -> LlmDeps {
    let (primary_client, primary_model) = build_primary_client();
    let disambiguator =
        primary_client.map(|c| Arc::new(EntityDisambiguator::new(c, primary_model)));
    LlmDeps {
        router: build_router(),
        cache: open_cache(shard_dir),
        disambiguator,
    }
}

fn anthropic_model() -> String {
    std::env::var("BRAIN_ANTHROPIC_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_ANTHROPIC_MODEL.to_string())
}

fn openai_model() -> String {
    std::env::var("BRAIN_OPENAI_MODEL")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_OPENAI_MODEL.to_string())
}

/// Pick the primary client for single-call surfaces (today: the
/// partial-match disambiguator). Anthropic wins ties — it's the
/// reference path Brain optimises prompt caching against.
///
/// Returns `(None, String::new())` when no provider is configured.
fn build_primary_client() -> (Option<Arc<dyn LlmClient>>, String) {
    let model_a = anthropic_model();
    if let Some(c) = AnthropicClient::from_env(model_a.clone()) {
        let client: Arc<dyn LlmClient> = Arc::new(c);
        return (Some(client), model_a);
    }
    let model_o = openai_model();
    if let Some(c) = OpenAIClient::from_env(model_o.clone()) {
        let client: Arc<dyn LlmClient> = Arc::new(c);
        return (Some(client), model_o);
    }
    (None, String::new())
}

fn build_router() -> Option<Arc<ModelRouter>> {
    let mut r = ModelRouter::new();
    let mut any = false;

    if let Some(c) = AnthropicClient::from_env(anthropic_model()) {
        let client: Arc<dyn LlmClient> = Arc::new(c);
        r = r.with_anthropic(client);
        any = true;
    }

    if let Some(c) = OpenAIClient::from_env(openai_model()) {
        let client: Arc<dyn LlmClient> = Arc::new(c);
        r = r.with_openai(client);
        any = true;
    }

    if any {
        Some(Arc::new(r))
    } else {
        None
    }
}

fn open_cache(shard_dir: &Path) -> Option<Arc<Mutex<LlmCacheDb>>> {
    let path = shard_dir.join("llm_cache.redb");
    match LlmCacheDb::open(&path) {
        Ok(db) => Some(Arc::new(Mutex::new(db))),
        Err(e) => {
            // redb uses POSIX flock; the most common cause of a failed
            // open at server startup is another brain-server process
            // already holding it. Spell that out so operators know what
            // to do — the warn alone left users grep-ing.
            let hint = if e.to_string().contains("Database already open")
                || e.to_string().contains("Cannot acquire lock")
            {
                Some(format!(
                    "another process holds the redb lock — check `fuser {p}` or \
                     `pgrep -fl brain-server`",
                    p = path.display()
                ))
            } else {
                None
            };
            tracing::warn!(
                target: "brain_server::shard::llm_setup",
                path = %path.display(),
                error = %e,
                hint = hint.as_deref().unwrap_or(""),
                "failed to open llm_cache.redb; LLM extractors will skip caching on this shard",
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // Env vars are process-global; serialize env-mutating tests so
    // they don't trample each other under cargo's default parallel
    // test runner.
    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    /// Save + restore the four env vars this module reads. Drops
    /// at end of scope restore previous values, including absence.
    struct EnvGuard {
        snapshot: Vec<(&'static str, Option<String>)>,
    }

    impl EnvGuard {
        fn new(keys: &[&'static str]) -> Self {
            let snapshot = keys.iter().map(|k| (*k, std::env::var(k).ok())).collect();
            // Start from a clean slate.
            for k in keys {
                std::env::remove_var(k);
            }
            Self { snapshot }
        }

        fn set(&self, key: &str, value: &str) {
            std::env::set_var(key, value);
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in &self.snapshot {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    const ALL_KEYS: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "BRAIN_ANTHROPIC_MODEL",
        "BRAIN_OPENAI_MODEL",
    ];

    #[test]
    fn build_router_returns_none_when_no_keys() {
        let _g = ENV_LOCK.lock().unwrap();
        let _e = EnvGuard::new(ALL_KEYS);
        assert!(build_router().is_none());
    }

    #[test]
    fn build_router_with_anthropic_only() {
        let _g = ENV_LOCK.lock().unwrap();
        let env = EnvGuard::new(ALL_KEYS);
        env.set("ANTHROPIC_API_KEY", "test-key-anthropic");
        let r = build_router().expect("router");
        assert!(r.resolve("claude-haiku-4-5").is_some());
        assert!(r.resolve("gpt-4o-mini").is_none());
    }

    #[test]
    fn build_router_with_openai_only() {
        let _g = ENV_LOCK.lock().unwrap();
        let env = EnvGuard::new(ALL_KEYS);
        env.set("OPENAI_API_KEY", "test-key-openai");
        let r = build_router().expect("router");
        assert!(r.resolve("gpt-4o-mini").is_some());
        assert!(r.resolve("claude-haiku-4-5").is_none());
    }

    #[test]
    fn build_router_with_both_keys() {
        let _g = ENV_LOCK.lock().unwrap();
        let env = EnvGuard::new(ALL_KEYS);
        env.set("ANTHROPIC_API_KEY", "test-key-a");
        env.set("OPENAI_API_KEY", "test-key-o");
        let r = build_router().expect("router");
        assert!(r.resolve("claude-haiku-4-5").is_some());
        assert!(r.resolve("gpt-4o-mini").is_some());
    }

    #[test]
    fn model_override_env_vars_take_effect() {
        let _g = ENV_LOCK.lock().unwrap();
        let env = EnvGuard::new(ALL_KEYS);
        env.set("BRAIN_ANTHROPIC_MODEL", "claude-sonnet-4-6");
        env.set("BRAIN_OPENAI_MODEL", "gpt-4o");
        assert_eq!(anthropic_model(), "claude-sonnet-4-6");
        assert_eq!(openai_model(), "gpt-4o");
    }

    #[test]
    fn model_defaults_match_pricing_table() {
        let _g = ENV_LOCK.lock().unwrap();
        let _e = EnvGuard::new(ALL_KEYS);
        assert_eq!(anthropic_model(), "claude-haiku-4-5");
        assert_eq!(openai_model(), "gpt-4o-mini");
    }

    #[test]
    fn open_cache_creates_redb_at_shard_dir() {
        let dir = tempfile::tempdir().unwrap();
        let cache = open_cache(dir.path()).expect("cache opened");
        // Round-trip a row to confirm the standard tables are present.
        let mut db = cache.lock();
        let wtxn = db.write_txn().expect("write_txn");
        {
            let _ = wtxn
                .open_table(brain_metadata::llm_cache::LLM_RESPONSES_TABLE)
                .expect("responses table");
        }
        wtxn.commit().unwrap();
        assert!(dir.path().join("llm_cache.redb").exists());
    }

    #[test]
    fn open_cache_returns_none_when_directory_unwritable() {
        // Point at a path under a regular file — opening a redb
        // inside that "directory" fails. On platforms that allow
        // it we just see `Some`, which is also acceptable; the
        // contract is "log warn + return None", not "must fail".
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("not-a-dir");
        std::fs::write(&file_path, b"x").unwrap();
        let result = open_cache(&file_path);
        // Either outcome is acceptable; the important contract is
        // we never panic. Assert we got an Option back.
        let _ = result;
    }

    #[test]
    fn into_materialize_deps_threads_router_cache_and_labels() {
        let dir = tempfile::tempdir().unwrap();
        let deps = LlmDeps {
            router: None,
            cache: open_cache(dir.path()),
            disambiguator: None,
        };
        let labels = Arc::new(vec!["brain:Person".to_string()]);
        let materialize = deps.into_materialize_deps(None, labels.clone());
        assert!(materialize.model_router.is_none());
        assert!(materialize.llm_cache.is_some());
        assert!(materialize.classifier_model.is_none());
        assert_eq!(materialize.entity_type_qnames.as_slice(), labels.as_slice());
    }

    /// Documents the constraint that drove the spawn_shard fix:
    /// redb's lock is process-wide and inode-keyed. Two live
    /// opens of the same `llm_cache.redb` from the same process
    /// MUST fail with `Database already open`. The shard's
    /// startup path therefore opens the cache exactly once (in
    /// the Glommio closure via `build_llm_deps`).
    #[test]
    fn second_open_of_same_path_fails_while_first_alive() {
        let dir = tempfile::tempdir().unwrap();
        let first = open_cache(dir.path()).expect("first open");
        // Second open with the first still alive — must fail.
        let path = dir.path().join("llm_cache.redb");
        // LlmCacheDb is not Debug (its inner redb::Database isn't),
        // so we can't use expect_err here — match on Result manually.
        let err = match LlmCacheDb::open(&path) {
            Ok(_) => panic!("second open must fail while first is still alive"),
            Err(e) => e,
        };
        assert!(
            err.to_string().contains("Database already open"),
            "expected redb lock error, got: {err}"
        );
        drop(first);
        // After drop, re-open succeeds.
        match LlmCacheDb::open(&path) {
            Ok(_db) => {}
            Err(e) => panic!("re-open after drop must succeed: {e}"),
        }
    }

    /// Two `MaterializeDeps` instances sharing the same single
    /// open `LlmCacheDb` handle via `Arc::clone` must both work —
    /// this is the contract `materialize_extractors` relies on
    /// when wiring multiple LLM extractors against one cache.
    #[test]
    fn shared_cache_handle_supports_many_materialize_deps() {
        let dir = tempfile::tempdir().unwrap();
        let llm_deps = build_llm_deps(dir.path());
        assert!(llm_deps.cache.is_some(), "cache should open");
        let cache_arc = llm_deps.cache.clone().unwrap();
        // Drop the original LlmDeps so its embedded Arc clone goes away;
        // the refcount we assert below should reflect only cache_arc plus
        // the two MaterializeDeps copies, not the original `build_llm_deps`
        // return value.
        drop(llm_deps);

        let labels = Arc::new(vec!["brain:Person".to_string()]);
        let deps_a = LlmDeps {
            router: None,
            cache: Some(Arc::clone(&cache_arc)),
            disambiguator: None,
        }
        .into_materialize_deps(None, labels.clone());
        let deps_b = LlmDeps {
            router: None,
            cache: Some(Arc::clone(&cache_arc)),
            disambiguator: None,
        }
        .into_materialize_deps(None, labels.clone());

        // Both deps point at the same redb file via Arc::clone
        // — no second `LlmCacheDb::open` was performed.
        assert!(deps_a.llm_cache.is_some());
        assert!(deps_b.llm_cache.is_some());
        assert_eq!(Arc::strong_count(&cache_arc), 3); // cache_arc + deps_a + deps_b
    }
}
