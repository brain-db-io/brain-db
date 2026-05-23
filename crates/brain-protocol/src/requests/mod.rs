//! Per-op-family request payload structs. Split out of `request.rs`
//! to keep the dispatch enum (and its impl) the visible heart of the
//! module while structurally-similar payloads live together.

pub mod admin;
pub mod cognitive;
pub mod entity;
pub mod extractor;
pub mod link;
pub mod procedural;
pub mod query;
pub mod relation;
pub mod schema;
pub mod statement;
pub mod stream;
pub mod subscribe;
pub mod txn;
pub mod types;

pub use admin::*;
pub use cognitive::*;
pub use entity::*;
pub use extractor::*;
pub use link::*;
pub use procedural::*;
pub use query::*;
pub use relation::*;
pub use schema::*;
pub use statement::*;
pub use stream::*;
pub use subscribe::*;
pub use txn::*;
pub use types::*;
