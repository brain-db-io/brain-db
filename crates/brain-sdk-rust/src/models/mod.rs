//! Typed value models exposed by the SDK: domain types (entities,
//! relations, statements) and the typed-handle wrappers (`EntityHandle<T>`,
//! `BrainEntityType`).
//!
//! Distinct from `ops/` (per-opcode request builders) — these are the
//! Rust types you read from / write to. The split mirrors brain-ops'
//! `apply/` (per-table value mutations) vs `handlers/` (per-opcode
//! request handlers): one layer holds the verbs, the other holds the
//! nouns.

pub mod entity;
pub mod errors;
