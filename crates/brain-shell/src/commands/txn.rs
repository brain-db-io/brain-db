//! `txn begin` / `txn commit` / `txn abort`.

use brain_explore::{TxnAbortRendered, TxnBeginRendered, TxnCommitRendered};
use brain_sdk_rust::{Client, ClientError};

use crate::parser::{parse_txn_id, TxnCommand};
use crate::session::Session;

use super::{is_txn_terminal, Rendered};

pub async fn run(
    client: &Client,
    session: &mut Session,
    cmd: TxnCommand,
) -> Result<Rendered, ClientError> {
    match cmd {
        TxnCommand::Begin { idle_timeout } => {
            let resp = client.txn_begin_with_timeout(idle_timeout).await?;
            session.active_txn = Some(resp.txn_id);
            Ok(Box::new(TxnBeginRendered(resp)))
        }
        TxnCommand::Commit { id } => {
            let bytes = resolve_txn_id(id.as_deref(), session)?;
            let result = client.txn_commit(bytes).await;
            clear_if_matches(session, bytes, &result);
            let resp = result?;
            Ok(Box::new(TxnCommitRendered(resp)))
        }
        TxnCommand::Abort { id } => {
            let bytes = resolve_txn_id(id.as_deref(), session)?;
            let result = client.txn_abort(bytes).await;
            clear_if_matches(session, bytes, &result);
            let resp = result?;
            Ok(Box::new(TxnAbortRendered(resp)))
        }
    }
}

/// Resolve the txn id for a `commit` / `abort`: explicit hex arg
/// wins; otherwise fall back to the session's active txn. Refuses
/// when both are missing — the operator has no txn to act on.
fn resolve_txn_id(arg: Option<&str>, session: &Session) -> Result<[u8; 16], ClientError> {
    if let Some(s) = arg {
        return parse_txn_id(s).map_err(ClientError::Internal);
    }
    match session.active_txn {
        Some(bytes) => Ok(bytes),
        None => Err(ClientError::Internal(
            "txn commit / abort needs an id — no transaction is currently \
             attached to this session. Run `txn begin` first or pass the \
             id explicitly."
                .into(),
        )),
    }
}

/// Drop `session.active_txn` if the call matched the active id AND
/// either succeeded or failed with a terminal txn error (the server
/// no longer knows the id or it's no longer Active). Other errors
/// (network, validation, …) leave the session untouched — the user
/// can retry.
fn clear_if_matches<T>(session: &mut Session, bytes: [u8; 16], result: &Result<T, ClientError>) {
    if session.active_txn != Some(bytes) {
        return;
    }
    let should_clear = match result {
        Ok(_) => true,
        Err(e) => is_txn_terminal(e),
    };
    if should_clear {
        session.active_txn = None;
    }
}
