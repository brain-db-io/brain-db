//! User-facing error card.
//!
//! Wraps the wire `ErrorResponse` shape into a display-only struct so
//! renderers don't pull in the protocol's rkyv types and so this layer
//! stays free to evolve presentation independently of wire shape
//! changes. Callers (brain-shell, brain-cli) build a [`RenderableError`]
//! from whatever they have in hand — a wire `ErrorResponse`, an SDK
//! `BrainError`, or a hand-built diagnostic.

use std::io::{self, Write};

use serde_json::{json, Value};

use crate::theme::Token;
use crate::{Render, RenderCtx};

/// Display-only error wrapper.
///
/// `code` and `category` are stringly-typed because this struct is
/// rendered, not interpreted: the caller decides what string form is
/// useful for humans (`"PermissionDenied"`, `"0x0030"`, …) without
/// forcing the protocol's wire enums into the rendering layer.
pub struct RenderableError {
    pub code: String,
    pub category: String,
    pub message: String,
    pub details: Option<Value>,
    pub retry_after_ms: Option<u32>,
}

impl Render for RenderableError {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let policy = ctx.policy;
        let theme = &ctx.theme;
        let tag = theme.paint(Token::Error, "ERROR", policy);
        let code = theme.paint(Token::Accent, &self.code, policy);
        let category = theme.paint(Token::Muted, &self.category, policy);
        writeln!(w, "{tag}  {code}  [{category}]")?;
        writeln!(w, "  {}", self.message)?;
        if let Some(details) = &self.details {
            let heading = theme.paint(Token::Label, "Details", policy);
            writeln!(w, "{heading}")?;
            // Pretty-print so the operator can read nested validation
            // errors without piping through jq.
            let rendered =
                serde_json::to_string_pretty(details).unwrap_or_else(|_| details.to_string());
            for line in rendered.lines() {
                writeln!(w, "  {line}")?;
            }
        }
        if let Some(ms) = self.retry_after_ms {
            let hint_str = format!("retry after {ms} ms");
            let hint = theme.paint(Token::Muted, &hint_str, policy);
            writeln!(w, "  ({hint})")?;
        }
        Ok(())
    }

    fn render_json(&self, _ctx: &RenderCtx) -> Value {
        json!({
            "code": self.code,
            "category": self.category,
            "message": self.message,
            "details": self.details,
            "retry_after_ms": self.retry_after_ms,
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
    fn renders_minimum_fields() {
        let e = RenderableError {
            code: "InvalidArgument".into(),
            category: "Validation".into(),
            message: "top_k must be > 0".into(),
            details: None,
            retry_after_ms: None,
        };
        let mut buf = Vec::new();
        e.render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("ERROR"));
        assert!(s.contains("InvalidArgument"));
        assert!(s.contains("Validation"));
        assert!(s.contains("top_k must be > 0"));
    }

    #[test]
    fn renders_details_and_retry_hint() {
        let e = RenderableError {
            code: "RateLimited".into(),
            category: "ResourceExhausted".into(),
            message: "slow down".into(),
            details: Some(json!({"window_ms": 1000})),
            retry_after_ms: Some(500),
        };
        let mut buf = Vec::new();
        e.render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("Details"));
        assert!(s.contains("window_ms"));
        assert!(s.contains("retry after 500 ms"));
    }

    #[test]
    fn json_round_trips_all_fields() {
        let e = RenderableError {
            code: "MemoryNotFound".into(),
            category: "NotFound".into(),
            message: "no such id".into(),
            details: Some(json!({"id": "0x0"})),
            retry_after_ms: None,
        };
        let v = e.render_json(&ctx());
        assert_eq!(v["code"], "MemoryNotFound");
        assert_eq!(v["category"], "NotFound");
        assert_eq!(v["details"]["id"], "0x0");
        assert_eq!(v["retry_after_ms"], Value::Null);
    }
}
