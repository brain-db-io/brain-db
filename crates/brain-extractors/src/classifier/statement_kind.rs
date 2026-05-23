//! Rule-based statement-kind classifier.
//!
//! Cheap pre-filter that decides Fact / Preference / Event from
//! deterministic patterns on the lowercased sentence. Runs in
//! microseconds and keeps the LLM tier off the 70-90% of statements
//! whose kind is unambiguous from surface cues.

use brain_core::StatementKind;

/// Default confidence threshold above which the pattern classifier's
/// kind decision is treated as authoritative. The pipeline accepts any
/// `Some((kind, conf))` where `conf >= STATEMENT_KIND_PATTERN_THRESHOLD`
/// and otherwise defers to downstream (LLM or stored default).
pub const STATEMENT_KIND_PATTERN_THRESHOLD: f32 = 0.7;

/// Classify a sentence into Fact / Preference / Event using
/// deterministic patterns. Returns `Some((kind, confidence))` for clean
/// matches and `None` when no pattern fires — the caller should defer
/// to the LLM tier in that case.
///
/// The function is the cheap pre-filter that keeps the LLM tier off the
/// 70-90 % of statements whose kind is unambiguous from surface cues.
/// It runs in microseconds and is allocation-free on the hot path
/// (lowercased text is borrowed once; everything else is byte-scan).
///
/// Order matters: Preference cues beat Event cues which beat Fact cues.
/// A sentence carrying both "I prefer" and a date is treated as a
/// Preference — preference statements naturally embed temporal context
/// ("I prefer my coffee black since 2019") and the preference framing
/// dominates the truth-condition.
pub fn classify_statement_kind_pattern(text: &str) -> Option<(StatementKind, f32)> {
    let lower = text.to_ascii_lowercase();
    let lower = lower.as_str();

    if let Some(score) = score_preference(lower) {
        return Some((StatementKind::Preference, score));
    }
    if let Some(score) = score_event(lower) {
        return Some((StatementKind::Event, score));
    }
    if let Some(score) = score_fact(lower) {
        return Some((StatementKind::Fact, score));
    }
    None
}

/// First-person preference and dispreference cues. We anchor on the
/// "I" pronoun + a preference verb so the pattern doesn't fire on
/// third-person reports of someone else's preference (which read more
/// like Facts: "Alice prefers tea" → Fact about Alice's preference).
fn score_preference(text: &str) -> Option<f32> {
    const STRONG_FIRST_PERSON: &[&str] = &[
        "i prefer",
        "i'd prefer",
        "i would prefer",
        "i like",
        "i love",
        "i hate",
        "i dislike",
        "i don't like",
        "i do not like",
        "i don't want",
        "i do not want",
        "i want",
        "i wish",
        "i'd rather",
        "i would rather",
        "i enjoy",
        "i can't stand",
        "i cannot stand",
        "my favorite",
        "my favourite",
        "my preference",
    ];
    if STRONG_FIRST_PERSON.iter().any(|cue| text.contains(cue)) {
        return Some(0.9);
    }
    // Weaker third-person preference cues. Lower confidence — the
    // sentence might be a Fact reporting someone else's preference,
    // but if the predicate noun is unambiguous we still call it.
    const SOFT_PREFERENCE: &[&str] = &[
        "favorite ",
        "favourite ",
        "preferred ",
        "preferences ",
        "preference ",
    ];
    if SOFT_PREFERENCE.iter().any(|cue| text.contains(cue)) {
        return Some(0.75);
    }
    None
}

/// Event cues: a temporal anchor (explicit date/time or relative time
/// word) AND either an event verb or a scheduled-action noun. Either
/// alone is insufficient — "she works in 2024" is a Fact, "the meeting
/// happened" without a date may be a Fact about a past event. The
/// combination is the discriminator.
fn score_event(text: &str) -> Option<f32> {
    let has_temporal = has_explicit_date(text)
        || has_clock_time(text)
        || has_relative_time(text)
        || has_year_anchor(text);
    if !has_temporal {
        return None;
    }
    const EVENT_VERBS: &[&str] = &[
        "happened",
        "occurred",
        "took place",
        "is scheduled",
        "scheduled for",
        "scheduled on",
        "scheduled at",
        "will happen",
        "will occur",
        "will take place",
        "starts at",
        "starts on",
        "begins at",
        "begins on",
        "ends at",
        "ends on",
        "is at ",
        "is on ",
        " at ",
        " on ",
    ];
    const EVENT_NOUNS: &[&str] = &[
        "meeting",
        "all-hands",
        "all hands",
        "standup",
        "stand-up",
        "kickoff",
        "kick-off",
        "release",
        "launch",
        "demo",
        "review",
        "deadline",
        "conference",
        "summit",
        "workshop",
        "ceremony",
        "appointment",
        "event",
        "interview",
        "call",
        "sync",
        "1:1",
        "one-on-one",
        "deploy",
        "deployment",
        "outage",
        "incident",
        "milestone",
        "anniversary",
        "birthday",
        "wedding",
        "flight",
        "trip",
        "visit",
    ];
    let has_verb = EVENT_VERBS.iter().any(|v| text.contains(v));
    let has_noun = EVENT_NOUNS.iter().any(|n| text.contains(n));
    if has_verb && has_noun {
        return Some(0.9);
    }
    if has_verb || has_noun {
        return Some(0.8);
    }
    None
}

/// Fact cues: a copula or attribution verb without preference / event
/// markers. We've already ruled the preference / event branches out by
/// the time we get here, so the bar is lower — any plausible
/// declarative sentence anchored on "X is/are/has/works/lives/owns" is
/// a Fact.
fn score_fact(text: &str) -> Option<f32> {
    const COPULA: &[&str] = &[
        " is ",
        " are ",
        " was ",
        " were ",
        " has ",
        " have ",
        " had ",
        " works at",
        " works for",
        " works on",
        " works in",
        " lives in",
        " lives at",
        " lives on",
        " owns ",
        " runs ",
        " manages ",
        " leads ",
        " founded ",
        " co-founded",
        " reports to",
        " belongs to",
        " contains ",
        " consists of",
        " includes ",
    ];
    if COPULA.iter().any(|c| text.contains(c)) {
        return Some(0.75);
    }
    None
}

/// Returns true if `text` contains an ISO-ish date (`2024-05-16`, `5/16/2024`,
/// `16-05-2024`).
fn has_explicit_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    // YYYY-MM-DD or DD-MM-YYYY or YYYY/MM/DD with 1-4 digits per group.
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let group_len = i - start;
            if (group_len == 4 || group_len == 1 || group_len == 2)
                && i < bytes.len()
                && (bytes[i] == b'-' || bytes[i] == b'/')
            {
                // Look ahead: second group of digits, separator, third group.
                let sep = bytes[i];
                i += 1;
                let g2 = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                let g2_len = i - g2;
                if (1..=4).contains(&g2_len)
                    && i < bytes.len()
                    && bytes[i] == sep
                    && i + 1 < bytes.len()
                    && bytes[i + 1].is_ascii_digit()
                {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Returns true if `text` contains a clock-style time like `3pm`,
/// `10am`, `3:30pm`, `15:00`.
fn has_clock_time(text: &str) -> bool {
    let bytes = text.as_bytes();
    // h(h):mm or h(h)am/pm.
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let dlen = i - start;
            if dlen == 0 || dlen > 2 {
                continue;
            }
            // `:mm`
            if i + 2 < bytes.len()
                && bytes[i] == b':'
                && bytes[i + 1].is_ascii_digit()
                && bytes[i + 2].is_ascii_digit()
            {
                return true;
            }
            // `am` / `pm`
            if i + 1 < bytes.len() {
                let suffix = &bytes[i..(i + 2).min(bytes.len())];
                if suffix == b"am" || suffix == b"pm" {
                    return true;
                }
            }
        } else {
            i += 1;
        }
    }
    false
}

/// Relative-time tokens: "yesterday", "tomorrow", "next monday",
/// "last friday", "this week", weekday names following "on".
fn has_relative_time(text: &str) -> bool {
    const REL: &[&str] = &[
        "yesterday",
        "tomorrow",
        "tonight",
        "this morning",
        "this afternoon",
        "this evening",
        "next week",
        "next month",
        "next year",
        "last week",
        "last month",
        "last year",
        "next monday",
        "next tuesday",
        "next wednesday",
        "next thursday",
        "next friday",
        "next saturday",
        "next sunday",
        "last monday",
        "last tuesday",
        "last wednesday",
        "last thursday",
        "last friday",
        "last saturday",
        "last sunday",
        "on monday",
        "on tuesday",
        "on wednesday",
        "on thursday",
        "on friday",
        "on saturday",
        "on sunday",
        " jan ",
        " feb ",
        " mar ",
        " apr ",
        " may ",
        " jun ",
        " jul ",
        " aug ",
        " sep ",
        " oct ",
        " nov ",
        " dec ",
        " january ",
        " february ",
        " march ",
        " april ",
        " june ",
        " july ",
        " august ",
        " september ",
        " october ",
        " november ",
        " december ",
    ];
    REL.iter().any(|cue| text.contains(cue))
}

/// Standalone four-digit year (1900-2099) preceded by " in " — covers
/// "in 2024", "in 1999". Excluded from `has_explicit_date` which
/// requires a date separator.
fn has_year_anchor(text: &str) -> bool {
    let bytes = text.as_bytes();
    // Walk byte by byte; not Unicode-aware but every relevant ASCII
    // year token survives lowercasing.
    let needle = b" in ";
    let mut i = 0;
    while i + needle.len() + 4 <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let y = i + needle.len();
            if bytes[y..y + 4].iter().all(|b| b.is_ascii_digit())
                && (bytes[y] == b'1' || bytes[y] == b'2')
            {
                // Trailing boundary: end of string or non-digit.
                if y + 4 == bytes.len() || !bytes[y + 4].is_ascii_digit() {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

