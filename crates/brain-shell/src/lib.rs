//! `brain` — interactive shell + one-shot CLI for the Brain
//! cognitive substrate. The `psql` / `redis-cli` / `mongosh`
//! equivalent. Speaks the binary wire protocol via
//! [`brain_sdk_rust::Client`].
//!
//! # Two modes, one parser
//!
//! Both invocation modes are parsed by the same `clap` tree
//! ([`parser::Cli`]). Each REPL line is tokenised and fed through
//! `Cli::try_parse_from` exactly as if it were argv:
//!
//! ```text
//! $ brain encode "hello" --context 1          # one-shot
//! $ brain
//! brain> encode "hello" --context 1           # REPL
//! ```
//!
//! # Module map
//!
//! - [`parser`]  — clap `Cli` + `Command` + tokeniser.
//! - [`commands`] — one module per verb, all returning a uniform
//!   boxed renderer for the dispatch loop to format.
//! - [`connection`] — wrap [`brain_sdk_rust::Client`] with the
//!   shell's defaults (timeout, retry, agent id).
//! - [`session`] — REPL state across lines (active txn, sticky
//!   context, recent ids).
//! - Rendering — handled by the `brain-explore` crate (theme, term,
//!   table, render). Both brain-shell and brain-cli consume the same
//!   library so their output stays in lockstep.
//! - [`repl`] — rustyline editor, completion, event loop.
//! - [`cli`] — one-shot dispatch.

#![forbid(unsafe_code)]
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    // The closure in cli/args.rs::dispatch_argv carries a multi-arm
    // `match` that isn't reducible to a simple value, so `unwrap_or`
    // (eager) isn't equivalent to `unwrap_or_else` (lazy). Clippy's
    // newer heuristic mis-flags it.
    clippy::unnecessary_lazy_evaluations,
    // The pre-existing rustyline-Context API uses `&mut Context`; the
    // call site doesn't actually mutate, but the API requires the
    // reference shape. Can't be changed without bumping rustyline.
    clippy::unnecessary_mut_passed,
    // PromotionNote's two variants share the `Promoted` prefix because
    // they're both promotion outcomes; the prefix is meaningful in the
    // domain, not redundant.
    clippy::enum_variant_names,
    // AgentEntry::Default implementation is intentionally hand-rolled
    // — defaults are non-trivial (empty strings; bool false) and the
    // hand-rolled form anchors the contract for a reader.
    clippy::derivable_impls,
    // commands/info.rs uses a closure for clarity at the call site
    // even when clippy would prefer the bare function reference.
    clippy::redundant_closure,
)]

use std::process::ExitCode;

pub mod cli;
pub mod commands;
pub mod connection;
pub mod parser;
pub mod repl;
pub mod session;

/// Top-level entry — parses argv, dispatches to one-shot or REPL.
/// Returns a process exit code (0 success, 1 op failure, 2 usage
/// error).
pub async fn run() -> ExitCode {
    cli::dispatch_argv(std::env::args().collect()).await
}
