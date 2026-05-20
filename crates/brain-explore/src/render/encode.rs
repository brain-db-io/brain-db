//! ENCODE response renderer.
//!
//! Wrapped in [`EncodeRendered`] so we can carry the request's
//! `deduplicate` flag alongside the response — the wire response can
//! only tell us whether dedup *fired*, not whether it was even
//! requested. Without that distinction the operator can't tell
//! "dedup off" from "dedup on, miss."

use std::io::{self, Write};

use brain_protocol::response::EncodeResponse;
use serde_json::{json, Value};

use crate::render::{fmt_hex_16, fmt_id, fmt_short_hex_16, fmt_short_id};
use crate::theme::Token;
use crate::{Render, RenderCtx};

/// Render-time wrapper around [`EncodeResponse`] carrying the request's
/// `deduplicate` flag.
pub struct EncodeRendered {
    pub response: EncodeResponse,
    pub dedup_requested: bool,
}

impl EncodeRendered {
    fn dedup_state(&self) -> &'static str {
        match (self.dedup_requested, self.response.was_deduplicated) {
            (false, _) => "off",
            (true, true) => "hit",
            (true, false) => "miss",
        }
    }
}

impl Render for EncodeRendered {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let r = &self.response;
        let policy = ctx.policy;
        let theme = &ctx.theme;
        let id_short = fmt_short_id(r.memory_id);
        let id_cell = theme.paint(Token::MemoryId, &id_short, policy);
        let ok = theme.paint(Token::Success, "ok", policy);
        // `lsn == 0` is the wire sentinel for "no LSN" (dedup hit or
        // cached replay of one). Show it as an em-dash so operators
        // don't read it as a real position-zero LSN.
        let lsn_text: String = if r.lsn == 0 {
            theme.paint(Token::Muted, "—", policy).into_owned()
        } else {
            r.lsn.to_string()
        };
        writeln!(w, "{ok}  {id_cell}  lsn={lsn_text}")?;
        let kind = format!("{:?}", r.kind).to_lowercase();
        let mut parts: Vec<String> = vec![
            format!("agent={}", fmt_short_hex_16(&r.agent_id)),
            format!("ctx={}", r.context_id),
            kind,
            format!("sal={:.3}", r.salience),
        ];
        if r.edges_out_count > 0 {
            parts.push(format!("edges_out={}", r.edges_out_count));
        }
        let dedup = self.dedup_state();
        if dedup != "off" {
            parts.push(format!("dedup={dedup}"));
        }
        parts.push(format!("fp={}", fmt_short_hex_16(&r.embedding_model_fp)));
        writeln!(w, "    {}", parts.join(" · "))
    }

    fn render_json(&self, _ctx: &RenderCtx) -> Value {
        let r = &self.response;
        json!({
            "memory_id": fmt_id(r.memory_id),
            "lsn": r.lsn,
            "dedup": self.dedup_state(),
            "was_deduplicated": r.was_deduplicated,
            "salience": r.salience,
            "auto_edges_added": r.auto_edges_added,
            "agent_id": fmt_hex_16(&r.agent_id),
            "context_id": r.context_id,
            "kind": format!("{:?}", r.kind),
            "created_at_unix_nanos": r.created_at_unix_nanos,
            "edges_out_count": r.edges_out_count,
            "embedding_model_fp": fmt_hex_16(&r.embedding_model_fp),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::OutputFormat;
    use crate::theme::Theme;
    use crate::TermPolicy;
    use brain_core::MemoryId;
    use brain_protocol::request::MemoryKindWire;

    fn sample() -> EncodeRendered {
        EncodeRendered {
            response: EncodeResponse {
                memory_id: MemoryId::pack(0, 1, 1).raw(),
                was_deduplicated: false,
                salience: 0.42,
                auto_edges_added: 0,
                lsn: 17,
                agent_id: [0xAB; 16],
                context_id: 7,
                kind: MemoryKindWire::Episodic,
                created_at_unix_nanos: 0,
                edges_out_count: 0,
                embedding_model_fp: [0xCD; 16],
            },
            dedup_requested: true,
        }
    }

    fn ctx() -> RenderCtx {
        RenderCtx {
            policy: TermPolicy::plain(),
            theme: Theme::default(),
            format: OutputFormat::Table,
        }
    }

    #[test]
    fn dedup_state_distinguishes_off_hit_miss() {
        let mut r = sample();
        r.dedup_requested = false;
        assert_eq!(r.dedup_state(), "off");
        r.dedup_requested = true;
        r.response.was_deduplicated = true;
        assert_eq!(r.dedup_state(), "hit");
        r.response.was_deduplicated = false;
        assert_eq!(r.dedup_state(), "miss");
    }

    #[test]
    fn table_includes_id_lsn_and_dedup_when_relevant() {
        let mut buf = Vec::new();
        sample().render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("ok"));
        assert!(s.contains("lsn=17"));
        assert!(s.contains("dedup=miss"));
    }

    #[test]
    fn json_carries_canonical_id() {
        let v = sample().render_json(&ctx());
        let id = v["memory_id"].as_str().unwrap();
        assert!(id.starts_with("0x"));
        assert_eq!(v["dedup"], "miss");
    }

    #[test]
    fn render_table_shows_dash_when_lsn_is_zero() {
        let mut r = sample();
        r.response.lsn = 0;
        let mut buf = Vec::new();
        r.render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("lsn=—"), "expected em-dash sentinel, got: {s}");
        assert!(!s.contains("lsn=0"));
    }

    #[test]
    fn render_table_shows_value_when_lsn_is_nonzero() {
        let mut buf = Vec::new();
        sample().render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("lsn=17"));
        assert!(!s.contains("lsn=—"));
    }

    #[test]
    fn render_json_shows_zero_literally() {
        // JSON is the machine-readable view; clients apply the
        // sentinel convention themselves, so we MUST NOT swap `0`
        // for a string here.
        let mut r = sample();
        r.response.lsn = 0;
        let v = r.render_json(&ctx());
        assert_eq!(v["lsn"], 0);
    }
}
