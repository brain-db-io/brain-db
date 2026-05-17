//! Unit tests for `BrainGraphRetriever` (phase 23.2).

use std::sync::Arc;

use brain_core::knowledge::{Cardinality, Relation};
use brain_core::{Entity, EntityId, EntityTypeId, ExtractorId, RelationId, RelationTypeId};
use brain_index::{
    Direction, GraphError, GraphQuery, GraphRetriever, GraphRetrieverConfig, RankedItemId,
};
use brain_metadata::entity_ops::entity_put;
use brain_metadata::entity_type_ops::entity_type_intern;
use brain_metadata::relation_ops::relation_create;
use brain_metadata::relation_type_ops::relation_type_intern;
use brain_metadata::MetadataDb;
use parking_lot::Mutex;
use tempfile::TempDir;

use super::BrainGraphRetriever;

// ---------------------------------------------------------------------------
// Setup helpers.
// ---------------------------------------------------------------------------

const PERSON_TYPE: &str = "brain:Person";

fn ensure_person_type(metadata: &mut MetadataDb) -> EntityTypeId {
    let wtxn = metadata.write_txn().expect("wtxn");
    let id = entity_type_intern(&wtxn, PERSON_TYPE, Vec::new(), 0).expect("type intern");
    wtxn.commit().expect("commit");
    id
}

fn make_retriever_with_db(metadata: Arc<Mutex<MetadataDb>>) -> BrainGraphRetriever {
    BrainGraphRetriever::new(metadata)
}

fn fresh_with_metadata() -> (TempDir, Arc<Mutex<MetadataDb>>) {
    let dir = TempDir::new().expect("tempdir");
    let mut metadata = MetadataDb::open(dir.path().join("metadata.redb")).expect("open");
    ensure_person_type(&mut metadata);
    (dir, Arc::new(Mutex::new(metadata)))
}

fn put_entity(metadata: &Arc<Mutex<MetadataDb>>, name: &str, type_id: EntityTypeId) -> EntityId {
    let id = EntityId::new();
    let entity = Entity::new_active(id, type_id, name.into(), name.to_lowercase(), 0);
    let mut db = metadata.lock();
    let wtxn = db.write_txn().expect("wtxn");
    entity_put(&wtxn, &entity).expect("entity_put");
    wtxn.commit().expect("commit");
    id
}

fn intern_relation_type(
    metadata: &Arc<Mutex<MetadataDb>>,
    namespace: &str,
    name: &str,
) -> RelationTypeId {
    let mut db = metadata.lock();
    let wtxn = db.write_txn().expect("wtxn");
    let id = relation_type_intern(
        &wtxn,
        namespace,
        name,
        None,
        None,
        Cardinality::ManyToMany,
        false,
        1,
        "",
        0,
    )
    .expect("rtype");
    wtxn.commit().expect("commit");
    id
}

fn create_relation(
    metadata: &Arc<Mutex<MetadataDb>>,
    relation_type: RelationTypeId,
    from: EntityId,
    to: EntityId,
) -> RelationId {
    let id = RelationId::new();
    let r = Relation::new_root(
        id,
        relation_type,
        from,
        to,
        0.9,
        Vec::new(),
        ExtractorId::from(0),
        0,
        false,
    );
    let mut db = metadata.lock();
    let wtxn = db.write_txn().expect("wtxn");
    let created = relation_create(&wtxn, &r, 0).expect("relation_create");
    wtxn.commit().expect("commit");
    created
}

fn current_person_type(metadata: &Arc<Mutex<MetadataDb>>) -> EntityTypeId {
    // Person type was already interned by fresh_with_metadata;
    // re-call intern to get its id back.
    let mut db = metadata.lock();
    let wtxn = db.write_txn().expect("wtxn");
    let id = entity_type_intern(&wtxn, PERSON_TYPE, Vec::new(), 0).expect("intern");
    wtxn.commit().expect("commit");
    id
}

// ---------------------------------------------------------------------------
// Star.
// ---------------------------------------------------------------------------

#[test]
fn star_depth_1_returns_neighbours() {
    let (_dir, metadata) = fresh_with_metadata();
    let type_id = current_person_type(&metadata);
    let a = put_entity(&metadata, "A", type_id);
    let b = put_entity(&metadata, "B", type_id);
    let rt = intern_relation_type(&metadata, "brain", "knows");
    let _r_ab = create_relation(&metadata, rt, a, b);

    let retriever = make_retriever_with_db(metadata);
    let result = retriever
        .retrieve(
            &GraphQuery::Star {
                anchor: a,
                depth: 1,
                direction: Direction::Outgoing,
                relation_types: None,
                include_statements: false,
            },
            &GraphRetrieverConfig::default(),
        )
        .expect("retrieve");

    // Should include B (entity) and the relation. Anchor A omitted.
    let mut entity_count = 0;
    let mut relation_count = 0;
    let mut sees_b = false;
    for item in &result {
        match item.id {
            RankedItemId::Entity(id) => {
                entity_count += 1;
                if id == b {
                    sees_b = true;
                }
            }
            RankedItemId::Relation(_) => relation_count += 1,
            _ => {}
        }
    }
    assert_eq!(entity_count, 1, "exactly one entity (B); A omitted");
    assert!(sees_b);
    assert_eq!(relation_count, 1, "the A→B relation");
}

#[test]
fn star_relation_type_filter_narrows() {
    let (_dir, metadata) = fresh_with_metadata();
    let type_id = current_person_type(&metadata);
    let a = put_entity(&metadata, "A", type_id);
    let b = put_entity(&metadata, "B", type_id);
    let c = put_entity(&metadata, "C", type_id);
    let knows = intern_relation_type(&metadata, "brain", "knows");
    let likes = intern_relation_type(&metadata, "brain", "likes");
    create_relation(&metadata, knows, a, b);
    create_relation(&metadata, likes, a, c);

    let retriever = make_retriever_with_db(metadata);
    let result = retriever
        .retrieve(
            &GraphQuery::Star {
                anchor: a,
                depth: 1,
                direction: Direction::Outgoing,
                relation_types: Some(vec![knows]),
                include_statements: false,
            },
            &GraphRetrieverConfig::default(),
        )
        .expect("retrieve");

    let entities: Vec<EntityId> = result
        .iter()
        .filter_map(|i| match i.id {
            RankedItemId::Entity(id) => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(entities, vec![b], "only B reachable via `knows`");
}

#[test]
fn star_direction_outgoing_vs_incoming() {
    let (_dir, metadata) = fresh_with_metadata();
    let type_id = current_person_type(&metadata);
    let a = put_entity(&metadata, "A", type_id);
    let b = put_entity(&metadata, "B", type_id);
    let c = put_entity(&metadata, "C", type_id);
    let rt = intern_relation_type(&metadata, "brain", "knows");
    create_relation(&metadata, rt, a, b); // A→B
    create_relation(&metadata, rt, c, a); // C→A

    let retriever = make_retriever_with_db(metadata);

    let out = retriever
        .retrieve(
            &GraphQuery::Star {
                anchor: a,
                depth: 1,
                direction: Direction::Outgoing,
                relation_types: None,
                include_statements: false,
            },
            &GraphRetrieverConfig::default(),
        )
        .expect("retrieve");
    let out_entities: Vec<EntityId> = out
        .iter()
        .filter_map(|i| match i.id {
            RankedItemId::Entity(id) => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(out_entities, vec![b], "outgoing reaches B only");

    let inc = retriever
        .retrieve(
            &GraphQuery::Star {
                anchor: a,
                depth: 1,
                direction: Direction::Incoming,
                relation_types: None,
                include_statements: false,
            },
            &GraphRetrieverConfig::default(),
        )
        .expect("retrieve");
    let inc_entities: Vec<EntityId> = inc
        .iter()
        .filter_map(|i| match i.id {
            RankedItemId::Entity(id) => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(inc_entities, vec![c], "incoming reaches C only");
}

#[test]
fn star_depth_2_reaches_second_hop() {
    let (_dir, metadata) = fresh_with_metadata();
    let type_id = current_person_type(&metadata);
    let a = put_entity(&metadata, "A", type_id);
    let b = put_entity(&metadata, "B", type_id);
    let c = put_entity(&metadata, "C", type_id);
    let rt = intern_relation_type(&metadata, "brain", "knows");
    create_relation(&metadata, rt, a, b);
    create_relation(&metadata, rt, b, c);

    let retriever = make_retriever_with_db(metadata);
    let result = retriever
        .retrieve(
            &GraphQuery::Star {
                anchor: a,
                depth: 2,
                direction: Direction::Outgoing,
                relation_types: None,
                include_statements: false,
            },
            &GraphRetrieverConfig::default(),
        )
        .expect("retrieve");

    let entities: Vec<EntityId> = result
        .iter()
        .filter_map(|i| match i.id {
            RankedItemId::Entity(id) => Some(id),
            _ => None,
        })
        .collect();
    assert!(entities.contains(&b));
    assert!(entities.contains(&c));
}

// ---------------------------------------------------------------------------
// Path.
// ---------------------------------------------------------------------------

#[test]
fn path_finds_direct_link() {
    let (_dir, metadata) = fresh_with_metadata();
    let type_id = current_person_type(&metadata);
    let a = put_entity(&metadata, "A", type_id);
    let b = put_entity(&metadata, "B", type_id);
    let rt = intern_relation_type(&metadata, "brain", "knows");
    let _r = create_relation(&metadata, rt, a, b);

    let retriever = make_retriever_with_db(metadata);
    let result = retriever
        .retrieve(
            &GraphQuery::Path {
                from: a,
                to: b,
                max_depth: 3,
            },
            &GraphRetrieverConfig::default(),
        )
        .expect("retrieve");

    // Must include both A and B as entities, plus the relation.
    let entity_ids: Vec<EntityId> = result
        .iter()
        .filter_map(|i| match i.id {
            RankedItemId::Entity(id) => Some(id),
            _ => None,
        })
        .collect();
    assert!(entity_ids.contains(&a));
    assert!(entity_ids.contains(&b));
}

#[test]
fn path_no_path_returns_both_endpoints() {
    let (_dir, metadata) = fresh_with_metadata();
    let type_id = current_person_type(&metadata);
    let a = put_entity(&metadata, "A", type_id);
    let b = put_entity(&metadata, "B", type_id);
    // No relations linking A and B.

    let retriever = make_retriever_with_db(metadata);
    let result = retriever
        .retrieve(
            &GraphQuery::Path {
                from: a,
                to: b,
                max_depth: 3,
            },
            &GraphRetrieverConfig::default(),
        )
        .expect("retrieve");

    let entity_ids: Vec<EntityId> = result
        .iter()
        .filter_map(|i| match i.id {
            RankedItemId::Entity(id) => Some(id),
            _ => None,
        })
        .collect();
    assert!(entity_ids.contains(&a));
    assert!(entity_ids.contains(&b));
    // Half-credit score per §23/04 §3.
    for item in &result {
        if matches!(item.id, RankedItemId::Relation(_)) {
            panic!("no-path result should not contain relations");
        }
    }
}

// ---------------------------------------------------------------------------
// Subgraph.
// ---------------------------------------------------------------------------

#[test]
fn subgraph_returns_closed_neighbourhood() {
    let (_dir, metadata) = fresh_with_metadata();
    let type_id = current_person_type(&metadata);
    let a = put_entity(&metadata, "A", type_id);
    let b = put_entity(&metadata, "B", type_id);
    let c = put_entity(&metadata, "C", type_id);
    let rt = intern_relation_type(&metadata, "brain", "knows");
    create_relation(&metadata, rt, a, b);
    create_relation(&metadata, rt, b, c);

    let retriever = make_retriever_with_db(metadata);
    let result = retriever
        .retrieve(
            &GraphQuery::Subgraph {
                anchor: a,
                depth: 2,
            },
            &GraphRetrieverConfig::default(),
        )
        .expect("retrieve");

    let entity_ids: Vec<EntityId> = result
        .iter()
        .filter_map(|i| match i.id {
            RankedItemId::Entity(id) => Some(id),
            _ => None,
        })
        .collect();
    // Anchor A omitted; B and C in the closed neighbourhood.
    assert!(entity_ids.contains(&b));
    assert!(entity_ids.contains(&c));
    assert!(!entity_ids.contains(&a));
}

// ---------------------------------------------------------------------------
// Caps + errors.
// ---------------------------------------------------------------------------

#[test]
fn max_depth_above_cap_errors() {
    let (_dir, metadata) = fresh_with_metadata();
    let type_id = current_person_type(&metadata);
    let a = put_entity(&metadata, "A", type_id);

    let retriever = make_retriever_with_db(metadata);
    let err = retriever
        .retrieve(
            &GraphQuery::Star {
                anchor: a,
                depth: 6,
                direction: Direction::Outgoing,
                relation_types: None,
                include_statements: false,
            },
            &GraphRetrieverConfig::default(),
        )
        .expect_err("rejects depth above cap");
    assert!(matches!(err, GraphError::MaxDepthExceeded { got: 6 }));
}

#[test]
fn anchor_not_found_errors() {
    let (_dir, metadata) = fresh_with_metadata();
    let retriever = make_retriever_with_db(metadata);
    let random = EntityId::new();

    let err = retriever
        .retrieve(
            &GraphQuery::Star {
                anchor: random,
                depth: 1,
                direction: Direction::Outgoing,
                relation_types: None,
                include_statements: false,
            },
            &GraphRetrieverConfig::default(),
        )
        .expect_err("rejects missing anchor");
    assert!(matches!(err, GraphError::AnchorNotFound(_)));
}

#[test]
fn ranks_are_dense_and_one_based() {
    let (_dir, metadata) = fresh_with_metadata();
    let type_id = current_person_type(&metadata);
    let a = put_entity(&metadata, "A", type_id);
    let b = put_entity(&metadata, "B", type_id);
    let c = put_entity(&metadata, "C", type_id);
    let rt = intern_relation_type(&metadata, "brain", "knows");
    create_relation(&metadata, rt, a, b);
    create_relation(&metadata, rt, a, c);

    let retriever = make_retriever_with_db(metadata);
    let result = retriever
        .retrieve(
            &GraphQuery::Star {
                anchor: a,
                depth: 1,
                direction: Direction::Outgoing,
                relation_types: None,
                include_statements: false,
            },
            &GraphRetrieverConfig::default(),
        )
        .expect("retrieve");

    for (i, item) in result.iter().enumerate() {
        assert_eq!(item.rank, (i as u32) + 1);
    }
}
