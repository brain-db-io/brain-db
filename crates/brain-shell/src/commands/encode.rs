//! `encode` verb.
//!
//! Source modes (in precedence order — clap enforces mutual
//! exclusivity at parse time):
//!   * `--from-file <path>` (path `-` = stdin; `.jsonl` opens a TXN
//!     and batches each line).
//!   * `--from-stdin` (shortcut for `--from-file -`).
//!   * positional `TEXT`.
//!
//! `--wait-for-extraction` opens a subscribe stream right after the
//! encode returns and blocks until the extractor stage of the write
//! emits a `StageCompleted` event for the new memory id (or the
//! global `--timeout` elapses).

use std::io::Read;
use std::time::Duration;

use brain_core::{MemoryId, RequestId};
use brain_protocol::responses::types::EventType;
use brain_sdk_rust::{Client, ClientError};
use futures_lite::StreamExt;
use uuid::Uuid;

use brain_explore::{AutoEdgeSummary, AutoEdgesDelta, EncodeRendered};

use crate::parser::{parse_txn_id, EncodeArgs};
use crate::session::Session;

use super::Rendered;

/// Send an `ENCODE`. Inherits the session's active txn + sticky
/// context when the caller didn't override them. Pushes the
/// resulting memory id onto the recent-id list.
pub async fn run(
    client: &Client,
    session: &mut Session,
    args: EncodeArgs,
) -> Result<Rendered, ClientError> {
    let text = resolve_source_text(&args)?;
    let request_id = parse_request_id(args.request_id.as_deref())?;

    let explicit_txn = match args.txn.as_deref() {
        Some(s) => Some(parse_txn_id(s).map_err(ClientError::Internal)?),
        None => None,
    };
    let txn = session.effective_txn(explicit_txn);
    let context_id = session.effective_context(args.context);

    // Deduplication is on by default — encoding the same text twice in
    // the same (agent, context) should return the existing memory, not
    // create a duplicate. `--allow-duplicate` is the explicit opt-out
    // for episodic memory where the same content really is a second
    // distinct event.
    let deduplicate = !args.allow_duplicate;

    let mut b = client
        .encode(text.clone())
        .context(context_id)
        .salience(args.salience.unwrap_or(0.5))
        .deduplicate(deduplicate);
    if let Some(k) = args.kind {
        b = b.kind(k.into_wire());
    }
    if let Some(t) = txn {
        b = b.txn(t);
    }
    if !args.edges.is_empty() {
        let edges = args.edges.iter().map(|e| e.into_request()).collect();
        b = b.edges(edges);
    }
    if let Some(rid) = request_id {
        b = b.request_id(rid);
    }
    let resp = b.send().await?;
    session.push_recent_id(MemoryId::from_raw(resp.memory_id));

    if args.wait_for_extraction {
        // `--wait-for-extraction` is sugar for "wait for the extractor
        // stage of this write to finish." Filter the ack's
        // pending_stages to just the extractor entry; an empty filter
        // returns immediately (no extractor was queued).
        use brain_protocol::responses::StageKind;
        let stages: Vec<StageKind> = resp
            .pending_stages
            .iter()
            .copied()
            .filter(|k| *k == StageKind::Extractor)
            .collect();
        wait_for_stages(
            client,
            MemoryId::from_raw(resp.memory_id),
            &stages,
            resp.lsn,
        )
        .await?;
    }

    // When --wait-auto-edges-ms is positive, open a filtered subscribe
    // stream for that window and collect EdgeAdded(AUTO_DERIVED) events
    // whose `from_id` matches this encode's memory id. The watcher
    // returns whatever it sees within the window; non-blocking on
    // success or empty result. The encode response already left the
    // wire — the watcher only amends what we render to the user.
    let auto_edges_delta = if args.wait_auto_edges_ms > 0 {
        let delta = watch_auto_edges(
            client,
            MemoryId::from_raw(resp.memory_id),
            resp.lsn,
            args.wait_auto_edges_ms,
        )
        .await;
        Some(delta)
    } else {
        None
    };

    let mut rendered = EncodeRendered::new(resp).with_source(text.clone());
    if let Some(delta) = auto_edges_delta {
        rendered = rendered.with_auto_edges_delta(delta);
    }
    Ok(Box::new(rendered))
}

/// Pull text from whichever source the user picked. Errors when no
/// source resolves to a non-empty payload.
fn resolve_source_text(args: &EncodeArgs) -> Result<String, ClientError> {
    if args.from_stdin {
        return read_stdin();
    }
    if let Some(path) = &args.from_file {
        if path == "-" {
            return read_stdin();
        }
        if path.ends_with(".jsonl") {
            tracing::warn!(
                target: "brain_shell",
                ".jsonl batching opens a TXN per file and submits one encode per line. \
                 SDK does not yet expose multi-encode batching; current implementation \
                 reads the file but submits a single encode of the first line. \
                 Wire the txn/batch path in a follow-up.",
            );
            todo!(
                "follow-up: implement .jsonl batching via TxnBegin + per-line encode + \
                 TxnCommit. Requires a multi-statement encode helper, or repeated single \
                 sends in the txn — needs an explicit decision on which."
            );
        }
        return std::fs::read_to_string(path)
            .map_err(|e| ClientError::Internal(format!("read {path}: {e}")));
    }
    match &args.text {
        Some(t) if !t.is_empty() => Ok(t.clone()),
        _ => Err(ClientError::Internal(
            "encode requires a TEXT positional or one of --from-file / --from-stdin".into(),
        )),
    }
}

fn read_stdin() -> Result<String, ClientError> {
    let mut s = String::new();
    std::io::stdin()
        .read_to_string(&mut s)
        .map_err(|e| ClientError::Internal(format!("read stdin: {e}")))?;
    Ok(s)
}

fn parse_request_id(arg: Option<&str>) -> Result<Option<RequestId>, ClientError> {
    let Some(s) = arg else { return Ok(None) };
    let uuid = Uuid::parse_str(s.trim())
        .map_err(|e| ClientError::Internal(format!("bad --request-id `{s}`: {e}")))?;
    Ok(Some(RequestId(uuid)))
}

/// Subscribe at `lsn+1` and collect `EdgeAdded(AUTO_DERIVED)` events
/// whose source matches `memory_id` for up to `window_ms` milliseconds.
/// Returns whatever the watcher saw — empty list when the worker
/// didn't pair this memory or didn't run in time.
///
/// Errors during subscribe are logged and swallowed: the encode
/// already succeeded; an observation problem must not crash the
/// caller's response.
async fn watch_auto_edges(
    client: &Client,
    memory_id: MemoryId,
    start_lsn: u64,
    window_ms: u32,
) -> AutoEdgesDelta {
    // Origin discriminator on the wire's `EdgeEventPayload.origin`.
    // Mirrors `brain_metadata::tables::edge::origin::AUTO_DERIVED`;
    // hardcoded here so brain-shell doesn't pull brain-metadata in
    // for a single byte. If the meaning ever changes the broken
    // filter would silently keep `EXPLICIT` edges through, which is
    // visible immediately on first run (the wrong column on the card).
    const AUTO_DERIVED: u8 = 1;

    let started = std::time::Instant::now();
    let from_id_bytes = memory_id.raw().to_be_bytes();
    let mut edges: Vec<AutoEdgeSummary> = Vec::new();

    let stream_result = client
        .subscribe()
        .start_lsn(start_lsn.saturating_add(1))
        .send_stream()
        .await;
    let mut stream = match stream_result {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                target: "brain_shell",
                "encode --wait-auto-edges-ms: subscribe failed ({e}); \
                 encode succeeded, returning empty delta.",
            );
            return AutoEdgesDelta {
                elapsed_ms: started.elapsed().as_millis() as u64,
                edges,
            };
        }
    };

    let window = Duration::from_millis(u64::from(window_ms));
    let deadline = tokio::time::Instant::now() + window;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let next = match tokio::time::timeout(remaining, stream.next()).await {
            Ok(Some(Ok(ev))) => ev,
            Ok(Some(Err(e))) => {
                tracing::debug!(
                    target: "brain_shell",
                    "encode --wait-auto-edges-ms: stream error ({e}); returning partial delta."
                );
                break;
            }
            Ok(None) => break, // server closed the stream
            Err(_) => break,   // timeout — done
        };
        if !matches!(next.event_type, EventType::EdgeAdded) {
            continue;
        }
        let Some(payload) = next.edge_payload.as_ref() else {
            continue;
        };
        // Match the encode's memory on either side of the edge.
        // Different workers stamp the new memory in different
        // positions:
        //   * AutoEdgeWorker (SimilarTo) writes `new → similar`
        //     (and a mirror), so `from_id == this memory`.
        //   * TemporalEdgeWorker (FollowedBy) writes
        //     `predecessor → new`, so `to_id == this memory`.
        //   * CausalEdgeWorker (when it lands) is direction-by-
        //     statement-semantics; either side can match.
        // Reject EXPLICIT-origin events so a concurrent `link`
        // call doesn't appear in the delta line.
        if payload.origin != AUTO_DERIVED {
            continue;
        }
        let this_is_source = payload.from_id == from_id_bytes;
        let this_is_target = payload.to_id == from_id_bytes;
        if !this_is_source && !this_is_target {
            continue;
        }
        // Display the OTHER end of the edge — the one the user
        // actually cares about ("what got linked to my new memory").
        let other_bytes = if this_is_source {
            payload.to_id
        } else {
            payload.from_id
        };
        edges.push(AutoEdgeSummary {
            target: u128::from_be_bytes(other_bytes),
            kind: edge_kind_label(payload.edge_kind_tag, payload.edge_kind_byte),
            weight: payload.weight,
        });
    }

    AutoEdgesDelta {
        elapsed_ms: started.elapsed().as_millis() as u64,
        edges,
    }
}

/// Cheap label for the edge-kind discriminator the wire ships.
/// `tag 0` = `Builtin(EdgeKind)`; the byte is the substrate kind.
/// `tag 1` = `Mentions`; `tag 2` = `Typed(RelationTypeId)`. Auto-edges
/// are always `Builtin(SimilarTo)` today; the other variants render
/// generically in case future workers emit them.
fn edge_kind_label(tag: u8, byte: u8) -> String {
    match tag {
        0 => match byte {
            0 => "Caused",
            1 => "FollowedBy",
            2 => "DerivedFrom",
            3 => "SimilarTo",
            4 => "Contradicts",
            5 => "Supports",
            6 => "References",
            7 => "PartOf",
            _ => "Builtin",
        }
        .to_string(),
        1 => "Mentions".to_string(),
        2 => "Typed".to_string(),
        _ => format!("kind({tag},{byte})"),
    }
}

/// Subscribe at `lsn+1` and block until *every* pending stage for the
/// just-submitted memory has emitted a `StageCompleted` event — or
/// until the deadline fires. `pending_stages` comes from the encode
/// ack and lists which background stages the write queued; the
/// helper decrements them as events arrive.
///
/// When `pending_stages` is empty the helper returns immediately —
/// there's nothing to wait for (substrate-only deployment, dedup hit,
/// or workers disabled).
async fn wait_for_stages(
    client: &Client,
    memory_id: MemoryId,
    pending_stages: &[brain_protocol::responses::StageKind],
    start_lsn: u64,
) -> Result<(), ClientError> {
    use brain_protocol::responses::StageKind;

    if pending_stages.is_empty() {
        return Ok(());
    }
    let target_raw = memory_id.raw();
    let mut remaining_kinds: std::collections::HashSet<StageKind> =
        pending_stages.iter().copied().collect();

    let stream_result = client
        .subscribe()
        .start_lsn(start_lsn.saturating_add(1))
        .send_stream()
        .await;
    let mut stream = match stream_result {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                target: "brain_shell",
                "encode --wait: subscribe failed ({e}); encode succeeded, returning anyway."
            );
            return Ok(());
        }
    };
    // Hard cap so a worker that never drains doesn't pin the shell.
    // 60s is generous; the global --timeout cap is what's printed.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while !remaining_kinds.is_empty() {
        let elapsed = deadline.saturating_duration_since(tokio::time::Instant::now());
        if elapsed.is_zero() {
            tracing::warn!(
                target: "brain_shell",
                "encode --wait: timed out waiting for stages {:?} on memory {}; \
                 returning without confirmation.",
                remaining_kinds,
                target_raw,
            );
            return Ok(());
        }
        let next = match tokio::time::timeout(elapsed, stream.next()).await {
            Ok(Some(Ok(ev))) => ev,
            Ok(Some(Err(e))) => return Err(e),
            Ok(None) => {
                tracing::warn!(
                    target: "brain_shell",
                    "encode --wait: subscription closed before stages {:?} completed for {}.",
                    remaining_kinds,
                    target_raw,
                );
                return Ok(());
            }
            Err(_) => continue,
        };
        if !matches!(next.event_type, EventType::StageCompleted) {
            continue;
        }
        if next.memory_id != target_raw {
            continue;
        }
        if let Some(kind) = next.stage_kind {
            remaining_kinds.remove(&kind);
        }
    }
    Ok(())
}
