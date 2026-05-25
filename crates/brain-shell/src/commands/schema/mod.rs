//! `schema` management commands.
//!
//! Schema is a first-class additive expansion of the active vocabulary,
//! not a flag or mode. `schema upload` merges new declarations on top of
//! the system-default `brain:` namespace (plus any earlier user
//! uploads); `schema list` / `schema get` surface what's currently
//! active. Every action reuses the SDK `SchemaClient`.

pub mod get;
pub mod list;
pub mod upload;
pub mod validate;

use brain_sdk_rust::{Client, ClientError};

use crate::parser::SchemaCommand;
use crate::session::Session;

use super::Rendered;

pub async fn run(
    client: &Client,
    session: &mut Session,
    cmd: SchemaCommand,
) -> Result<Rendered, ClientError> {
    match cmd {
        SchemaCommand::Upload(args) => upload::run(client, session, args).await,
        SchemaCommand::Get(args) => get::run(client, session, args).await,
        SchemaCommand::List(args) => list::run(client, session, args).await,
        SchemaCommand::Validate(args) => validate::run(client, session, args).await,
    }
}

#[must_use]
pub fn op_name(cmd: &SchemaCommand) -> &'static str {
    match cmd {
        SchemaCommand::Upload(_) => "schema_upload",
        SchemaCommand::Get(_) => "schema_get",
        SchemaCommand::List(_) => "schema_list",
        SchemaCommand::Validate(_) => "schema_validate",
    }
}

/// Read DSL source from a path, or stdin when the path is `-`.
pub(super) fn read_source(from_file: &str) -> Result<String, ClientError> {
    use std::io::Read as _;

    if from_file == "-" {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| ClientError::Internal(format!("read stdin: {e}")))?;
        return Ok(s);
    }
    std::fs::read_to_string(from_file)
        .map_err(|e| ClientError::Internal(format!("read {from_file}: {e}")))
}
