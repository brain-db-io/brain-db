//! Apply functions for relation-shaped phases.
//!
//! Full P2c coverage: UpsertRelation, Tombstone(Relation),
//! Supersede(Relation) all real.

use brain_core::knowledge::Relation;
use brain_metadata::relation_ops::{relation_create, relation_supersede, relation_tombstone};
use redb::WriteTransaction;

use super::ApplyError;
use crate::write::{
    Phase, PhaseAck, SupersedeReplacement, SupersedeTarget, TombstoneTarget, Write,
};

pub fn apply_upsert_relation(
    wtxn: &WriteTransaction,
    phase: &Phase,
    _write: &Write,
) -> Result<PhaseAck, ApplyError> {
    let Phase::UpsertRelation {
        id,
        ty,
        from,
        to,
        confidence,
        evidence_memories,
        is_symmetric,
        extractor,
        extracted_at_unix_nanos,
    } = phase
    else {
        return Err(ApplyError::PhaseMisShape("expected UpsertRelation"));
    };
    let r = Relation::new_root(
        *id,
        *ty,
        *from,
        *to,
        *confidence,
        evidence_memories.clone(),
        *extractor,
        *extracted_at_unix_nanos,
        *is_symmetric,
    );
    relation_create(wtxn, &r, *extracted_at_unix_nanos)
        .map_err(|e| ApplyError::Metadata(format!("relation_create: {e}")))?;
    Ok(PhaseAck::UpsertedRelation(*id, 1))
}

pub fn apply_supersede_relation(
    wtxn: &WriteTransaction,
    phase: &Phase,
    _write: &Write,
) -> Result<PhaseAck, ApplyError> {
    let Phase::Supersede {
        target,
        replacement,
        at_unix_nanos,
    } = phase
    else {
        return Err(ApplyError::PhaseMisShape("expected Supersede"));
    };
    let SupersedeTarget::Relation(old_id) = target else {
        return Err(ApplyError::PhaseMisShape("expected Supersede(Relation)"));
    };
    let SupersedeReplacement::Relation(new_relation) = replacement else {
        return Err(ApplyError::PhaseMisShape(
            "expected Supersede with Relation replacement",
        ));
    };
    relation_supersede(wtxn, *old_id, new_relation.as_ref(), *at_unix_nanos)
        .map_err(|e| ApplyError::Metadata(format!("relation_supersede: {e}")))?;
    Ok(PhaseAck::Superseded(*target, replacement.id()))
}

pub fn apply_tombstone_relation(
    wtxn: &WriteTransaction,
    phase: &Phase,
    _write: &Write,
) -> Result<PhaseAck, ApplyError> {
    let Phase::Tombstone {
        target,
        at_unix_nanos,
        ..
    } = phase
    else {
        return Err(ApplyError::PhaseMisShape("expected Tombstone"));
    };
    let TombstoneTarget::Relation(id) = target else {
        return Err(ApplyError::PhaseMisShape("expected Tombstone(Relation)"));
    };
    relation_tombstone(wtxn, *id, *at_unix_nanos)
        .map_err(|e| ApplyError::Metadata(format!("relation_tombstone: {e}")))?;
    Ok(PhaseAck::Tombstoned(*target))
}
