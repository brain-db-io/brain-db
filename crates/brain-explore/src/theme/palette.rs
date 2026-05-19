//! Default palette — dark-mode-first.
//!
//! Each [`Token`](super::Token) maps to one [`AnsiColors`] value. We stick
//! to the 8-color ANSI set (plus the Bright variants) so output looks
//! sensible on every terminal a user could plausibly be on, including
//! ones the user has retinted with a custom scheme. RGB / 256-color
//! escapes look prettier on a fresh terminal and worse everywhere else.

use owo_colors::AnsiColors;

/// Maps every [`Token`](super::Token) variant to an ANSI color.
///
/// Construct with [`Palette::dark`] (the default) or via `Default`. Custom
/// palettes ship later behind a config file — at v1 the palette is a
/// hard-coded sensible default.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub label: AnsiColors,
    pub value: AnsiColors,
    pub muted: AnsiColors,
    pub accent: AnsiColors,
    pub error: AnsiColors,
    pub warn: AnsiColors,
    pub success: AnsiColors,
    pub info: AnsiColors,
    pub confidence: AnsiColors,
    pub score: AnsiColors,
    pub predicate: AnsiColors,
    pub entity_id: AnsiColors,
    pub memory_id: AnsiColors,
    pub statement_id: AnsiColors,
}

impl Palette {
    /// The default dark-mode palette.
    ///
    /// Chosen to be readable on the most common dark terminal schemes
    /// (Solarized Dark, One Dark, Dracula, default Terminal.app / iTerm2)
    /// without sacrificing too much contrast on a stock black background.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            // Labels live next to values; cyan keeps them legible without
            // pulling the eye away from the actual datum.
            label: AnsiColors::Cyan,
            value: AnsiColors::Default,
            // Default-bright-black ("gray") is the canonical muted color
            // on almost every dark scheme.
            muted: AnsiColors::BrightBlack,
            accent: AnsiColors::BrightCyan,

            error: AnsiColors::Red,
            warn: AnsiColors::Yellow,
            success: AnsiColors::Green,
            info: AnsiColors::Blue,

            // Confidence + score look related; both green-ish today.
            // Distinct from `success` so they don't read as "this is OK"
            // — they're numeric quantities, not status.
            confidence: AnsiColors::BrightGreen,
            score: AnsiColors::BrightGreen,
            // Predicates are the verb of a statement — visually distinct
            // from the noun (entity).
            predicate: AnsiColors::Magenta,

            entity_id: AnsiColors::Yellow,
            memory_id: AnsiColors::BrightYellow,
            statement_id: AnsiColors::BrightMagenta,
        }
    }
}

impl Default for Palette {
    fn default() -> Self {
        Self::dark()
    }
}
