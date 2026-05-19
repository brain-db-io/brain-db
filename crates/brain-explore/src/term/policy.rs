//! [`TermPolicy`] — the capability bag every renderer consults.
//!
//! Built once at command dispatch, then passed by value into the
//! renderers so they don't reach into process state on the hot path.
//! Also home to the `--color` / `--hyperlinks` mode enums: they live
//! here (and not in each consumer's clap layer) so brain-shell and
//! brain-cli see *one* set of variants for "auto / always / never".

use std::io::{self, IsTerminal};

use serde::{Deserialize, Serialize};

use super::detect::{detect_terminal_size, should_use_color, should_use_hyperlinks};

/// User-supplied `--color` mode.
///
/// The CLI flag parsing stays in each consumer's clap layer, but the
/// enum lives in `brain-explore` so the resolved-vs-requested logic
/// has a single home.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMode {
    /// Honor NO_COLOR / CLICOLOR / isatty.
    #[default]
    Auto,
    /// Force color on, even when piped.
    Always,
    /// Force color off, even on a TTY.
    Never,
}

/// User-supplied `--hyperlinks` mode.
///
/// `auto` consults `supports-hyperlinks` so e.g. iTerm2 / kitty / WezTerm
/// get OSC 8 sequences while older or unknown terminals fall back to
/// plain text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum HyperlinkMode {
    #[default]
    Auto,
    Always,
    Never,
}

/// The resolved-capabilities bundle.
///
/// Field-level rather than method-level so a renderer can destructure
/// what it needs and the compiler keeps everyone honest as new fields
/// arrive.
#[derive(Debug, Clone, Copy)]
pub struct TermPolicy {
    pub color: bool,
    pub hyperlinks: bool,
    pub width: usize,
    pub height: usize,
    pub stdout_is_tty: bool,
}

impl TermPolicy {
    /// Probe the current process and reconcile against the user's
    /// `--color` / `--hyperlinks` global flags.
    #[must_use]
    pub fn detect(color: ColorMode, hyperlinks: HyperlinkMode) -> Self {
        let stdout_is_tty = io::stdout().is_terminal();
        let (width, height) = detect_terminal_size();
        Self {
            color: should_use_color(color, stdout_is_tty),
            hyperlinks: should_use_hyperlinks(hyperlinks, stdout_is_tty),
            width,
            height,
            stdout_is_tty,
        }
    }

    /// Convenience for tests: a deterministic policy that asks for no
    /// color, no hyperlinks, 80×24, not-a-TTY.
    #[must_use]
    pub fn plain() -> Self {
        Self {
            color: false,
            hyperlinks: false,
            width: 80,
            height: 24,
            stdout_is_tty: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::detect::should_use_color;
    use super::*;
    use std::env;

    /// Save/restore env vars so concurrent tests don't poison each other.
    /// Test order is otherwise unstable.
    fn with_env<F: FnOnce()>(set: &[(&str, Option<&str>)], f: F) {
        let saved: Vec<(String, Option<String>)> = set
            .iter()
            .map(|(k, _)| ((*k).to_string(), env::var(k).ok()))
            .collect();
        for (k, v) in set {
            match v {
                Some(value) => env::set_var(k, value),
                None => env::remove_var(k),
            }
        }
        f();
        for (k, v) in saved {
            match v {
                Some(value) => env::set_var(&k, value),
                None => env::remove_var(&k),
            }
        }
    }

    #[test]
    fn detect_honours_no_color_env() {
        with_env(
            &[
                ("NO_COLOR", Some("1")),
                ("CLICOLOR", None),
                ("CLICOLOR_FORCE", None),
            ],
            || {
                // Even when claiming stdout is a TTY, NO_COLOR wins.
                assert!(!should_use_color(ColorMode::Auto, true));
            },
        );
    }

    #[test]
    fn detect_honours_clicolor_force() {
        with_env(
            &[
                ("NO_COLOR", None),
                ("CLICOLOR", None),
                ("CLICOLOR_FORCE", Some("1")),
            ],
            || {
                // CLICOLOR_FORCE turns color on even off a TTY.
                assert!(should_use_color(ColorMode::Auto, false));
            },
        );
    }

    #[test]
    fn plain_has_no_capabilities() {
        let p = TermPolicy::plain();
        assert!(!p.color);
        assert!(!p.hyperlinks);
        assert!(!p.stdout_is_tty);
        assert_eq!(p.width, 80);
        assert_eq!(p.height, 24);
    }
}
