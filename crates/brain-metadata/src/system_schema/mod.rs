//! System schema bootstrap (spec §21/06, phase 19.7).
//!
//! At `MetadataDb::open`, this module:
//!
//! 1. Parses + validates the embedded [`SYSTEM_SCHEMA_SOURCE`].
//! 2. Reads the current active version for the `brain:` namespace.
//! 3. If unset (fresh DB), runs `schema_upload` to persist + fan
//!    out into the entity_type / predicate / relation_type /
//!    extractor intern paths.
//! 4. If already active (existing DB), reconciles the embedded
//!    extractor definitions against `EXTRACTORS_TABLE` so a
//!    codebase upgrade that adds a built-in extractor back-fills
//!    on next open. Same-content rows no-op; missing rows land;
//!    diverged rows surface as a hard error so an operator-visible
//!    upgrade conflict isn't silently overwritten.
//!
//! Parse / validate failures **panic** — the source is
//! `include_str!()` content; a failure is a build bug, not a
//! runtime condition.

use brain_protocol::schema::{parse_schema, validate_system_schema, SchemaItem, ValidatedSchema};
use redb::{Database, ReadableDatabase, WriteTransaction};

use crate::extractor::ops::{extractor_intern, ExtractorOpError};
use crate::schema::apply::map_extractor_kind;
use crate::schema::store::{schema_active, schema_upload, SchemaStoreError};

/// The embedded system-schema DSL source. Single source of truth
/// for the built-in `brain:*` types.
pub const SYSTEM_SCHEMA_SOURCE: &str = include_str!("schema.brain");

/// The namespace name the system schema declares. Reserved per
/// §21/04.
pub const SYSTEM_SCHEMA_NAMESPACE: &str = "brain";

#[derive(thiserror::Error, Debug)]
pub enum SystemSchemaError {
    #[error("redb transaction error: {0}")]
    Transaction(#[from] redb::TransactionError),

    #[error("redb commit error: {0}")]
    Commit(#[from] redb::CommitError),

    #[error("schema_store: {0}")]
    Schema(#[from] SchemaStoreError),

    /// The embedded schema's extractor row diverges from what's in
    /// `EXTRACTORS_TABLE`. Surfaced by reconciliation so an
    /// operator-edited `schema.brain` that breaks a prior
    /// definition isn't silently overwritten. Bumping the schema
    /// version (via a real SCHEMA_UPLOAD) is the recovery path.
    #[error("embedded system schema diverges from stored extractor {qname:?}")]
    DivergedExtractor { qname: String },

    /// `extractor_intern` returned a non-divergence error during
    /// reconciliation (storage, table, identifier validation).
    #[error("extractor reconciliation: {0}")]
    ExtractorOp(#[from] ExtractorOpError),

    /// `serde_json::to_vec` failed on an extractor AST during
    /// reconciliation. Build bug if it fires — the AST round-trips
    /// through the same encoder in `apply_schema_definitions`.
    #[error("extractor reconciliation encode: {0}")]
    ExtractorEncode(String),
}

/// Seed the system schema on first open; reconcile on subsequent
/// opens. Both branches are idempotent for inputs that match the
/// stored state.
pub fn seed_system_schema(db: &Database) -> Result<(), SystemSchemaError> {
    let schema = parse_schema(SYSTEM_SCHEMA_SOURCE)
        .expect("system schema must parse — include_str!() content is compile-time");
    let validated = validate_system_schema(&schema).unwrap_or_else(|errs| {
        panic!(
            "system schema must validate — include_str!() content is compile-time. Errors: {errs:?}"
        )
    });

    let active = {
        let rtxn = db.begin_read()?;
        schema_active(&rtxn, SYSTEM_SCHEMA_NAMESPACE)?
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    let wtxn = db.begin_write()?;
    match active {
        None => {
            schema_upload(&wtxn, &validated, now)?;
        }
        Some(version) => {
            reconcile_system_extractors(&wtxn, &validated, version, now)?;
        }
    }
    wtxn.commit()?;
    Ok(())
}

/// Diff the embedded schema's extractor definitions against
/// `EXTRACTORS_TABLE` via `extractor_intern`. Same-content rows
/// no-op; missing rows are written under the current active
/// `schema_version`; diverged rows raise [`SystemSchemaError::DivergedExtractor`].
///
/// Scoped to extractors only. Predicate / entity_type /
/// relation_type drift is a separate concern; if a similar gap
/// surfaces there, a follow-up plan can extend this helper to
/// cover them.
fn reconcile_system_extractors(
    wtxn: &WriteTransaction,
    validated: &ValidatedSchema,
    schema_version: u32,
    now_unix_nanos: u64,
) -> Result<(), SystemSchemaError> {
    let schema = validated.as_schema();
    let namespace = schema.namespace.as_str();
    for item in &schema.items {
        let SchemaItem::Extractor(e) = item else {
            continue;
        };
        let kind = map_extractor_kind(e.kind);
        let blob = serde_json::to_vec(e)
            .map_err(|err| SystemSchemaError::ExtractorEncode(err.to_string()))?;
        match extractor_intern(
            wtxn,
            namespace,
            &e.name,
            kind,
            schema_version,
            blob,
            now_unix_nanos,
        ) {
            Ok(_) => {}
            Err(ExtractorOpError::AlreadyExists { qname, .. }) => {
                return Err(SystemSchemaError::DivergedExtractor { qname });
            }
            Err(other) => return Err(SystemSchemaError::ExtractorOp(other)),
        }
    }
    Ok(())
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::schema::store::{schema_get, schema_list};
    use brain_core::EntityType;

    #[test]
    fn system_schema_parses_and_validates() {
        let schema = parse_schema(SYSTEM_SCHEMA_SOURCE).expect("system schema parses");
        let _ = validate_system_schema(&schema).expect("system schema validates under system mode");
    }

    #[test]
    fn user_validate_rejects_brain_namespace() {
        let schema = parse_schema(SYSTEM_SCHEMA_SOURCE).unwrap();
        let errs = brain_protocol::schema::validate(&schema)
            .expect_err("user validate must reject `namespace brain`");
        assert!(errs
            .iter()
            .any(|e| e.code
                == brain_protocol::schema::ValidationErrorCode::NamespaceInvalidIdentifier));
    }

    #[test]
    fn seed_first_open_creates_brain_v1() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let db = Database::create(&path).unwrap();
        seed_system_schema(&db).unwrap();

        let rtxn = db.begin_read().unwrap();
        assert_eq!(schema_active(&rtxn, "brain").unwrap(), Some(1));
        let row = schema_get(&rtxn, "brain", 1).unwrap().unwrap();
        assert_eq!(row.namespace, "brain");
        assert_eq!(row.version, 1);
        assert_eq!(row.validator_version, 1);
    }

    #[test]
    fn seed_reopen_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        {
            let db = Database::create(&path).unwrap();
            seed_system_schema(&db).unwrap();
        }
        {
            let db = Database::open(&path).unwrap();
            seed_system_schema(&db).unwrap();

            let rtxn = db.begin_read().unwrap();
            let list = schema_list(&rtxn, "brain").unwrap();
            assert_eq!(list.len(), 1, "reopen must not create v2");
            assert_eq!(list[0].version, 1);
        }
    }

    #[test]
    fn person_resolves_to_id_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let db = Database::create(&path).unwrap();
        seed_system_schema(&db).unwrap();

        // Verify the Person entity-type row exists at PERSON_ID.
        use crate::tables::entity_type::ENTITY_TYPES_TABLE;
        let rtxn = db.begin_read().unwrap();
        let t = rtxn.open_table(ENTITY_TYPES_TABLE).unwrap();
        let row = t
            .get(&EntityType::PERSON_ID.raw())
            .unwrap()
            .unwrap()
            .value();
        assert_eq!(row.name, "Person");
    }

    #[test]
    fn system_schema_seeds_builtin_extractors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let db = Database::create(&path).unwrap();
        seed_system_schema(&db).unwrap();

        let rtxn = db.begin_read().unwrap();
        let all = crate::extractor::ops::extractor_list(&rtxn).unwrap();
        assert_eq!(all.len(), 3);
        let names: Vec<&str> = all.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"entity_mentions"));
        assert!(names.contains(&"gliner"));
        assert!(names.contains(&"llm_predicate"));
        for ext in &all {
            assert_eq!(ext.namespace, "brain");
            assert!(ext.is_enabled());
        }
    }

    #[test]
    fn system_schema_extractor_ids_are_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let db = Database::create(&path).unwrap();
        seed_system_schema(&db).unwrap();

        let rtxn = db.begin_read().unwrap();
        let entity_mentions =
            crate::extractor::ops::extractor_lookup_by_qname(&rtxn, "brain", "entity_mentions")
                .unwrap()
                .expect("entity_mentions registered");
        let gliner = crate::extractor::ops::extractor_lookup_by_qname(&rtxn, "brain", "gliner")
            .unwrap()
            .expect("gliner registered");
        let llm_predicate =
            crate::extractor::ops::extractor_lookup_by_qname(&rtxn, "brain", "llm_predicate")
                .unwrap()
                .expect("llm_predicate registered");
        assert_eq!(entity_mentions.id().raw(), 1);
        assert_eq!(gliner.id().raw(), 2);
        assert_eq!(llm_predicate.id().raw(), 3);
    }

    #[test]
    fn system_schema_entity_type_ids_are_stable() {
        use crate::tables::entity_type::ENTITY_TYPES_TABLE;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let db = Database::create(&path).unwrap();
        seed_system_schema(&db).unwrap();

        let rtxn = db.begin_read().unwrap();
        let t = rtxn.open_table(ENTITY_TYPES_TABLE).unwrap();
        for (expected_id, name) in [
            (1u32, "Person"),
            (2, "Organization"),
            (3, "Project"),
            (4, "Event"),
            (5, "Place"),
            (6, "Concept"),
        ] {
            let row = t
                .get(&expected_id)
                .unwrap()
                .unwrap_or_else(|| panic!("EntityTypeId({expected_id}) ({name}) missing"))
                .value();
            assert_eq!(row.name, name, "id {expected_id}");
        }
    }

    #[test]
    fn system_schema_predicate_ids_are_stable() {
        use crate::schema::predicate::predicate_lookup_by_qname;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let db = Database::create(&path).unwrap();
        seed_system_schema(&db).unwrap();

        let rtxn = db.begin_read().unwrap();
        for (expected_id, name) in [
            (1u32, "is_a"),
            (2, "has_name"),
            (3, "mentions"),
            (4, "fact"),
            (5, "related_to"),
            (6, "prefers"),
            (7, "scheduled"),
            (8, "works_at"),
            (9, "member_of"),
            (10, "lives_in"),
            (11, "located_in"),
            (12, "owns"),
            (13, "current_role"),
            (14, "speaks"),
            (15, "has_skill"),
            (16, "likes"),
            (17, "dislikes"),
            (18, "occurred_at"),
            (19, "mentioned_in"),
            (20, "participated_in"),
            (21, "behavior_tone"),
            (22, "behavior_style"),
            (23, "behavior_avoids"),
            (24, "behavior_prefers"),
            (25, "behavior_constraint"),
        ] {
            let row = predicate_lookup_by_qname(&rtxn, "brain", name)
                .unwrap()
                .unwrap_or_else(|| panic!("brain:{name} predicate missing"));
            assert_eq!(row.id.raw(), expected_id, "brain:{name}");
        }
    }

    #[test]
    fn system_schema_extractor_definitions_decode_via_serde() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let db = Database::create(&path).unwrap();
        seed_system_schema(&db).unwrap();

        let rtxn = db.begin_read().unwrap();
        let row =
            crate::extractor::ops::extractor_lookup_by_qname(&rtxn, "brain", "entity_mentions")
                .unwrap()
                .unwrap();
        let ast: brain_protocol::schema::ExtractorDef =
            serde_json::from_slice(&row.definition_blob).expect("decode AST");
        assert_eq!(ast.name, "entity_mentions");
        assert!(
            matches!(
                ast.target,
                brain_protocol::schema::ExtractorTarget::EntityOrStatement
            ),
            "entity_mentions targets the union of entity + statement kinds",
        );
        let has_patterns = ast.fields.iter().any(
            |f| matches!(f, brain_protocol::schema::ExtractorField::Patterns(p) if p.len() == 2),
        );
        assert!(has_patterns);
    }

    #[test]
    fn reopen_does_not_duplicate_builtin_extractors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        {
            let db = Database::create(&path).unwrap();
            seed_system_schema(&db).unwrap();
        }
        let db = Database::open(&path).unwrap();
        seed_system_schema(&db).unwrap();

        let rtxn = db.begin_read().unwrap();
        let all = crate::extractor::ops::extractor_list(&rtxn).unwrap();
        assert_eq!(all.len(), 3, "reopen must not duplicate built-ins");
    }

    /// Reconciliation back-fills a built-in extractor row that's
    /// missing on reopen. Simulates the upgrade-time silent drift
    /// case: a deployment booted under a prior codebase version
    /// that pre-dated the row's introduction; the new boot picks
    /// it up.
    #[test]
    fn reconciliation_backfills_missing_extractor() {
        use crate::tables::extractor::{EXTRACTORS_BY_QNAME_TABLE, EXTRACTORS_TABLE};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        {
            let db = Database::create(&path).unwrap();
            seed_system_schema(&db).unwrap();

            // Manually drop the `gliner` row + its qname index
            // entry to simulate a DB that pre-dates that extractor's
            // addition. The schema-active row stays as-is so the
            // reconcile branch (not the seed branch) fires on
            // reopen.
            let wtxn = db.begin_write().unwrap();
            let removed_id = {
                let mut idx = wtxn.open_table(EXTRACTORS_BY_QNAME_TABLE).unwrap();
                let guard = idx.remove(&"brain:gliner").unwrap().unwrap();
                guard.value()
            };
            {
                let mut t = wtxn.open_table(EXTRACTORS_TABLE).unwrap();
                t.remove(&removed_id).unwrap().unwrap();
            }
            wtxn.commit().unwrap();
        }

        // Reopen — reconcile should re-intern `gliner`.
        let db = Database::open(&path).unwrap();
        seed_system_schema(&db).unwrap();

        let rtxn = db.begin_read().unwrap();
        let restored = crate::extractor::ops::extractor_lookup_by_qname(&rtxn, "brain", "gliner")
            .unwrap()
            .expect("gliner restored by reconciliation");
        assert_eq!(restored.namespace, "brain");
        assert_eq!(restored.name, "gliner");
        assert!(restored.is_enabled());
    }

    /// Reconciliation is a no-op when the table already matches
    /// the embedded schema. The post-reopen extractor list is
    /// byte-identical to the pre-reopen list (rkyv equality on
    /// `ExtractorDefinition` covers id, kind, schema_version,
    /// definition_blob, and timestamp).
    #[test]
    fn reconciliation_no_op_when_table_is_current() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        let before: Vec<_> = {
            let db = Database::create(&path).unwrap();
            seed_system_schema(&db).unwrap();
            let rtxn = db.begin_read().unwrap();
            crate::extractor::ops::extractor_list(&rtxn).unwrap()
        };

        let db = Database::open(&path).unwrap();
        seed_system_schema(&db).unwrap();

        let rtxn = db.begin_read().unwrap();
        let after = crate::extractor::ops::extractor_list(&rtxn).unwrap();
        assert_eq!(
            after, before,
            "reconciliation must not rewrite rows when content matches",
        );
    }

    /// A diverged extractor row (same qname, different
    /// definition_blob) raises `DivergedExtractor` instead of
    /// silently overwriting. Bumping the schema version via a
    /// real `SCHEMA_UPLOAD` is the recovery path; reconciliation
    /// refuses to make that decision unilaterally.
    #[test]
    fn reconciliation_propagates_diverged_definition() {
        use crate::tables::extractor::{ExtractorDefinition, EXTRACTORS_TABLE};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.redb");
        {
            let db = Database::create(&path).unwrap();
            seed_system_schema(&db).unwrap();

            // Overwrite `entity_mentions` (id 1) with a tampered
            // definition_blob. The qname index still points at id 1
            // so the intern's idempotency probe will fetch this row
            // and observe the divergence.
            let wtxn = db.begin_write().unwrap();
            {
                let mut t = wtxn.open_table(EXTRACTORS_TABLE).unwrap();
                let tampered = ExtractorDefinition::new(
                    brain_core::ExtractorId::from(1),
                    "brain".into(),
                    "entity_mentions".into(),
                    brain_core::ExtractorKind::Pattern,
                    true,
                    1,
                    b"tampered-definition".to_vec(),
                    0,
                );
                t.insert(&1u32, &tampered).unwrap();
            }
            wtxn.commit().unwrap();
        }

        let db = Database::open(&path).unwrap();
        let err = seed_system_schema(&db).expect_err("diverged definition must surface");
        match err {
            SystemSchemaError::DivergedExtractor { qname } => {
                assert_eq!(qname, "brain:entity_mentions");
            }
            other => panic!("expected DivergedExtractor, got {other:?}"),
        }
    }
}
