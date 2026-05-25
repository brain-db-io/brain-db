//! `schema validate --from-file <PATH>` — parse + validate a `.brain`
//! DSL file without persisting it. A clean validation reports the
//! version the upload *would* land at; a failure lists the issues.

use brain_sdk_rust::{Client, ClientError};

use brain_explore::AdHocTable;

use crate::commands::Rendered;
use crate::parser::SchemaValidateArgs;
use crate::session::Session;

pub async fn run(
    client: &Client,
    _session: &mut Session,
    args: SchemaValidateArgs,
) -> Result<Rendered, ClientError> {
    let source = super::read_source(&args.from_file)?;
    let outcome = client.schema().validate(source).await?;

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

    Ok(Box::new(AdHocTable {
        headers: vec![
            "namespace".into(),
            "would_be_version".into(),
            "status".into(),
        ],
        rows: vec![vec![
            outcome.namespace,
            outcome.would_be_version.to_string(),
            "valid".into(),
        ]],
    }))
}
