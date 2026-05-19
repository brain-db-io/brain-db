//! Semantic theme: tokens, palette, and the painter that joins them.

pub mod palette;
pub mod token;

pub use palette::Palette;
pub use token::Token;

use std::borrow::Cow;

use owo_colors::OwoColorize;

use crate::term::policy::TermPolicy;

/// The theme paints renderer text by its semantic [`Token`].
///
/// Today there's one palette (dark-mode-first); the struct exists so a
/// future config-driven theme drops in without rippling through every
/// call site.
#[derive(Debug, Default, Clone, Copy)]
pub struct Theme {
    pub palette: Palette,
}

impl Theme {
    /// Construct a theme from an explicit palette.
    #[must_use]
    pub const fn with_palette(palette: Palette) -> Self {
        Self { palette }
    }

    /// Paint `text` with the color the palette assigns to `token`.
    ///
    /// When `policy.color` is `false` (NO_COLOR, `--color=never`, stdout
    /// not a TTY in auto mode, …) this is the identity function and
    /// allocates nothing — call sites can route every span through here
    /// unconditionally.
    #[must_use]
    pub fn paint<'a>(&self, token: Token, text: &'a str, policy: TermPolicy) -> Cow<'a, str> {
        if !policy.color {
            return Cow::Borrowed(text);
        }
        let color = match token {
            Token::Label => self.palette.label,
            Token::Value => self.palette.value,
            Token::Muted => self.palette.muted,
            Token::Accent => self.palette.accent,
            Token::Error => self.palette.error,
            Token::Warn => self.palette.warn,
            Token::Success => self.palette.success,
            Token::Info => self.palette.info,
            Token::Confidence => self.palette.confidence,
            Token::Score => self.palette.score,
            Token::Predicate => self.palette.predicate,
            Token::EntityId => self.palette.entity_id,
            Token::MemoryId => self.palette.memory_id,
            Token::StatementId => self.palette.statement_id,
        };
        Cow::Owned(text.color(color).to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paint_is_identity_when_color_disabled() {
        let theme = Theme::default();
        let policy = TermPolicy::plain(); // color = false
                                          // Every token must round-trip unchanged when color is off.
        for token in [
            Token::Label,
            Token::Value,
            Token::Muted,
            Token::Accent,
            Token::Error,
            Token::Warn,
            Token::Success,
            Token::Info,
            Token::Confidence,
            Token::Score,
            Token::Predicate,
            Token::EntityId,
            Token::MemoryId,
            Token::StatementId,
        ] {
            let out = theme.paint(token, "hello", policy);
            assert_eq!(out.as_ref(), "hello", "token {token:?} mutated text");
            assert!(
                matches!(out, Cow::Borrowed(_)),
                "token {token:?} allocated under no-color"
            );
        }
    }

    #[test]
    fn paint_wraps_when_color_enabled() {
        let theme = Theme::default();
        let mut policy = TermPolicy::plain();
        policy.color = true;
        let out = theme.paint(Token::Accent, "hi", policy);
        assert!(out.contains("hi"));
        // Some ANSI escape must be present.
        assert!(out.contains('\x1b'));
    }
}
