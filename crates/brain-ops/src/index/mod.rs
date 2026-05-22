//! Per-shard index helpers — graph + semantic retrievers and the
//! tantivy text indexer. Index code lives here (not in `ops/` or
//! `writer/`) because retriever / indexer concerns are orthogonal to
//! both wire-opcode handling and the write engine.

pub mod graph_retriever;
pub mod semantic_retriever;
pub mod text_indexer;
