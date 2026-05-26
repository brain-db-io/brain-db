//! redb table definitions and value types, one module per table.
//!
//! The catalog is 13 domain tables plus one internal `__schema_meta`
//! from [`crate::storage_version`].

pub mod agent;
pub mod api_keys;
pub mod audit;
pub mod checkpoint;
pub mod context;
pub mod edge;
pub mod entity;
pub mod entity_type;
pub mod extractor;
pub mod extractor_audit;
pub mod fingerprint;
pub mod idempotency;
pub mod memory;
pub mod merge;
pub mod merge_review_queue;
pub mod model_fingerprint;
pub mod next_lsn;
pub mod predicate;
pub mod relation;
pub mod relation_type;
pub mod schema_version;
pub mod slot_version;
pub mod statement;
pub mod text;
pub mod worker_checkpoints;

/// Boilerplate `redb::Value` impl for an rkyv-archived struct.
///
/// Each value type in the knowledge layer uses the same encoding
/// pattern (rkyv with `check_bytes`, deserialize-on-read, type_name
/// versioned with `::v1`). This macro emits that impl from the type
/// name and a stable `type_name` string.
///
/// Mirrors the per-file impl in substrate tables (`agent.rs`,
/// `memory.rs`); collapsed into a macro here because 11 knowledge-layer
/// value structs share the exact same body.
#[macro_export]
macro_rules! impl_redb_rkyv_value {
    ($ty:ty, $type_name:literal) => {
        impl ::redb::Value for $ty {
            type SelfType<'a> = $ty;
            type AsBytes<'a> = ::std::vec::Vec<u8>;

            fn fixed_width() -> Option<usize> {
                None
            }

            fn from_bytes<'a>(data: &'a [u8]) -> Self::SelfType<'a>
            where
                Self: 'a,
            {
                let mut buf = ::rkyv::AlignedVec::with_capacity(data.len());
                buf.extend_from_slice(data);
                ::rkyv::from_bytes::<$ty>(&buf).expect(concat!(
                    stringify!($ty),
                    " bytes failed rkyv validation; redb file is corrupt"
                ))
            }

            fn as_bytes<'a, 'b: 'a>(value: &'a Self::SelfType<'b>) -> Self::AsBytes<'a>
            where
                Self: 'a,
                Self: 'b,
            {
                ::rkyv::to_bytes::<_, 256>(value)
                    .expect(concat!(stringify!($ty), " is rkyv-serializable"))
                    .into_vec()
            }

            fn type_name() -> ::redb::TypeName {
                ::redb::TypeName::new($type_name)
            }
        }
    };
}

#[cfg(all(test, not(miri)))]
pub(crate) fn fresh_db(dir: &tempfile::TempDir) -> redb::Database {
    redb::Database::create(dir.path().join("test.redb")).expect("create redb")
}
