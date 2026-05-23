//! Per-op-family response payload structs. Split out of `response.rs`
//! to keep the dispatch enum (and its impl) the visible heart of the
//! module while structurally-similar payloads live together.

pub mod admin;
pub mod cognitive;
pub mod entity;
pub mod error;
pub mod events;
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
pub use error::*;
pub use events::*;
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
