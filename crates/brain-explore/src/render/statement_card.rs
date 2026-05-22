//! Stacked card for a single statement's full record.
//!
//! Shape mirrors the entity card so a user moving between `entity show`
//! and `statement show` sees the same idiom: header line + indented id,
//! then optional sections. The evidence-memories list is hyperlinked
//! back into a `recall show` so a click in iTerm / kitty navigates
//! evidence → statement → evidence cleanly.

use std::io::{self, Write};

use serde_json::{json, Value};

use crate::render::entity_card::MemorySummary;
use crate::table::middle_truncate;
use crate::term::hyperlink::link;
use crate::theme::Token;
use crate::util::humanize::humanize_age;
use crate::{Render, RenderCtx};

/// What sits on the right-hand side of a Fact/Preference/Event triple.
///
/// Variants cover the three statement-kind object shapes from the
/// knowledge model: an entity reference, a typed scalar literal, or a
/// raw string label (Event objects often serialise as free-text spans).
#[derive(Debug, Clone)]
pub enum ObjectRef {
    Entity { id: String, name: String },
    Literal(String),
}

impl ObjectRef {
    fn label(&self) -> &str {
        match self {
            ObjectRef::Entity { name, .. } => name,
            ObjectRef::Literal(s) => s,
        }
    }
}

pub struct StatementCard {
    pub id: String,
    pub kind: String,
    pub subject_canonical: String,
    pub predicate_qname: String,
    pub object: ObjectRef,
    pub confidence: f32,
    pub evidence_memories: Vec<MemorySummary>,
    /// Set when the statement landed on the `brain:fact` wildcard
    /// sink: the renderer displays this in place of `predicate_qname`
    /// and appends an "(auto-coined)" hint so the user knows the
    /// predicate isn't in the schema yet.
    pub original_predicate_qname: Option<String>,
    /// Record-time invalidation. When set, the substrate no longer
    /// believes this statement (superseded / tombstoned / FORGET
    /// cascade). The renderer surfaces this with a warning line so the
    /// user knows the row is historical — the time-travel signal for
    /// agents browsing past beliefs without resurrecting tombstones.
    pub record_invalidated_at_unix_nanos: Option<u64>,
}

impl Render for StatementCard {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let policy = ctx.policy;
        let theme = &ctx.theme;
        let body_width = policy.width.saturating_sub(2);

        let subject = theme.paint(Token::Accent, &self.subject_canonical, policy);
        // Auto-coined rows surface the LLM's original predicate name
        // instead of the literal `brain:fact` so the user sees the
        // intent; the suffix tells them the predicate isn't declared
        // in the schema yet (a declare-this hint).
        let (predicate_label, auto_coined) = match &self.original_predicate_qname {
            Some(orig) => (orig.clone(), true),
            None => (self.predicate_qname.clone(), false),
        };
        let predicate = theme.paint(Token::Predicate, &predicate_label, policy);
        let object = match &self.object {
            ObjectRef::Entity { name, .. } => theme.paint(Token::EntityId, name, policy),
            ObjectRef::Literal(s) => theme.paint(Token::Value, s, policy),
        };
        let kind = theme.paint(Token::Muted, &self.kind, policy);
        if auto_coined {
            let hint = theme.paint(Token::Muted, "(auto-coined)", policy);
            writeln!(w, "[{kind}] {subject} {predicate} {object}  {hint}")?;
        } else {
            writeln!(w, "[{kind}] {subject} {predicate} {object}")?;
        }

        let id_painted = theme.paint(Token::StatementId, &self.id, policy);
        let conf_str = format!("{:.2}", self.confidence);
        let conf = theme.paint(Token::Confidence, &conf_str, policy);
        writeln!(w, "  id = {id_painted}   conf = {conf}")?;

        if let Some(ts) = self.record_invalidated_at_unix_nanos {
            let age = humanize_age(ts);
            let object_text = self.object.label();
            let warning = format!(
                "  Brain stopped believing this {age} (was: {} {} {})",
                self.subject_canonical, predicate_label, object_text,
            );
            let painted = theme.paint(Token::Muted, &warning, policy);
            writeln!(w, "{painted}")?;
        }
        writeln!(w)?;

        if !self.evidence_memories.is_empty() {
            let heading = theme.paint(Token::Label, "Evidence", policy);
            writeln!(w, "{heading}")?;
            for m in &self.evidence_memories {
                let label = theme.paint(Token::MemoryId, &m.short_id, policy);
                let memory_link = link(policy, &label, &format!("brain://recall/{}", m.short_id));
                let text = middle_truncate(&m.text, body_width.saturating_sub(20));
                writeln!(w, "  · {memory_link}  {text}")?;
            }
            writeln!(w)?;
        }

        Ok(())
    }

    fn render_json(&self, _ctx: &RenderCtx) -> Value {
        let object = match &self.object {
            ObjectRef::Entity { id, name } => json!({"kind": "entity", "id": id, "name": name}),
            ObjectRef::Literal(s) => json!({"kind": "literal", "value": s}),
        };
        json!({
            "id": self.id,
            "statement_kind": self.kind,
            "subject_canonical": self.subject_canonical,
            "predicate": self.predicate_qname,
            "original_predicate_qname": self.original_predicate_qname,
            "auto_coined": self.original_predicate_qname.is_some(),
            "object": object,
            "confidence": self.confidence,
            "evidence_memories": self.evidence_memories.iter().map(|m| json!({
                "memory_id": m.short_id,
                "text": m.text,
            })).collect::<Vec<_>>(),
            // Bare label form for tools that only want the right-hand side.
            "object_label": self.object.label(),
            // Record-time invalidation surfaces both the raw unix-nanos
            // (for downstream tooling that wants exact wall-clock) and
            // a boolean flag so JSON consumers can branch without
            // parsing the timestamp.
            "record_invalidated_at_unix_nanos": self.record_invalidated_at_unix_nanos,
            "record_invalidated": self.record_invalidated_at_unix_nanos.is_some(),
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

    fn sample() -> StatementCard {
        StatementCard {
            id: "stmt_1".into(),
            kind: "Fact".into(),
            subject_canonical: "Priya".into(),
            predicate_qname: "works_at".into(),
            object: ObjectRef::Entity {
                id: "ent_acme".into(),
                name: "Acme Corp".into(),
            },
            confidence: 0.95,
            evidence_memories: vec![MemorySummary {
                short_id: "s2/m17/v1".into(),
                text: "Priya works at Acme Corp".into(),
            }],
            original_predicate_qname: None,
            record_invalidated_at_unix_nanos: None,
        }
    }

    #[test]
    fn table_shows_triple_and_evidence() {
        let mut buf = Vec::new();
        sample().render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("Priya"));
        assert!(s.contains("works_at"));
        assert!(s.contains("Acme Corp"));
        assert!(s.contains("Evidence"));
        assert!(s.contains("s2/m17/v1"));
    }

    #[test]
    fn omits_evidence_section_when_empty() {
        let mut c = sample();
        c.evidence_memories.clear();
        let mut buf = Vec::new();
        c.render_table(&ctx(), &mut buf).unwrap();
        assert!(!String::from_utf8(buf).unwrap().contains("Evidence"));
    }

    #[test]
    fn json_distinguishes_entity_and_literal_objects() {
        let v = sample().render_json(&ctx());
        assert_eq!(v["object"]["kind"], "entity");
        let mut c = sample();
        c.object = ObjectRef::Literal("Senior Engineer".into());
        let v = c.render_json(&ctx());
        assert_eq!(v["object"]["kind"], "literal");
        assert_eq!(v["object"]["value"], "Senior Engineer");
    }

    #[test]
    fn auto_coined_predicate_shows_original_and_hint() {
        let mut c = sample();
        c.predicate_qname = "brain:fact".into();
        c.original_predicate_qname = Some("works_at".into());
        c.object = ObjectRef::Literal("billing rewrite".into());

        let mut buf = Vec::new();
        c.render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        // Renderer shows the LLM's qname, not the literal sink name.
        assert!(s.contains("works_at"));
        assert!(!s.contains("brain:fact"));
        assert!(s.contains("(auto-coined)"));
    }

    #[test]
    fn declared_predicate_omits_auto_coined_hint() {
        let mut buf = Vec::new();
        sample().render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(!s.contains("(auto-coined)"));
    }

    #[test]
    fn record_invalidated_renders_warning_line() {
        let mut c = sample();
        c.record_invalidated_at_unix_nanos = Some(1);
        let mut buf = Vec::new();
        c.render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("Brain stopped believing this"));
        assert!(s.contains("Priya"));
        assert!(s.contains("works_at"));
        assert!(s.contains("Acme Corp"));
    }

    #[test]
    fn record_invalidated_absent_no_warning_line() {
        let mut buf = Vec::new();
        sample().render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(!s.contains("Brain stopped believing this"));
    }

    #[test]
    fn json_carries_record_invalidated_flag() {
        let mut c = sample();
        c.record_invalidated_at_unix_nanos = Some(1_700_000_000_000_000_000);
        let v = c.render_json(&ctx());
        assert_eq!(v["record_invalidated"], true);
        assert_eq!(
            v["record_invalidated_at_unix_nanos"],
            1_700_000_000_000_000_000_u64
        );

        let v2 = sample().render_json(&ctx());
        assert_eq!(v2["record_invalidated"], false);
    }

    #[test]
    fn json_carries_auto_coined_metadata() {
        let mut c = sample();
        c.predicate_qname = "brain:fact".into();
        c.original_predicate_qname = Some("works_at".into());
        let v = c.render_json(&ctx());
        assert_eq!(v["original_predicate_qname"], "works_at");
        assert_eq!(v["auto_coined"], true);

        let v2 = sample().render_json(&ctx());
        assert_eq!(v2["auto_coined"], false);
    }
}
