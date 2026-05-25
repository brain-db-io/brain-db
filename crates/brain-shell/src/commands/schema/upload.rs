//! `schema upload --from-file <PATH>` — additively expand the active
//! schema with the declarations in a `.brain` DSL file.
//!
//! The upload merges on top of whatever is already active (the system
//! `brain:` namespace plus any earlier user uploads). On a validation
//! failure the server returns the issue list instead of persisting; we
//! render those rows so the operator can fix the source and retry.

use brain_sdk_rust::{Client, ClientError};

use brain_explore::AdHocTable;

use crate::commands::Rendered;
use crate::parser::SchemaUploadArgs;
use crate::session::Session;

pub async fn run(
    client: &Client,
    _session: &mut Session,
    args: SchemaUploadArgs,
) -> Result<Rendered, ClientError> {
    let source = super::read_source(&args.from_file)?;
    let outcome = client.schema().upload_text(source).await?;

    // A non-empty issue list means the server rejected the upload —
    // surface every diagnostic so the operator can fix the source.
    if !outcome.errors.is_empty() {
        let rows: Vec<Vec<String>> = outcome
            .errors
            .iter()
            .map(|e| {
                vec![
                    e.code.clone(),
                    format!("{}:{}", e.line, e.column),
                    e.message.clone(),
                ]
            })
            .collect();
        return Ok(Box::new(AdHocTable {
            headers: vec!["code".into(), "loc".into(), "message".into()],
            rows,
        }));
    }

    let version = outcome
        .schema_version
        .map(|v| v.to_string())
        .unwrap_or_else(|| "—".into());
    Ok(Box::new(AdHocTable {
        headers: vec!["namespace".into(), "active_version".into(), "status".into()],
        rows: vec![vec![outcome.namespace, version, "merged".into()]],
    }))
}
