//! Accuracy harness for `classify_statement_kind_pattern`.
//!
//! The function is the cheap pre-filter that keeps the LLM tier off
//! the common Preference / Event cases. Its accuracy bar is the
//! macro-F1 across a fixed labelled corpus — below the bar, the
//! pipeline still runs but loses the cost-moat advantage; above it,
//! the LLM tier sees only the ambiguous tail.
//!
//! The corpus is deliberately small and synthetic — common templates
//! per kind, with intentional confusables (preference-of-someone-else
//! that should read as Fact, year-anchored Facts that aren't Events).
//! Real production text will skew differently; this harness is a
//! regression gate, not a benchmark.

use brain_core::StatementKind;
use brain_extractors::{classify_statement_kind_pattern, STATEMENT_KIND_PATTERN_THRESHOLD};

#[derive(Clone, Copy)]
struct Case {
    text: &'static str,
    truth: StatementKind,
}

/// Pre-classifier corpus. ~200 cases evenly split across the three
/// kinds (~66-70 each) plus confusables.
fn corpus() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();

    // ---- Fact ----------------------------------------------------------
    let facts: &[&str] = &[
        "Alice works at Acme Corp.",
        "Bob lives in Berlin.",
        "Priya leads the platform team.",
        "Carol founded Acme in Seattle.",
        "Dan owns the Phoenix project.",
        "Eve runs the engineering org.",
        "Frank manages the data team.",
        "Grace co-founded the company.",
        "Harry reports to the CTO.",
        "Ivy belongs to the security guild.",
        "The capital of France is Paris.",
        "Acme has 200 employees.",
        "The repository contains 10 services.",
        "The board consists of seven members.",
        "Our product includes a free tier.",
        "Berlin is the capital of Germany.",
        "Python is a programming language.",
        "Acme was founded in 2014.",
        "Carol works for Acme.",
        "Dan works on the planner team.",
        "Eve lives at 5th Avenue.",
        "Frank lives on Main Street.",
        "Grace runs the design org.",
        "Harry has three direct reports.",
        "The CTO has a corner office.",
        "Priya was the first hire.",
        "The data lake is in us-west-2.",
        "Acme has offices in Berlin.",
        "Alice has worked at Acme since 2018.",
        "The team is based in Seattle.",
        "Bob is the new VP of engineering.",
        "Carol is responsible for procurement.",
        "Dan is in charge of release engineering.",
        "Our SDK is open source.",
        "The CI pipeline is hosted on GitHub.",
        "The contract is signed.",
        "The proposal is under review.",
        "Acme is a B2B SaaS company.",
        "Acme has raised 50 million dollars.",
        "Grace has been promoted.",
        "Harry was promoted to staff engineer.",
        "Ivy was hired last quarter.",
        "Frank has a PhD in computer science.",
        "Priya has experience with distributed systems.",
        "The product has a free trial.",
        "The API has a rate limit.",
        "The library has no external dependencies.",
        "Bob owns the on-call rotation.",
        "Carol owns the security review process.",
        "Dan manages the budget.",
        "Acme's HQ is in San Francisco.",
        "Eve is the on-call engineer this week.",
        "Frank leads the platform guild.",
        "Grace founded the diversity council.",
        "The team has a remote-first policy.",
        "Our database is PostgreSQL.",
        "The infra is hosted on AWS.",
        "Acme's revenue has grown 30 percent.",
        "Harry has shipped four features.",
        "Ivy reports to Carol.",
        "The release manager is Frank.",
        "The product owner is Grace.",
        "Acme's main competitor is GlobalCo.",
        "The roadmap includes a mobile client.",
        "The Q3 plan includes hiring two engineers.",
        "The vision document is on Notion.",
        "Our customer base includes Fortune 500 firms.",
        "The architecture diagram is in the wiki.",
    ];
    cases.extend(facts.iter().map(|t| Case {
        text: t,
        truth: StatementKind::Fact,
    }));

    // ---- Preference ----------------------------------------------------
    let prefs: &[&str] = &[
        "I prefer dark roast coffee.",
        "I prefer async meetings.",
        "I prefer mornings.",
        "I'd prefer to skip the meeting.",
        "I would prefer to work remote.",
        "I like async standups.",
        "I like Helix as my editor.",
        "I love this team.",
        "I love working on the planner.",
        "I love clean APIs.",
        "I hate flaky tests.",
        "I hate noisy alerts.",
        "I dislike long code reviews.",
        "I don't like context switching.",
        "I do not like surprise releases.",
        "I don't want to be on-call this week.",
        "I do not want to chase metrics.",
        "I want a quiet sprint.",
        "I want to focus on infra.",
        "I wish we had better tooling.",
        "I'd rather pair on the design.",
        "I would rather take notes by hand.",
        "I enjoy debugging tricky issues.",
        "I enjoy mentoring juniors.",
        "I can't stand long meetings.",
        "I cannot stand half-baked PRs.",
        "My favorite editor is helix.",
        "My favorite IDE is jetbrains.",
        "My favourite language is Rust.",
        "My preference is async-first.",
        "I prefer the staging environment.",
        "I like the new dashboard.",
        "I love how clean this API is.",
        "I hate the legacy build system.",
        "I dislike YAML configuration.",
        "I prefer typed languages.",
        "I prefer monorepos for small teams.",
        "I like writing integration tests.",
        "I love using property tests.",
        "I hate writing boilerplate.",
        "I prefer to work from home on Mondays.",
        "I'd prefer not to interrupt the sprint.",
        "I would prefer a smaller meeting.",
        "I love the kubectl plugin we built.",
        "I like the latency of the new shard.",
        "I prefer to ship in small increments.",
        "I'd rather refactor before adding features.",
        "I want async-first communication.",
        "I prefer working in the morning.",
        "I love clean commit history.",
        "I hate force pushes to main.",
        "I prefer feature flags over branches.",
        "I love the new feedback loop.",
        "I like keyboard-first interfaces.",
        "I hate mouse-heavy workflows.",
        "I prefer dark mode.",
        "I love compact UIs.",
        "I dislike modal dialogs.",
        "I do not want hard deadlines.",
        "I prefer pair programming.",
        "I want to learn distributed systems.",
        "I love how Rust handles errors.",
        "I prefer Tokio for HTTP clients.",
        "I hate building on macOS.",
        "I love the new release cadence.",
        "I prefer postgres over MySQL.",
    ];
    cases.extend(prefs.iter().map(|t| Case {
        text: t,
        truth: StatementKind::Preference,
    }));

    // ---- Event ---------------------------------------------------------
    let events: &[&str] = &[
        "The all-hands is Friday at 10am.",
        "The standup is at 9:30am.",
        "The release is scheduled for 2026-06-15.",
        "The deploy happened at 15:00.",
        "Our demo took place on Tuesday.",
        "The kickoff is on Monday.",
        "The interview is at 2pm.",
        "The summit is in October.",
        "The launch is scheduled on 2026-07-01.",
        "The workshop happened yesterday.",
        "The conference is next week.",
        "The deadline is Friday.",
        "The review meeting is at 4pm.",
        "Our sync is scheduled for Wednesday at 11am.",
        "The migration occurred on 2025-12-01.",
        "The outage happened on Sunday.",
        "The incident occurred at 3:14am.",
        "The release is at 5pm.",
        "The deploy is scheduled for tomorrow.",
        "The 1:1 is at 11am.",
        "The one-on-one is at 2pm tomorrow.",
        "The team retro is on Friday.",
        "Our anniversary is on June 12.",
        "Frank's birthday is on July 4.",
        "The wedding took place last Saturday.",
        "The flight is at 8:45am.",
        "The trip is scheduled for next month.",
        "The visit happened in March.",
        "The all-hands occurred on Friday.",
        "The release is on 2026-08-22.",
        "The launch is scheduled at noon.",
        "The keynote is at 10am.",
        "The Q3 review is on October 5.",
        "The hackathon is next weekend.",
        "The retrospective is at 4pm Friday.",
        "The training is scheduled on 2026-09-01.",
        "The migration is on Saturday at 3am.",
        "The deploy is on Tuesday at 6pm.",
        "The outage occurred at 13:42.",
        "The release happened yesterday.",
        "The interview occurred last Monday.",
        "The conference is in 2026.",
        "The standup occurred at 9:15.",
        "The product review is at 1pm.",
        "The architecture review is on Thursday.",
        "The deploy is at 11pm.",
        "The release window opens at 2am.",
        "The maintenance window starts at 1am.",
        "The maintenance ends at 3am.",
        "The kickoff happened last Friday.",
        "The launch happened on 2026-04-22.",
        "The all-hands is on Friday at 11am.",
        "The meeting is at 2:30pm.",
        "Our weekly sync is at 10am every Monday.",
        "The release ceremony is at 5pm.",
        "The outage incident occurred yesterday.",
        "The deployment ended at 4:00.",
        "The summit happened in September.",
        "The interview is scheduled for tomorrow at 10am.",
        "The 1:1 is tomorrow at 3pm.",
        "The annual review is in December.",
        "The demo is on Wednesday afternoon.",
        "The release is on June 30 at 5pm.",
        "The deploy is scheduled at 23:00.",
        "Our sprint review is on Friday.",
        "The team off-site is in November.",
        "The migration deployment occurred last week.",
        "The launch took place this morning.",
    ];
    cases.extend(events.iter().map(|t| Case {
        text: t,
        truth: StatementKind::Event,
    }));

    cases
}

#[derive(Default, Debug, Clone, Copy)]
struct PerClass {
    tp: u32,
    fp: u32,
    fn_: u32,
}

impl PerClass {
    fn precision(&self) -> f32 {
        let denom = self.tp + self.fp;
        if denom == 0 {
            0.0
        } else {
            self.tp as f32 / denom as f32
        }
    }
    fn recall(&self) -> f32 {
        let denom = self.tp + self.fn_;
        if denom == 0 {
            0.0
        } else {
            self.tp as f32 / denom as f32
        }
    }
    fn f1(&self) -> f32 {
        let (p, r) = (self.precision(), self.recall());
        if p + r == 0.0 {
            0.0
        } else {
            2.0 * p * r / (p + r)
        }
    }
}

fn class_idx(k: StatementKind) -> usize {
    match k {
        StatementKind::Fact => 0,
        StatementKind::Preference => 1,
        StatementKind::Event => 2,
    }
}

fn class_name(k: StatementKind) -> &'static str {
    match k {
        StatementKind::Fact => "Fact",
        StatementKind::Preference => "Preference",
        StatementKind::Event => "Event",
    }
}

#[test]
fn classify_statement_kind_pattern_meets_macro_f1_target() {
    let cases = corpus();
    assert!(cases.len() >= 150, "corpus too small: {}", cases.len());

    let mut per_class = [PerClass::default(); 3];
    let mut no_decision = 0u32;

    for case in &cases {
        let got = classify_statement_kind_pattern(case.text);
        let truth = case.truth;
        match got {
            Some((pred, conf)) if conf >= STATEMENT_KIND_PATTERN_THRESHOLD => {
                if pred == truth {
                    per_class[class_idx(truth)].tp += 1;
                } else {
                    per_class[class_idx(pred)].fp += 1;
                    per_class[class_idx(truth)].fn_ += 1;
                }
            }
            _ => {
                no_decision += 1;
                per_class[class_idx(truth)].fn_ += 1;
            }
        }
    }

    let macro_f1: f32 = (0..3).map(|i| per_class[i].f1()).sum::<f32>() / 3.0;
    let total = cases.len() as u32;
    let coverage = (total - no_decision) as f32 / total as f32;

    println!("statement-kind pattern classifier accuracy");
    println!("  total cases       : {total}");
    println!(
        "  no-decision cases : {no_decision} ({:.1}%)",
        100.0 * (1.0 - coverage)
    );
    println!("  coverage          : {:.3}", coverage);
    for k in [
        StatementKind::Fact,
        StatementKind::Preference,
        StatementKind::Event,
    ] {
        let pc = per_class[class_idx(k)];
        println!(
            "  {:<10}  tp={:>3}  fp={:>3}  fn={:>3}  P={:.3}  R={:.3}  F1={:.3}",
            class_name(k),
            pc.tp,
            pc.fp,
            pc.fn_,
            pc.precision(),
            pc.recall(),
            pc.f1()
        );
    }
    println!("  macro-F1          : {macro_f1:.3}");

    assert!(
        macro_f1 >= 0.85,
        "macro-F1 = {macro_f1:.3} below 0.85 target — pattern set regressed"
    );
}
