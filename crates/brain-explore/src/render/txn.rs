//! Transaction lifecycle renderers (begin / commit / abort).
//!
//! Each wire response gets a thin newtype so the orphan-free `Render`
//! impl lives in this crate and the caller can name what they're
//! printing (e.g. `TxnBeginRendered`) instead of relying on the wire
//! struct's name implying a renderer.

use std::io::{self, Write};

use brain_protocol::response::{TxnAbortResponse, TxnBeginResponse, TxnCommitResponse};
use serde_json::{json, Value};

use crate::render::fmt_txn_id;
use crate::theme::Token;
use crate::{Render, RenderCtx};

/// Renderer for a `TXN_BEGIN_RESP` body.
pub struct TxnBeginRendered(pub TxnBeginResponse);

/// Renderer for a `TXN_COMMIT_RESP` body.
pub struct TxnCommitRendered(pub TxnCommitResponse);

/// Renderer for a `TXN_ABORT_RESP` body.
pub struct TxnAbortRendered(pub TxnAbortResponse);

impl Render for TxnBeginRendered {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let r = &self.0;
        let ok = ctx.theme.paint(Token::Success, "ok", ctx.policy);
        writeln!(
            w,
            "{ok}  txn_id={}  timeout_seconds={}",
            fmt_txn_id(&r.txn_id),
            r.timeout_seconds,
        )
    }

    fn render_json(&self, _ctx: &RenderCtx) -> Value {
        let r = &self.0;
        json!({
            "txn_id": fmt_txn_id(&r.txn_id),
            "timeout_seconds": r.timeout_seconds,
            "started_at_unix_nanos": r.started_at_unix_nanos,
        })
    }
}

impl Render for TxnCommitRendered {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let r = &self.0;
        let ok = ctx.theme.paint(Token::Success, "ok", ctx.policy);
        writeln!(
            w,
            "{ok}  txn_id={}  operations_applied={}",
            fmt_txn_id(&r.txn_id),
            r.operations_applied,
        )
    }

    fn render_json(&self, _ctx: &RenderCtx) -> Value {
        let r = &self.0;
        json!({
            "txn_id": fmt_txn_id(&r.txn_id),
            "operations_applied": r.operations_applied,
            "committed_at_unix_nanos": r.committed_at_unix_nanos,
        })
    }
}

impl Render for TxnAbortRendered {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let r = &self.0;
        let ok = ctx.theme.paint(Token::Success, "ok", ctx.policy);
        writeln!(
            w,
            "{ok}  txn_id={}  operations_discarded={}",
            fmt_txn_id(&r.txn_id),
            r.operations_discarded,
        )
    }

    fn render_json(&self, _ctx: &RenderCtx) -> Value {
        let r = &self.0;
        json!({
            "txn_id": fmt_txn_id(&r.txn_id),
            "operations_discarded": r.operations_discarded,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::OutputFormat;
    use crate::theme::Theme;
    use crate::TermPolicy;

    fn ctx() -> RenderCtx {
        RenderCtx {
            policy: TermPolicy::plain(),
            theme: Theme::default(),
            format: OutputFormat::Table,
        }
    }

    #[test]
    fn begin_table_shows_id_and_timeout() {
        let r = TxnBeginRendered(TxnBeginResponse {
            txn_id: [0; 16],
            timeout_seconds: 30,
            started_at_unix_nanos: 0,
        });
        let mut buf = Vec::new();
        r.render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("timeout_seconds=30"));
        assert!(s.contains("0x00000000"));
    }

    #[test]
    fn commit_table_shows_operations_applied() {
        let r = TxnCommitRendered(TxnCommitResponse {
            txn_id: [1; 16],
            committed_at_unix_nanos: 0,
            operations_applied: 7,
        });
        let mut buf = Vec::new();
        r.render_table(&ctx(), &mut buf).unwrap();
        assert!(String::from_utf8(buf)
            .unwrap()
            .contains("operations_applied=7"));
    }

    #[test]
    fn abort_table_shows_operations_discarded() {
        let r = TxnAbortRendered(TxnAbortResponse {
            txn_id: [2; 16],
            operations_discarded: 3,
        });
        let mut buf = Vec::new();
        r.render_table(&ctx(), &mut buf).unwrap();
        assert!(String::from_utf8(buf)
            .unwrap()
            .contains("operations_discarded=3"));
    }
}
