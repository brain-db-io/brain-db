//! Semantic color tokens.
//!
//! Renderers ask the theme to paint a span by its *role* (this is a label,
//! this is an entity id, this is a confidence number) rather than by an
//! ANSI color. Swapping the palette later — for accessibility, light mode,
//! a confidence gradient — touches one file, not every renderer.

/// Role classification for a piece of rendered text.
///
/// New roles are added only when a renderer needs to distinguish a span
/// the existing roles can't carry. Don't reach for a generic `Color`
/// enum — that's how palettes turn into ANSI grab-bags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Token {
    /// A column header, key in a key/value row, or other "this names a
    /// thing" span. Subdued — the value is what the eye should land on.
    Label,
    /// The actual datum being shown. Default foreground; the baseline.
    Value,
    /// Subtle, deprioritised text (separators, hints, "n/a", units).
    Muted,
    /// Highlight color for emphasis — section titles, focused rows.
    Accent,

    /// Error message text or an error-state cell.
    Error,
    /// Warning — a degraded but recoverable state.
    Warn,
    /// Success — an operation completed cleanly.
    Success,
    /// Informational status text.
    Info,

    /// A confidence value (0.0–1.0). Today a flat color; a follow-up may
    /// map it to a gradient.
    Confidence,
    /// A retrieval score (raw or fused).
    Score,
    /// A relation predicate (e.g. `works_at`, `prefers`).
    Predicate,

    /// An entity identifier in any of its surface forms.
    EntityId,
    /// A memory identifier (the `s{shard}/m{slot}/v{version}` form).
    MemoryId,
    /// A statement identifier.
    StatementId,
}
