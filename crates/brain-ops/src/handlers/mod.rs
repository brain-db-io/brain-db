//! Wire-opcode handlers: one module per spec §09 cognitive operation.
//! Engine code (`writer`, `apply`, `index`) and infrastructure
//! (`context`, `dispatch`, `idempotency`, etc.) live at the crate root.

pub mod admin;
pub mod encode;
pub mod entity;
pub mod events;
pub mod extractor_admin;
pub mod forget;
pub mod link;
pub mod plan;
pub mod procedural;
pub mod query;
pub mod reason;
pub mod recall;
pub mod relation;
pub mod schema;
pub mod statement;
pub mod subscribe;
pub mod txn;
