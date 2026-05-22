//! `encode` flag parsing: `--edge`, `--request-id`, `--from-file`,
//! `--from-stdin`, `--wait`, `--allow-duplicate`. Verifies parse,
//! plus the mutual-exclusivity matrix clap enforces.

use brain_shell::parser::{Cli, Command, EdgeKindArg};
use clap::Parser;

fn parse(argv: &[&str]) -> Result<Cli, clap::Error> {
    let mut full = vec!["brain".to_string()];
    full.extend(argv.iter().map(|s| (*s).to_string()));
    Cli::try_parse_from(full)
}

fn parse_ok(argv: &[&str]) -> Cli {
    parse(argv).expect("parse should succeed")
}

#[test]
fn encode_with_inline_edges() {
    let cli = parse_ok(&[
        "encode",
        "hello",
        "--edge",
        "similar_to:s2/m17/v1",
        "--edge",
        "references:s2/m5/v1",
    ]);
    let Some(Command::Encode(a)) = cli.subcommand else {
        panic!("expected Encode")
    };
    assert_eq!(a.edges.len(), 2);
    assert_eq!(a.edges[0].kind, EdgeKindArg::SimilarTo);
    assert_eq!(a.edges[1].kind, EdgeKindArg::References);
}

#[test]
fn encode_request_id_accepted_as_uuid() {
    let cli = parse_ok(&[
        "encode",
        "hello",
        "--request-id",
        "019e3b00-0000-7000-8000-000000000001",
    ]);
    let Some(Command::Encode(a)) = cli.subcommand else {
        panic!("expected Encode")
    };
    assert!(a.request_id.is_some());
}

#[test]
fn encode_from_file_excludes_positional_text() {
    let err = parse(&["encode", "hello", "--from-file", "/tmp/x"]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("cannot be used with") || msg.contains("not allowed"),
        "expected clap mutual-exclusivity error, got: {msg}"
    );
}

#[test]
fn encode_from_stdin_alone() {
    let cli = parse_ok(&["encode", "--from-stdin"]);
    let Some(Command::Encode(a)) = cli.subcommand else {
        panic!("expected Encode")
    };
    assert!(a.from_stdin);
    assert!(a.text.is_none());
}

#[test]
fn encode_rejects_unknown_flag_cleanly() {
    // Unknown flags must surface as a clap error (no panic, no
    // backtrace). The shell relies on clap's `try_parse_from` for
    // this; the regression is that nothing in the call chain
    // unwraps or panics on bad argv.
    let err = parse(&["encode", "hello", "--bogus-flag", "x"]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("unexpected") || msg.contains("unrecognized") || msg.contains("--bogus-flag"),
        "expected clap unknown-arg error, got: {msg}"
    );
}

#[test]
fn encode_vector_flag_is_removed() {
    // The flag is gone — clap must reject it rather than silently
    // accepting it (which would mean a leftover declaration).
    let err = parse(&["encode", "hello", "--vector", "0.1,0.2,0.3"]).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("unexpected") || msg.contains("unrecognized") || msg.contains("--vector"),
        "--vector must be unknown to clap, got: {msg}"
    );
}

#[test]
fn encode_wait_parses_and_covers_all_stages() {
    let cli = parse_ok(&["encode", "hello", "--wait"]);
    let Some(Command::Encode(a)) = cli.subcommand else {
        panic!("expected Encode")
    };
    assert!(a.wait);
}

#[test]
fn encode_legacy_wait_for_extraction_flag_rejected() {
    // No back-compat alias: the old --wait-for-extraction is gone.
    // Brain is pre-release, so renaming wins over a synonym layer.
    assert!(parse(&["encode", "hello", "--wait-for-extraction"]).is_err());
}

#[test]
fn encode_dedup_is_on_by_default() {
    // Without --allow-duplicate, deduplication is ON. The CLI struct
    // exposes the opt-out; the handler converts to the wire bool.
    let cli = parse_ok(&["encode", "hello world that is plenty long enough"]);
    let Some(Command::Encode(a)) = cli.subcommand else {
        panic!("expected Encode")
    };
    assert!(
        !a.allow_duplicate,
        "default should be allow_duplicate=false (dedup on)"
    );
}

#[test]
fn encode_allow_duplicate_flag_parses() {
    let cli = parse_ok(&[
        "encode",
        "hello world that is plenty long enough",
        "--allow-duplicate",
    ]);
    let Some(Command::Encode(a)) = cli.subcommand else {
        panic!("expected Encode")
    };
    assert!(a.allow_duplicate);
}

#[test]
fn encode_legacy_deduplicate_flag_rejected() {
    // No compat shim: the old --deduplicate / --no-dedup flags are gone.
    assert!(parse(&["encode", "hello", "--deduplicate"]).is_err());
    assert!(parse(&["encode", "hello", "--no-dedup"]).is_err());
}

#[test]
fn edge_spec_rejects_unknown_kind() {
    let err = parse(&["encode", "hello", "--edge", "foo:s2/m1/v1"]).unwrap_err();
    assert!(err.to_string().contains("unknown edge kind"), "{err}");
}

#[test]
fn edge_spec_rejects_missing_colon() {
    let err = parse(&["encode", "hello", "--edge", "similar_to_s2_m1_v1"]).unwrap_err();
    assert!(err.to_string().contains("expected"), "got: {err}");
}

#[test]
fn short_o_flag_aliases_output() {
    let cli = parse_ok(&["-o", "ndjson", "encode", "hi"]);
    use brain_shell::parser::OutputFormatArg;
    assert_eq!(cli.global.output, Some(OutputFormatArg::Ndjson));
}

#[test]
fn output_jsonpath_carries_expr() {
    let cli = parse_ok(&["-o", "jsonpath=.memory_id", "recall", "x"]);
    use brain_shell::parser::OutputFormatArg;
    match cli.global.output {
        Some(OutputFormatArg::JsonPath(expr)) => assert_eq!(expr, ".memory_id"),
        other => panic!("expected JsonPath, got {other:?}"),
    }
}
