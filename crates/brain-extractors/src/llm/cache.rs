//! Idempotency cache helpers for the LLM extractor tier.
//!
//! Wraps the substrate's [`LlmCacheDb`] with the `(input_hash,
//! extractor_id, extractor_version, model_id_hash)` keying so two
//! extractor versions can co-exist on the same memory and the
//! sweeper can drop entries by extractor when a schema is migrated.

use std::sync::Arc;
use std::time::Duration;

use brain_metadata::llm_cache::{LlmResponse as CachedResponse, LLM_RESPONSES_TABLE};
use brain_metadata::LlmCacheDb;
use parking_lot::Mutex;

pub(super) fn cache_get(
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
pub(super) fn cache_put(
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

