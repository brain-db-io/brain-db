//! Tracing/log subscriber installation.
//!
//! Sub-task 12.2. Spec §14/02.
//!
//! Two entry points:
//!
//! - [`init_pre_config`] — called before the config is loaded so
//!   startup errors are still captured. Defaults to a `compact`
//!   formatter at `info` level; honors `BRAIN_LOG` / `RUST_LOG`.
//! - [`reinit_from_config`] — called after `Config::load`. Switches
//!   the formatter and level per the `[logging]` section. Because
//!   `tracing` only allows one global subscriber, this is a no-op if
//!   `init_pre_config` already installed one — but the values are
//!   logged for operator visibility.
//!
//! ## Formats supported
//!
//! - `compact` — single-line `<ts> <LEVEL> <target>: <message>`. Dev
//!   default; readable in a terminal.
//! - `json` — newline-delimited JSON per spec §14/02 §1. Production
//!   default; ingestible by Loki / Elastic / Splunk.
//!
//! ## Environment
//!
//! The filter precedence is `BRAIN_LOG` > `RUST_LOG` > config
//! `[logging.level]`. `BRAIN_LOG` is the operator-facing knob; the
//! `RUST_LOG` fallback exists because every Rust crate the world over
//! reads it, and surprising operators isn't worth being purist.

#![cfg(target_os = "linux")]

use tracing::info;
use tracing_subscriber::fmt;
use tracing_subscriber::EnvFilter;

use crate::config::LoggingConfig;

/// Resolved log format — one of `compact`, `json`. Unrecognised
/// strings fall back to `Compact` with a warning at install time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    Compact,
    Json,
}

impl LogFormat {
    /// Parse the `[logging] format = "..."` config knob.
    #[must_use]
    pub fn parse(s: &str) -> (Self, Option<String>) {
        match s.to_ascii_lowercase().as_str() {
            "compact" | "" => (LogFormat::Compact, None),
            "json" => (LogFormat::Json, None),
            other => (
                LogFormat::Compact,
                Some(format!(
                    "unrecognised logging.format `{other}` (allowed: compact, json) — using compact",
                )),
            ),
        }
    }
}

/// Build an [`EnvFilter`] from the environment with `default_level`
/// as the fallback. Precedence: `BRAIN_LOG` > `RUST_LOG` >
/// `default_level`.
fn build_filter(default_level: &str) -> EnvFilter {
    if let Ok(s) = std::env::var("BRAIN_LOG") {
        if let Ok(f) = EnvFilter::try_new(s) {
            return f;
        }
    }
    EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level))
}

/// Install a minimal subscriber before the config is loaded. Idempotent
/// via `try_init` — only the first call succeeds.
pub fn init_pre_config() {
    let filter = build_filter("info");
    let _ = fmt().with_env_filter(filter).with_target(true).try_init();
}

/// Re-install or update the subscriber from `[logging]`. Honors the
/// `format` knob:
///
/// - `compact` → text formatter (dev-friendly).
/// - `json` → JSON formatter (one object per line, spec §14/02 §1).
///
/// Logs an `info` event with the resolved format + level so operators
/// can confirm the wiring.
pub fn reinit_from_config(logging: &LoggingConfig) {
    let (format, warn) = LogFormat::parse(&logging.format);
    let filter = build_filter(logging.level.as_str());

    let installed = match format {
        LogFormat::Compact => fmt()
            .with_env_filter(filter)
            .with_target(true)
            .try_init()
            .is_ok(),
        LogFormat::Json => fmt()
            .with_env_filter(filter)
            .with_target(true)
            .json()
            .try_init()
            .is_ok(),
    };

    if let Some(msg) = warn {
        tracing::warn!("{msg}");
    }
    info!(
        format = ?format,
        level = %logging.level,
        output = %logging.output,
        installed,
        "logging subscriber configured"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_compact_is_default() {
        assert_eq!(LogFormat::parse("compact").0, LogFormat::Compact);
        assert_eq!(LogFormat::parse("Compact").0, LogFormat::Compact);
        assert_eq!(LogFormat::parse("").0, LogFormat::Compact);
    }

    #[test]
    fn parse_json_recognised() {
        assert_eq!(LogFormat::parse("json").0, LogFormat::Json);
        assert_eq!(LogFormat::parse("JSON").0, LogFormat::Json);
    }

    #[test]
    fn parse_unknown_falls_back_with_warning() {
        let (fmt, warn) = LogFormat::parse("yaml");
        assert_eq!(fmt, LogFormat::Compact);
        assert!(warn.is_some(), "unknown format must surface a warning");
        assert!(warn.unwrap().contains("yaml"));
    }
}
