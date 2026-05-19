//! Flyctl-style stacked card for `entity show`.
//!
//! Sections (in order, optional): Identity · Aliases · Statements ·
//! Mentioned-in · Relations. Long lines are middle-truncated to the
//! detected terminal width. Entity ids in the Relations section and
//! memory ids in the Mentioned-in section are wrapped in OSC 8
//! hyperlinks so a click in iTerm / kitty / WezTerm navigates the
//! graph.

use std::io::{self, Write};

use serde_json::{json, Value};

use crate::table::middle_truncate;
use crate::term::hyperlink::link;
use crate::theme::Token;
use crate::{Render, RenderCtx};

/// Renderer for a single entity's full record.
pub struct EntityCard {
    pub id: String,
    pub canonical_name: String,
    pub type_qname: String,
    pub aliases: Vec<String>,
    pub statements: Vec<StatementSummary>,
    pub mentioned_in: Vec<MemorySummary>,
    pub relations_out: Vec<RelationSummary>,
    pub relations_in: Vec<RelationSummary>,
}

pub struct StatementSummary {
    pub id: String,
    pub kind: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32,
}

pub struct MemorySummary {
    pub short_id: String,
    pub text: String,
}

pub struct RelationSummary {
    pub other_id: String,
    pub other_name: String,
    pub predicate: String,
}

impl Render for EntityCard {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let policy = ctx.policy;
        let theme = &ctx.theme;
        let body_width = policy.width.saturating_sub(2);

        let name = theme.paint(Token::Accent, &self.canonical_name, policy);
        let qname = theme.paint(Token::Muted, &self.type_qname, policy);
        writeln!(w, "{name}  ({qname})")?;
        let id_painted = theme.paint(Token::EntityId, &self.id, policy);
        writeln!(w, "  id = {id_painted}")?;
        writeln!(w)?;

        if !self.aliases.is_empty() {
            let heading = theme.paint(Token::Label, "Aliases", policy);
            writeln!(w, "{heading}")?;
            for a in &self.aliases {
                writeln!(w, "  · {a}")?;
            }
            writeln!(w)?;
        }

        if !self.statements.is_empty() {
            let heading = theme.paint(Token::Label, "Statements", policy);
            writeln!(w, "{heading}")?;
            for s in &self.statements {
                let predicate = theme.paint(Token::Predicate, &s.predicate, policy);
                let conf_str = format!("{:.2}", s.confidence);
                let conf = theme.paint(Token::Confidence, &conf_str, policy);
                let sid = theme.paint(Token::StatementId, &s.id, policy);
                let line = format!(
                    "[{}] {predicate} {} (conf {conf})  id={sid}",
                    s.kind, s.object,
                );
                writeln!(w, "  · {}", middle_truncate(&line, body_width))?;
            }
            writeln!(w)?;
        }

        if !self.mentioned_in.is_empty() {
            let heading = theme.paint(Token::Label, "Mentioned in", policy);
            writeln!(w, "{heading}")?;
            for m in &self.mentioned_in {
                let label = theme.paint(Token::MemoryId, &m.short_id, policy);
                let memory_link = link(policy, &label, &format!("brain://recall/{}", m.short_id));
                let text = middle_truncate(&m.text, body_width.saturating_sub(20));
                writeln!(w, "  · {memory_link}  {text}")?;
            }
            writeln!(w)?;
        }

        if !self.relations_out.is_empty() {
            let heading = theme.paint(Token::Label, "Relations (out)", policy);
            writeln!(w, "{heading}")?;
            for r in &self.relations_out {
                let label = theme.paint(Token::EntityId, &r.other_name, policy);
                let other_link = link(policy, &label, &format!("brain://entity/{}", r.other_id));
                let pred = theme.paint(Token::Predicate, &r.predicate, policy);
                writeln!(w, "  · --[{pred}]--> {other_link}")?;
            }
            writeln!(w)?;
        }

        if !self.relations_in.is_empty() {
            let heading = theme.paint(Token::Label, "Relations (in)", policy);
            writeln!(w, "{heading}")?;
            for r in &self.relations_in {
                let label = theme.paint(Token::EntityId, &r.other_name, policy);
                let other_link = link(policy, &label, &format!("brain://entity/{}", r.other_id));
                let pred = theme.paint(Token::Predicate, &r.predicate, policy);
                writeln!(w, "  · {other_link} --[{pred}]--> ·")?;
            }
            writeln!(w)?;
        }

        Ok(())
    }

    fn render_json(&self, _ctx: &RenderCtx) -> Value {
        json!({
            "id": self.id,
            "canonical_name": self.canonical_name,
            "type": self.type_qname,
            "aliases": self.aliases,
            "statements": self.statements.iter().map(|s| json!({
                "id": s.id,
                "kind": s.kind,
                "predicate": s.predicate,
                "object": s.object,
                "confidence": s.confidence,
            })).collect::<Vec<_>>(),
            "mentioned_in": self.mentioned_in.iter().map(|m| json!({
                "memory_id": m.short_id,
                "text": m.text,
            })).collect::<Vec<_>>(),
            "relations_out": self.relations_out.iter().map(|r| json!({
                "other_id": r.other_id,
                "other_name": r.other_name,
                "predicate": r.predicate,
            })).collect::<Vec<_>>(),
            "relations_in": self.relations_in.iter().map(|r| json!({
                "other_id": r.other_id,
                "other_name": r.other_name,
                "predicate": r.predicate,
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

    fn sample_card() -> EntityCard {
        EntityCard {
            id: "ent_a1b2".into(),
            canonical_name: "Priya".into(),
            type_qname: "Person".into(),
            aliases: vec!["P.".into()],
            statements: vec![StatementSummary {
                id: "stmt_1".into(),
                kind: "Fact".into(),
                predicate: "works_at".into(),
                object: "Acme Corp".into(),
                confidence: 0.95,
            }],
            mentioned_in: vec![MemorySummary {
                short_id: "s2/m17/v1".into(),
                text: "Priya works at Acme Corp as a staff engineer".into(),
            }],
            relations_out: vec![RelationSummary {
                other_id: "ent_xy".into(),
                other_name: "Acme Corp".into(),
                predicate: "works_at".into(),
            }],
            relations_in: vec![],
        }
    }

    #[test]
    fn renders_all_sections() {
        let card = sample_card();
        let mut buf = Vec::new();
        card.render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("Priya"));
        assert!(s.contains("Aliases"));
        assert!(s.contains("Statements") && s.contains("works_at"));
        assert!(s.contains("Mentioned in") && s.contains("s2/m17/v1"));
        assert!(s.contains("Relations (out)"));
    }

    #[test]
    fn omits_empty_sections() {
        let mut card = sample_card();
        card.aliases.clear();
        card.mentioned_in.clear();
        let mut buf = Vec::new();
        card.render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(!s.contains("Aliases"));
        assert!(!s.contains("Mentioned in"));
    }

    #[test]
    fn json_contains_all_fields() {
        let v = sample_card().render_json(&ctx());
        assert_eq!(v["canonical_name"], "Priya");
        assert_eq!(v["type"], "Person");
        assert_eq!(v["statements"][0]["predicate"], "works_at");
    }
}
