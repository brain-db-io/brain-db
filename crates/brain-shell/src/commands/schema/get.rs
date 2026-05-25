//! `schema get <ns> [--version N]` — show a namespace's active (or a
//! specific) schema version. `--version 0` (the default) resolves to
//! the active version.

use brain_sdk_rust::{Client, ClientError};

use brain_explore::AdHocTable;

use crate::commands::Rendered;
use crate::parser::SchemaGetArgs;
use crate::session::Session;

pub async fn run(
    client: &Client,
    _session: &mut Session,
    args: SchemaGetArgs,
) -> Result<Rendered, ClientError> {
    let view = client.schema().get(args.namespace, args.version).await?;

    let has_doc = if view.schema_document.is_empty() {
        "no"
    } else {
        "yes"
    };
    Ok(Box::new(AdHocTable {
        headers: vec![
            "namespace".into(),
            "version".into(),
            "validator".into(),
            "has_source".into(),
        ],
        rows: vec![vec![
            view.namespace,
            view.schema_version.to_string(),
            view.validator_version.to_string(),
            has_doc.into(),
        ]],
    }))
}
