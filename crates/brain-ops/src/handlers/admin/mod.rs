//! Admin handlers — internal-tooling surfaces that don't (yet) have
//! dedicated wire opcodes. CLI / future admin protocol layers call into
//! these directly. Each function builds a `Write` and submits through
//! the unified writer path, so admin actions land in the WAL and audit
//! tables the same way wire ops do.

pub mod merge_review;
