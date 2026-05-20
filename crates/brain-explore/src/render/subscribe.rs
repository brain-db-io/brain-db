//! SUBSCRIBE event renderer — one event at a time.
//!
//! The streaming loop is the caller's job (brain-shell's subscribe
//! command holds the SSE/wire connection). This renderer just renders
//! one event into a single row; the caller can either append it to a
//! running table or print one row per event in tail-f style.

use std::io::{self, Write};

use brain_protocol::knowledge::{AuditStatus, KnowledgeEventPayload};
use brain_protocol::response::types::EventType;
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
    let text = if matches!(e.event_type, EventType::ExtractedKnowledge) {
        extracted_knowledge_summary(e).unwrap_or_else(|| e.text.clone())
    } else {
        e.text.clone()
    };
    table.add_row(vec![
        Cell::new(e.lsn),
        Cell::new(format!("{:?}", e.event_type)),
        Cell::new(theme.paint(Token::MemoryId, &fmt_short_id(e.memory_id), policy)),
        Cell::new(e.context_id),
        Cell::new(fmt_kind(e.kind)),
        Cell::new(text),
    ]);
}

fn event_to_json(e: &SubscriptionEvent) -> Value {
    let mut out = json!({
        "lsn": e.lsn,
        "event_type": format!("{:?}", e.event_type),
        "memory_id": fmt_id(e.memory_id),
        "context_id": e.context_id,
        "kind": fmt_kind(e.kind),
        "salience": e.salience,
        "timestamp_unix_nanos": e.timestamp_unix_nanos,
        "text": e.text,
    });
    if let Some(payload) = extracted_knowledge_payload(e) {
        if let Some(map) = out.as_object_mut() {
            map.insert("entity_count".into(), json!(payload.entity_count));
            map.insert("statement_count".into(), json!(payload.statement_count));
            map.insert("relation_count".into(), json!(payload.relation_count));
            map.insert(
                "audit_status".into(),
                json!(audit_status_str(payload.audit_status)),
            );
        }
    }
    // EdgeAdded / EdgeRemoved / EdgeSuperseded events carry an
    // edge_payload side-channel — surface it in the JSON so agents
    // driving on the change feed can filter on origin (AUTO_DERIVED
    // vs EXPLICIT) and inspect the (from, to, kind, weight) tuple
    // without a second RPC.
    if let Some(ep) = e.edge_payload.as_ref() {
        if let Some(map) = out.as_object_mut() {
            map.insert(
                "edge_payload".into(),
                json!({
                    "from_kind": ep.from_kind,
                    "from_id": fmt_id_from_bytes(&ep.from_id),
                    "to_kind": ep.to_kind,
                    "to_id": fmt_id_from_bytes(&ep.to_id),
                    "edge_kind_tag": ep.edge_kind_tag,
                    "edge_kind_byte": ep.edge_kind_byte,
                    "relation_type_id": ep.relation_type_id,
                    "weight": ep.weight,
                    "relation_id": ep.relation_id.map(|b| fmt_id_from_bytes(&b)),
                    "superseded_relation_id": ep
                        .superseded_relation_id
                        .map(|b| fmt_id_from_bytes(&b)),
                    "origin": ep.origin,
                }),
            );
        }
    }
    out
}

/// Format a 16-byte id (memory id, entity id, relation id) as the
/// canonical `0x` + 32-hex form used everywhere else in the JSON
/// envelope. Mirrors `fmt_id` but takes raw bytes rather than the
/// `u128`-packed memory_id type.
fn fmt_id_from_bytes(b: &[u8; 16]) -> String {
    let mut s = String::with_capacity(34);
    s.push_str("0x");
    for byte in b {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

/// Format the `ExtractedKnowledge` knowledge_payload as a short text
/// summary for table rendering. Returns `None` if the event isn't an
/// `ExtractedKnowledge` carrying its payload — falls back to the
/// substrate `text` field.
fn extracted_knowledge_summary(e: &SubscriptionEvent) -> Option<String> {
    let p = extracted_knowledge_payload(e)?;
    Some(format!(
        "{} entities, {} statements, {} relations ({})",
        p.entity_count,
        p.statement_count,
        p.relation_count,
        audit_status_str(p.audit_status),
    ))
}

fn extracted_knowledge_payload(
    e: &SubscriptionEvent,
) -> Option<&brain_protocol::knowledge::ExtractedKnowledgeEvent> {
    match &e.knowledge_payload {
        Some(KnowledgeEventPayload::ExtractedKnowledge(p)) => Some(p),
        _ => None,
    }
}

fn audit_status_str(s: AuditStatus) -> &'static str {
    match s {
        AuditStatus::Succeeded => "succeeded",
        AuditStatus::PartiallyApplied => "partially_applied",
        AuditStatus::Failed => "failed",
        AuditStatus::Skipped => "skipped",
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
    fn extracted_knowledge_renders_counts_and_status() {
        use brain_protocol::knowledge::{ExtractedKnowledgeEvent, KnowledgeEventPayload};

        let ev = SubscriptionEvent {
            event_type: EventType::ExtractedKnowledge,
            memory_id: MemoryId::pack(0, 17, 1).raw(),
            context_id: 0,
            text: String::new(),
            kind: MemoryKindWire::Episodic,
            salience: 0.0,
            timestamp_unix_nanos: 0,
            lsn: 99,
            knowledge_payload: Some(KnowledgeEventPayload::ExtractedKnowledge(
                ExtractedKnowledgeEvent {
                    memory_id: MemoryId::pack(0, 17, 1).raw(),
                    entity_count: 3,
                    statement_count: 5,
                    relation_count: 2,
                    audit_status: AuditStatus::Succeeded,
                },
            )),
            edge_payload: None,
        };

        let r = SubscriptionEventRendered(ev.clone());
        let mut buf = Vec::new();
        r.render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // comfy_table may wrap the summary across cells on a narrow
        // policy; assert the individual count tokens land somewhere in
        // the output and that the audit-status verb is present.
        assert!(s.contains("ExtractedKnowledge"), "rendered: {s}");
        assert!(s.contains("entities") && s.contains('3'), "rendered: {s}");
        assert!(s.contains("statements") && s.contains('5'), "rendered: {s}");
        assert!(s.contains("relations") && s.contains('2'), "rendered: {s}");
        assert!(s.contains("succeeded"), "rendered: {s}");

        let json = r.render_json(&ctx());
        assert_eq!(json["entity_count"], 3);
        assert_eq!(json["statement_count"], 5);
        assert_eq!(json["relation_count"], 2);
        assert_eq!(json["audit_status"], "succeeded");
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
