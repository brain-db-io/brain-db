//! Stacked card for a single relation's full record.
//!
//! Mirrors the [`StatementCard`](super::statement_card) shape — header,
//! id/confidence row, optional Evidence section — so a user navigating
//! the typed graph sees the same idiom across all three knowledge
//! primitives (entity, statement, relation).

use std::io::{self, Write};

use serde_json::{json, Value};

use crate::render::entity_card::MemorySummary;
use crate::table::middle_truncate;
use crate::term::hyperlink::link;
use crate::theme::Token;
use crate::{Render, RenderCtx};

/// Sided endpoint of a typed relation.
#[derive(Debug, Clone)]
pub struct EntityRef {
    pub id: String,
    pub name: String,
}

pub struct RelationCard {
    pub id: String,
    pub predicate_qname: String,
    pub from: EntityRef,
    pub to: EntityRef,
    pub confidence: f32,
    pub evidence_memories: Vec<MemorySummary>,
}

impl Render for RelationCard {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let policy = ctx.policy;
        let theme = &ctx.theme;
        let body_width = policy.width.saturating_sub(2);

        let from = theme.paint(Token::EntityId, &self.from.name, policy);
        let to = theme.paint(Token::EntityId, &self.to.name, policy);
        let predicate = theme.paint(Token::Predicate, &self.predicate_qname, policy);
        writeln!(w, "{from} --[{predicate}]--> {to}")?;

        let id_painted = theme.paint(Token::EntityId, &self.id, policy);
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
        json!({
            "id": self.id,
            "predicate": self.predicate_qname,
            "from": {"id": self.from.id, "name": self.from.name},
            "to": {"id": self.to.id, "name": self.to.name},
            "confidence": self.confidence,
            "evidence_memories": self.evidence_memories.iter().map(|m| json!({
                "memory_id": m.short_id,
                "text": m.text,
            })).collect::<Vec<_>>(),
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

    fn sample() -> RelationCard {
        RelationCard {
            id: "rel_1".into(),
            predicate_qname: "works_at".into(),
            from: EntityRef {
                id: "ent_p".into(),
                name: "Priya".into(),
            },
            to: EntityRef {
                id: "ent_a".into(),
                name: "Acme".into(),
            },
            confidence: 0.95,
            evidence_memories: vec![MemorySummary {
                short_id: "s2/m17/v1".into(),
                text: "Priya works at Acme".into(),
            }],
        }
    }

    #[test]
    fn table_shows_endpoints_predicate_and_evidence() {
        let mut buf = Vec::new();
        sample().render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("Priya"));
        assert!(s.contains("Acme"));
        assert!(s.contains("works_at"));
        assert!(s.contains("Evidence"));
    }

    #[test]
    fn json_carries_both_endpoints() {
        let v = sample().render_json(&ctx());
        assert_eq!(v["from"]["name"], "Priya");
        assert_eq!(v["to"]["name"], "Acme");
        assert_eq!(v["predicate"], "works_at");
    }
}
