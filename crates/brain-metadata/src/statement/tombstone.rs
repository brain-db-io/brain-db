//! Soft delete (`statement_tombstone`) and v1 retract intent.
//!
//! Physical reclamation lives in [`crate::extractor::sweep`].

use brain_core::knowledge::TombstoneReason;
use brain_core::StatementId;
use redb::{ReadableTable, WriteTransaction};

use crate::tables::statement::{
    StatementMetadata, STATEMENTS_BY_SUBJECT_TABLE, STATEMENTS_TABLE, STATEMENT_EMBED_QUEUE_TABLE,
};

use super::StatementOpError;

/// Soft delete. Sets `tombstoned / tombstoned_at / tombstone_reason`
/// and flips the by-subject `is_current` bit. Re-tombstoning an
/// already-tombstoned row is a no-op (returns `Ok`).
pub fn statement_tombstone(
    wtxn: &WriteTransaction,
    id: StatementId,
    reason: TombstoneReason,
    now_unix_nanos: u64,
) -> Result<(), StatementOpError> {
    let mut row = {
        let t = wtxn.open_table(STATEMENTS_TABLE)?;
        let r: Option<StatementMetadata> = t.get(&id.to_bytes())?.map(|g| g.value());
        r.ok_or(StatementOpError::NotFound(id))?
    };
    if row.is_tombstoned() {
        return Ok(());
    }
    let was_current = row.is_current != 0;
    let subject_bytes = row.subject_entity_bytes;
    let kind_byte = row.kind;
    let pred = row.predicate_id;

    row.tombstoned = 1;
    row.tombstoned_at_unix_nanos = Some(now_unix_nanos);
    row.tombstone_reason = reason.as_u8();
    // Record-time invalidation: tombstoning is the moment the substrate
    // stops recording the row as truth. Always stamp — even on the
    // `now == 0` legacy callers — so as-of queries get a consistent
    // signal (zero reads as epoch-time, but a tombstoned row is still
    // a tombstoned row at any later as_of).
    row.record_invalidated_at_unix_nanos = Some(now_unix_nanos);
    row.is_current = 0;

    {
        let mut t = wtxn.open_table(STATEMENTS_TABLE)?;
        t.insert(&row.statement_id_bytes, &row)?;
    }
    if was_current {
        let mut bys = wtxn.open_table(STATEMENTS_BY_SUBJECT_TABLE)?;
        bys.remove(&(subject_bytes, kind_byte, pred, 1u8))?;
        bys.insert(
            &(subject_bytes, kind_byte, pred, 0u8),
            &row.statement_id_bytes,
        )?;
    }
    // Drop any pending embed-queue row — a tombstoned statement does
    // not belong in the Statement HNSW. The worker filters tombstoned
    // rows defensively too, but removing here keeps the queue small
    // and avoids paying an embedding cost on a doomed row.
    {
        let mut q = wtxn.open_table(STATEMENT_EMBED_QUEUE_TABLE)?;
        q.remove(&row.statement_id_bytes)?;
    }
    Ok(())
}

/// Hard-delete intent. v1 implementation = `tombstone` with reason
/// `ExtractorRetraction` (caller may override). Physical reclamation
/// happens later via the phase-21+ GC worker.
//
// TODO(phase 21): wire the periodic reclamation worker so retracted
// rows are physically removed from STATEMENTS_TABLE + indexes after
// `RETRACT_GRACE_NANOS`.
pub fn statement_retract(
    wtxn: &WriteTransaction,
    id: StatementId,
    reason: TombstoneReason,
    now_unix_nanos: u64,
) -> Result<(), StatementOpError> {
    statement_tombstone(wtxn, id, reason, now_unix_nanos)
}

#[cfg(all(test, not(miri)))]
mod tests {
    use super::*;
    use crate::entity::ops::{entity_put, normalize_name};
    use crate::schema::predicate::predicate_intern;
    use crate::statement::crud::{statement_create, statement_get};
    use brain_core::knowledge::{
        Entity, EntityType, EvidenceRef, Statement, StatementKind, StatementObject, StatementValue,
        SubjectRef,
    };
    use brain_core::{EntityId, ExtractorId, PredicateId};

    fn open_db() -> (tempfile::TempDir, crate::MetadataDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::MetadataDb::open(dir.path().join("md.redb")).unwrap();
        (dir, db)
    }

    fn make_entity(db: &mut crate::MetadataDb, name: &str) -> EntityId {
        let id = EntityId::new();
        let e = Entity::new_active(
            id,
            EntityType::PERSON_ID,
            name.to_string(),
            normalize_name(name),
            1_700_000_000_000_000_000,
        );
        let wtxn = db.write_txn().unwrap();
        entity_put(&wtxn, &e).unwrap();
        wtxn.commit().unwrap();
        id
    }

    fn intern_fact(db: &mut crate::MetadataDb, name: &str) -> PredicateId {
        let wtxn = db.write_txn().unwrap();
        let id = predicate_intern(
            &wtxn,
            "test",
            name,
            Some(StatementKind::Fact),
            2,
            1,
            "",
            false,
            1_700_000_000_000_000_000,
        )
        .unwrap();
        wtxn.commit().unwrap();
        id
    }

    fn fresh_fact(subject: EntityId, predicate: PredicateId, value: &str) -> Statement {
        Statement::new_root(
            StatementId::new(),
            StatementKind::Fact,
            SubjectRef::Entity(subject),
            predicate,
            StatementObject::Value(StatementValue::Text(value.into())),
            0.9,
            EvidenceRef::default(),
            ExtractorId::from(0),
            1_700_000_000_000_000_000,
            1,
        )
    }

    #[test]
    fn tombstone_stamps_record_invalidated_at() {
        let (_dir, mut db) = open_db();
        let subj = make_entity(&mut db, "ada");
        let pred = intern_fact(&mut db, "knows");
        let s = fresh_fact(subj, pred, "lovelace");
        let wtxn = db.write_txn().unwrap();
        statement_create(&wtxn, &s, 1_700_000_000_000_000_000).unwrap();
        wtxn.commit().unwrap();

        let tomb_now: u64 = 1_700_000_000_000_000_750;
        let wtxn = db.write_txn().unwrap();
        statement_tombstone(&wtxn, s.id, TombstoneReason::UserRequest, tomb_now).unwrap();
        wtxn.commit().unwrap();

        let rtxn = db.read_txn().unwrap();
        let got = statement_get(&rtxn, s.id).unwrap().unwrap();
        assert!(got.tombstoned);
        assert_eq!(got.tombstoned_at_unix_nanos, Some(tomb_now));
        assert_eq!(got.record_invalidated_at_unix_nanos, Some(tomb_now));
    }

    #[test]
    fn double_tombstone_keeps_first_invalidation_timestamp() {
        // Re-tombstoning is a no-op (early return), so the
        // `record_invalidated_at` stays at the first call's wall-clock.
        let (_dir, mut db) = open_db();
        let subj = make_entity(&mut db, "ada-double");
        let pred = intern_fact(&mut db, "knows_double");
        let s = fresh_fact(subj, pred, "lovelace");
        let wtxn = db.write_txn().unwrap();
        statement_create(&wtxn, &s, 1_700_000_000_000_000_000).unwrap();
        wtxn.commit().unwrap();

        let first: u64 = 1_700_000_000_000_000_500;
        let wtxn = db.write_txn().unwrap();
        statement_tombstone(&wtxn, s.id, TombstoneReason::UserRequest, first).unwrap();
        wtxn.commit().unwrap();

        let later: u64 = 1_700_000_000_000_001_500;
        let wtxn = db.write_txn().unwrap();
        statement_tombstone(&wtxn, s.id, TombstoneReason::UserRequest, later).unwrap();
        wtxn.commit().unwrap();

        let rtxn = db.read_txn().unwrap();
        let got = statement_get(&rtxn, s.id).unwrap().unwrap();
        assert_eq!(got.record_invalidated_at_unix_nanos, Some(first));
    }
}
