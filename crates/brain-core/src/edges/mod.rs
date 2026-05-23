//! Links between nodes.
//!
//! Substrate edges (memory ↔ memory), mention edges (memory → entity),
//! and typed relations (entity ↔ entity) all share the same key shape:
//! `(NodeRef from, EdgeKindRef kind, NodeRef to, disambiguator)`. One
//! unified edge table holds all three so a single prefix scan from any
//! `NodeRef` returns every outgoing edge regardless of kind.

pub mod edge;
pub mod edge_kind_ref;
pub mod node_ref;

pub use edge::{Edge, EdgeKind, EdgeOrigin};
pub use edge_kind_ref::{EdgeKindRef, EdgeKindRefError};
pub use node_ref::{NodeRef, NodeRefError};
