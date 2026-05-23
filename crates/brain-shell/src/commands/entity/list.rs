//! `entity list [--type T] [--limit N]` — paginated entity table.
//!
//! The wire op (`EntityListReq`) takes an `entity_type_id` (u32). The
//! shell CLI exposes a string `--type` flag (e.g. `Person`); resolving
//! a qname to its numeric type id needs a schema-resolver op the SDK
//! doesn't ship. The command supports only the built-in `Person` for
//! now; other types return a structured error.

use brain_sdk_rust::{Client, ClientError, Person};

use brain_explore::AdHocTable;

use crate::commands::Rendered;
use crate::parser::EntityListArgs;
use crate::session::Session;

pub async fn run(
    client: &Client,
    _session: &mut Session,
    args: EntityListArgs,
) -> Result<Rendered, ClientError> {
    let qname = args.type_qname.as_deref().unwrap_or("Person");
    if !qname.eq_ignore_ascii_case("person") {
        return Err(ClientError::Internal(format!(
            "entity list --type {qname}: only `Person` is supported. \
             Listing other types needs a schema-resolver wire op (not in this build)."
        )));
    }

    let mut builder = client.entity::<Person>().list().limit(args.limit);
    if let Some(prefix) = args.prefix {
        builder = builder.with_prefix(prefix);
    }
    let handles = builder.fetch().await?;

    let rows: Vec<Vec<String>> = handles
        .into_iter()
        .map(|h| {
            vec![
                h.id.0.to_string(),
                h.canonical_name,
                qname.to_string(),
                h.mention_count.to_string(),
            ]
        })
        .collect();

    Ok(Box::new(AdHocTable {
        headers: vec![
            "id".to_string(),
            "name".to_string(),
            "type".to_string(),
            "mentions".to_string(),
        ],
        rows,
    }))
}
