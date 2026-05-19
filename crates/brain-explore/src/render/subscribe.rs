//! SUBSCRIBE event renderer — one event at a time.
//!
//! The streaming loop is the caller's job (brain-shell's subscribe
//! command holds the SSE/wire connection). This renderer just renders
//! one event into a single row; the caller can either append it to a
//! running table or print one row per event in tail-f style.

use std::io::{self, Write};

use brain_protocol::response::SubscriptionEvent;
use comfy_table::Cell;
use serde_json::{json, Value};

use crate::render::{fmt_id, fmt_kind, fmt_short_id};
use crate::table::build_table;
use crate::theme::Token;
use crate::{Render, RenderCtx};

/// Newtype around `SubscriptionEvent` for [`Render`] dispatch.
///
/// Renders a single-row table; collections live in a tiny
/// [`SubscriptionEventList`] wrapper so a batch dump (e.g. after a
/// `replay --until-lsn=…`) prints as one table.
pub struct SubscriptionEventRendered(pub SubscriptionEvent);

/// Optional batch wrapper. Renders a multi-row table.
pub struct SubscriptionEventList(pub Vec<SubscriptionEvent>);

impl Render for SubscriptionEventRendered {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let mut table = build_table(ctx.policy);
        push_header(&mut table, ctx);
        push_event_row(&mut table, ctx, &self.0);
        writeln!(w, "{table}")
    }

    fn render_json(&self, _ctx: &RenderCtx) -> Value {
        event_to_json(&self.0)
    }
}

impl Render for SubscriptionEventList {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let mut table = build_table(ctx.policy);
        push_header(&mut table, ctx);
        for e in &self.0 {
            push_event_row(&mut table, ctx, e);
        }
        writeln!(w, "{table}")
    }

    fn render_json(&self, _ctx: &RenderCtx) -> Value {
        Value::Array(self.0.iter().map(event_to_json).collect())
    }
}

fn push_header(table: &mut comfy_table::Table, ctx: &RenderCtx) {
    let (theme, policy) = (&ctx.theme, ctx.policy);
    table.set_header(vec![
        Cell::new(theme.paint(Token::Label, "lsn", policy)),
        Cell::new(theme.paint(Token::Label, "type", policy)),
        Cell::new(theme.paint(Token::Label, "id", policy)),
        Cell::new(theme.paint(Token::Label, "ctx", policy)),
        Cell::new(theme.paint(Token::Label, "kind", policy)),
        Cell::new(theme.paint(Token::Label, "text", policy)),
    ]);
}

fn push_event_row(table: &mut comfy_table::Table, ctx: &RenderCtx, e: &SubscriptionEvent) {
    let (theme, policy) = (&ctx.theme, ctx.policy);
    table.add_row(vec![
        Cell::new(e.lsn),
        Cell::new(format!("{:?}", e.event_type)),
        Cell::new(theme.paint(Token::MemoryId, &fmt_short_id(e.memory_id), policy)),
        Cell::new(e.context_id),
        Cell::new(fmt_kind(e.kind)),
        Cell::new(&e.text),
    ]);
}

fn event_to_json(e: &SubscriptionEvent) -> Value {
    json!({
        "lsn": e.lsn,
        "event_type": format!("{:?}", e.event_type),
        "memory_id": fmt_id(e.memory_id),
        "context_id": e.context_id,
        "kind": fmt_kind(e.kind),
        "salience": e.salience,
        "timestamp_unix_nanos": e.timestamp_unix_nanos,
        "text": e.text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::OutputFormat;
    use crate::theme::Theme;
    use crate::TermPolicy;
    use brain_core::MemoryId;
    use brain_protocol::request::MemoryKindWire;
    use brain_protocol::response::types::EventType;

    fn ctx() -> RenderCtx {
        RenderCtx {
            policy: TermPolicy::plain(),
            theme: Theme::default(),
            format: OutputFormat::Table,
        }
    }

    fn event(text: &str) -> SubscriptionEvent {
        SubscriptionEvent {
            event_type: EventType::Encoded,
            memory_id: MemoryId::pack(0, 1, 1).raw(),
            context_id: 7,
            text: text.into(),
            kind: MemoryKindWire::Episodic,
            salience: 0.5,
            timestamp_unix_nanos: 0,
            lsn: 42,
            knowledge_payload: None,
            edge_payload: None,
        }
    }

    #[test]
    fn single_event_renders_table() {
        let r = SubscriptionEventRendered(event("hello"));
        let mut buf = Vec::new();
        r.render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("42"));
        assert!(s.contains("Encoded"));
        assert!(s.contains("hello"));
    }

    #[test]
    fn batch_renders_one_row_per_event() {
        let r = SubscriptionEventList(vec![event("a"), event("b")]);
        let mut buf = Vec::new();
        r.render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("a") && s.contains("b"));
    }
}
