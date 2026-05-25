//! `schema list [<ns>]` — the schema versions stored for a namespace,
//! newest first. Defaults to the system `brain:` namespace, which is
//! always active and carries the built-in vocabulary every shard boots
//! with; user uploads land as later versions of their own namespaces.

use brain_sdk_rust::{Client, ClientError};

use brain_explore::AdHocTable;

use crate::commands::Rendered;
use crate::parser::SchemaListArgs;
use crate::session::Session;

pub async fn run(
    client: &Client,
    _session: &mut Session,
    args: SchemaListArgs,
) -> Result<Rendered, ClientError> {
    let view = client.schema().list(args.namespace.clone()).await?;

    let rows: Vec<Vec<String>> = view
        .items
        .iter()
        .map(|e| {
            vec![
                view.namespace.clone(),
                e.schema_version.to_string(),
                e.validator_version.to_string(),
                if e.has_source_text { "yes" } else { "no" }.into(),
            ]
        })
        .collect();

    Ok(Box::new(AdHocTable {
        headers: vec![
            "namespace".into(),
            "version".into(),
            "validator".into(),
            "has_source".into(),
        ],
        rows,
    }))
}
