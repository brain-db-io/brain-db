//! Terminal capability detection and lifecycle.
//!
//! The rest of the library never touches env vars or isatty directly —
//! every renderer consults a [`TermPolicy`](policy::TermPolicy) built
//! once at command dispatch. One place to honor NO_COLOR, CLICOLOR,
//! `--color`, `--hyperlinks`, `$PAGER`, and the terminal-size probe.

pub mod detect;
pub mod hyperlink;
pub mod pager;
pub mod policy;

pub use hyperlink::link;
pub use pager::Pager;
pub use policy::{ColorMode, HyperlinkMode, TermPolicy};
