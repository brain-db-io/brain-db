//! Extract / audit status card.
//!
//! Renders the per-memory result of the extraction pipeline (pattern →
//! classifier → LLM tiers) so an operator can see which tier produced
//! which artifacts and what the LLM spend was. The card is the
//! user-facing twin of the admin audit row (which lives in brain-cli);
//! the user-facing card is keyed on `memory_id` and emphasises
//! "what came out of my ENCODE," while the admin row emphasises
//! "what is the worker queue doing right now."

use std::io::{self, Write};

use serde_json::{json, Value};

use crate::theme::Token;
use crate::{Render, RenderCtx};

/// One row in the per-tier extraction outcome list.
#[derive(Debug, Clone)]
pub struct TierOutcome {
    /// e.g. `pattern`, `classifier`, `llm`.
    pub tier: String,
    /// e.g. `accepted`, `skipped`, `failed`.
    pub outcome: String,
    /// Free-form note ("budget reached" / "no matches" / etc.).
    pub note: Option<String>,
}

pub struct AuditCard {
    pub memory_id: String,
    pub status: String,
    pub tier_outcomes: Vec<TierOutcome>,
    pub cost_micro_usd: u64,
    pub entities_created: u32,
    pub statements_created: u32,
    pub relations_created: u32,
}

impl Render for AuditCard {
    fn render_table(&self, ctx: &RenderCtx, w: &mut dyn Write) -> io::Result<()> {
        let policy = ctx.policy;
        let theme = &ctx.theme;

        let memory_painted = theme.paint(Token::MemoryId, &self.memory_id, policy);
        let status_token = match self.status.as_str() {
            "ok" | "complete" | "completed" | "success" => Token::Success,
            "failed" | "error" => Token::Error,
            _ => Token::Info,
        };
        let status_painted = theme.paint(status_token, &self.status, policy);
        writeln!(w, "{memory_painted}  status={status_painted}")?;
        writeln!(w)?;

        if !self.tier_outcomes.is_empty() {
            let heading = theme.paint(Token::Label, "Tiers", policy);
            writeln!(w, "{heading}")?;
            for t in &self.tier_outcomes {
                let tier = theme.paint(Token::Accent, &t.tier, policy);
                let outcome = match t.outcome.as_str() {
                    "accepted" | "ok" => theme.paint(Token::Success, &t.outcome, policy),
                    "failed" => theme.paint(Token::Error, &t.outcome, policy),
                    "skipped" => theme.paint(Token::Muted, &t.outcome, policy),
                    _ => theme.paint(Token::Info, &t.outcome, policy),
                };
                match &t.note {
                    Some(note) => writeln!(w, "  · {tier:12} {outcome}  ({note})")?,
                    None => writeln!(w, "  · {tier:12} {outcome}")?,
                }
            }
            writeln!(w)?;
        }

        let heading = theme.paint(Token::Label, "Created", policy);
        writeln!(w, "{heading}")?;
        writeln!(w, "  · entities    = {}", self.entities_created)?;
        writeln!(w, "  · statements  = {}", self.statements_created)?;
        writeln!(w, "  · relations   = {}", self.relations_created)?;
        writeln!(w)?;

        let cost_label = theme.paint(Token::Label, "Cost", policy);
        writeln!(
            w,
            "{cost_label} = ${:.6}",
            self.cost_micro_usd as f64 / 1_000_000.0
        )
    }

    fn render_json(&self, _ctx: &RenderCtx) -> Value {
        json!({
            "memory_id": self.memory_id,
            "status": self.status,
            "tier_outcomes": self.tier_outcomes.iter().map(|t| json!({
                "tier": t.tier,
                "outcome": t.outcome,
                "note": t.note,
            })).collect::<Vec<_>>(),
            "cost_micro_usd": self.cost_micro_usd,
            "entities_created": self.entities_created,
            "statements_created": self.statements_created,
            "relations_created": self.relations_created,
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

    fn sample() -> AuditCard {
        AuditCard {
            memory_id: "s0/m1/v1".into(),
            status: "complete".into(),
            tier_outcomes: vec![
                TierOutcome {
                    tier: "pattern".into(),
                    outcome: "accepted".into(),
                    note: None,
                },
                TierOutcome {
                    tier: "llm".into(),
                    outcome: "skipped".into(),
                    note: Some("budget reached".into()),
                },
            ],
            cost_micro_usd: 1234,
            entities_created: 2,
            statements_created: 3,
            relations_created: 1,
        }
    }

    #[test]
    fn table_shows_status_tiers_and_counts() {
        let mut buf = Vec::new();
        sample().render_table(&ctx(), &mut buf).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("s0/m1/v1"));
        assert!(s.contains("complete"));
        assert!(s.contains("pattern"));
        assert!(s.contains("budget reached"));
        assert!(s.contains("entities    = 2"));
    }

    #[test]
    fn json_carries_every_counter() {
        let v = sample().render_json(&ctx());
        assert_eq!(v["entities_created"], 2);
        assert_eq!(v["statements_created"], 3);
        assert_eq!(v["relations_created"], 1);
        assert_eq!(v["cost_micro_usd"], 1234);
        assert_eq!(v["tier_outcomes"][1]["outcome"], "skipped");
    }
}
