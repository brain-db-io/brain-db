//! Capability probes: color, hyperlinks, terminal size.
//!
//! Pure functions over env vars + isatty + `terminal_size` so the rest of
//! the codebase never reads `NO_COLOR` itself. Add a new env override
//! here, then thread it through [`TermPolicy::detect`](super::policy::TermPolicy::detect).

use std::env;

use super::policy::{ColorMode, HyperlinkMode};

/// Resolve the `--color` flag against env vars + isatty.
///
/// Precedence (highest first):
///   1. `--color=always` / `--color=never`
///   2. `NO_COLOR` set (any value) → off (per <https://no-color.org>)
///   3. `CLICOLOR_FORCE` non-zero → on
///   4. `CLICOLOR=0` → off
///   5. isatty(stdout) — color iff stdout is a TTY
#[must_use]
pub fn should_use_color(mode: ColorMode, stdout_is_tty: bool) -> bool {
    match mode {
        ColorMode::Always => return true,
        ColorMode::Never => return false,
        ColorMode::Auto => {}
    }
    if env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if let Ok(v) = env::var("CLICOLOR_FORCE") {
        if v != "0" {
            return true;
        }
    }
    if let Ok(v) = env::var("CLICOLOR") {
        if v == "0" {
            return false;
        }
    }
    stdout_is_tty
}

/// Resolve the `--hyperlinks` flag.
///
/// `auto` consults the `supports-hyperlinks` probe — it knows the
/// terminals (iTerm2, kitty, WezTerm, modern VTE, …) that handle OSC 8
/// cleanly and bails on the ones that print the escape sequence as noise.
#[must_use]
pub fn should_use_hyperlinks(mode: HyperlinkMode, stdout_is_tty: bool) -> bool {
    match mode {
        HyperlinkMode::Always => true,
        HyperlinkMode::Never => false,
        HyperlinkMode::Auto => {
            stdout_is_tty && supports_hyperlinks::on(supports_hyperlinks::Stream::Stdout)
        }
    }
}

/// Probe terminal size, honoring `$COLUMNS` / `$LINES` overrides.
///
/// Env vars come first so test harnesses, recorded sessions, and ssh
/// pipes can pin the size without poking ioctl.
#[must_use]
pub fn detect_terminal_size() -> (usize, usize) {
    let env_cols = env::var("COLUMNS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    let env_lines = env::var("LINES").ok().and_then(|s| s.parse::<usize>().ok());

    let (probed_w, probed_h) = terminal_size::terminal_size()
        .map(|(w, h)| (w.0 as usize, h.0 as usize))
        .unwrap_or((100, 30));

    let width = env_cols.unwrap_or(probed_w);
    let height = env_lines.unwrap_or(probed_h);
    (width, height)
}
