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
}

impl Render for StatementCard {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let policy = ctx.policy;
        let theme = &ctx.theme;
        let body_width = policy.width.saturating_sub(2);

        let subject = theme.paint(Token::Accent, &self.subject_canonical, policy);
        let predicate = theme.paint(Token::Predicate, &self.predicate_qname, policy);
        let object = match &self.object {
            ObjectRef::Entity { name, .. } => theme.paint(Token::EntityId, name, policy),
            ObjectRef::Literal(s) => theme.paint(Token::Value, s, policy),
        };
        let kind = theme.paint(Token::Muted, &self.kind, policy);
        writeln!(w, "[{kind}] {subject} {predicate} {object}")?;

        let id_painted = theme.paint(Token::StatementId, &self.id, policy);
        let conf_str = format!("{:.2}", self.confidence);
        let conf = theme.paint(Token::Confidence, &conf_str, policy);
        writeln!(w, "  id = {id_painted}   conf = {conf}")?;
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
            "object": object,
            "confidence": self.confidence,
            "evidence_memories": self.evidence_memories.iter().map(|m| json!({
                "memory_id": m.short_id,
                "text": m.text,
            })).collect::<Vec<_>>(),
            // Bare label form for tools that only want the right-hand side.
            "object_label": self.object.label(),
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
}
