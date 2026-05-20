//! `recall` verb.
//!
//! With `--include-graph`, per-hit knowledge enrichment (linked
//! entities + top statements + relations) is requested from the
//! server. The current wire RecallResp doesn't carry that side-channel;
//! the shell-side renderer falls back to empty enrichment sections so
//! the surface compiles today and the renderer comes alive as soon as
//! the wire response grows the fields.

use brain_core::MemoryId;
use brain_sdk_rust::{Client, ClientError};

use brain_explore::{GraphEnrichment, RecallResults, RecallWithGraph};

use crate::parser::{parse_txn_id, RecallArgs};
use crate::session::Session;

use super::Rendered;

/// Send a `RECALL`, collecting all frames into a `Vec<MemoryResult>`.
/// Pushes every returned id onto the session's recent-id list.
pub async fn run(
    client: &Client,
    session: &mut Session,
    args: RecallArgs,
) -> Result<Rendered, ClientError> {
    let explicit_txn = match args.txn.as_deref() {
        Some(s) => Some(parse_txn_id(s).map_err(ClientError::Internal)?),
        None => None,
    };
    let txn = session.effective_txn(explicit_txn);

    let mut b = client
        .recall(args.query)
        .top_k(args.top_k)
        .confidence_threshold(args.confidence)
        .salience_floor(args.salience_floor)
        .include_text(args.include_text);
    if !args.filter_context.is_empty() {
        b = b.context_filter(args.filter_context);
    }
    if !args.filter_kind.is_empty() {
        let kinds = args
            .filter_kind
            .into_iter()
            .map(|k| k.into_wire())
            .collect();
        b = b.kind_filter(kinds);
    }
    if let Some(secs) = args.max_age_seconds {
        // The wire field is an absolute lower-bound timestamp ("keep
        // memories created at or after this point"). Compute it from
        // the client's clock; nanos because that's the wire unit.
        let now_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(u64::MAX);
        let cutoff = now_nanos.saturating_sub(secs.saturating_mul(1_000_000_000));
        b = b.age_bound_unix_nanos(Some(cutoff));
    }
    if let Some(t) = txn {
        b = b.txn(t);
    }
    let results = b.send().await?;
    for r in &results {
        session.push_recent_id(MemoryId::from_raw(r.memory_id));
    }

    if args.include_graph {
        tracing::warn!(
            target: "brain_shell",
            "recall --include-graph: per-hit enrichment depends on a wire \
             RecallResp field (entities / statements / relations) that is not \
             populated today. Rendering empty enrichment sections — the \
             renderer comes online once the wire grows the fields.",
        );
        let graphs = results.iter().map(|_| GraphEnrichment::default()).collect();
        return Ok(Box::new(RecallWithGraph {
            hits: results,
            graphs,
        }));
    }

    Ok(Box::new(RecallResults(results)))
}
