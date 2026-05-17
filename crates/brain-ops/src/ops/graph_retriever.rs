//! Production `GraphRetriever` impl (phase 23.2).
//!
//! Same split as the semantic retriever (23.1): the trait + value
//! types live in `brain-index::graph_retriever`; the impl that
//! actually reads the entity / relation / statement redb tables
//! lives here so `brain-index` stays free of the
//! `brain-metadata` → `brain-storage` → `glommio` Linux-only
//! transitive dep.
//!
//! Three traversal modes per §23/04 §3:
//!
//! - `Star` — BFS from anchor, emitting visited entities,
//!   traversed relations, and (optionally) statements with the
//!   visited entities as subjects.
//! - `Path` — single-source BFS from `from` with early
//!   termination at `to`; v1 emits the discovered path. Bidirectional
//!   BFS is post-v1 (§23/04 §3).
//! - `Subgraph` — closed k-hop neighbourhood (Star with
//!   `relation_types = None` and `include_statements = true`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use brain_core::knowledge::{Relation, SubjectRef};
use brain_core::{EntityId, RelationId, RelationTypeId, StatementId};
use brain_index::{
    proximity_score, validate_graph_depth, Direction, GraphError, GraphQuery, GraphRetriever,
    GraphRetrieverConfig, RankedItem, RankedItemId,
};
use brain_metadata::entity_ops::{entity_get, EntityOpError};
use brain_metadata::relation_ops::{
    relation_list_from, relation_list_to, RelationListFilter, RelationOpError,
};
use brain_metadata::statement_ops::{statement_list, StatementListFilter, StatementOpError};
use brain_metadata::MetadataDb;
use parking_lot::Mutex;
use redb::ReadTransaction;

/// Production `GraphRetriever` impl. Cheap to clone — single
/// `Arc<Mutex<...>>` field.
#[derive(Clone)]
pub struct BrainGraphRetriever {
    metadata: Arc<Mutex<MetadataDb>>,
}

impl BrainGraphRetriever {
    #[must_use]
    pub fn new(metadata: Arc<Mutex<MetadataDb>>) -> Self {
        Self { metadata }
    }
}

impl GraphRetriever for BrainGraphRetriever {
    fn retrieve(
        &self,
        query: &GraphQuery,
        config: &GraphRetrieverConfig,
    ) -> Result<Vec<RankedItem>, GraphError> {
        validate_graph_depth(query, config)?;

        let db_guard = self.metadata.lock();
        let rtxn = db_guard
            .read_txn()
            .map_err(|e| GraphError::IndexUnavailable(format!("read_txn: {e}")))?;

        match query {
            GraphQuery::Star {
                anchor,
                depth,
                direction,
                relation_types,
                include_statements,
            } => run_star(
                &rtxn,
                *anchor,
                *depth,
                *direction,
                relation_types.as_deref(),
                *include_statements,
                config,
            ),
            GraphQuery::Path {
                from,
                to,
                max_depth,
            } => run_path(&rtxn, *from, *to, *max_depth, config),
            GraphQuery::Subgraph { anchor, depth } => {
                run_star(&rtxn, *anchor, *depth, Direction::Both, None, true, config)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Star / Subgraph.
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn run_star(
    rtxn: &ReadTransaction,
    anchor: EntityId,
    depth: u8,
    direction: Direction,
    relation_types: Option<&[RelationTypeId]>,
    include_statements: bool,
    config: &GraphRetrieverConfig,
) -> Result<Vec<RankedItem>, GraphError> {
    if entity_get(rtxn, anchor).map_err(map_entity_err)?.is_none() {
        return Err(GraphError::AnchorNotFound(anchor));
    }

    let mut visited: HashSet<EntityId> = HashSet::new();
    visited.insert(anchor);
    let mut frontier: VecDeque<(EntityId, u8)> = VecDeque::new();
    frontier.push_back((anchor, 0));

    let mut emitted_entities: Vec<(EntityId, u8)> = Vec::new();
    let mut emitted_relations: Vec<(RelationId, u8)> = Vec::new();
    let mut emitted_relations_seen: HashSet<RelationId> = HashSet::new();
    let mut emitted_statements: Vec<(StatementId, u8)> = Vec::new();

    while let Some((current, hop)) = frontier.pop_front() {
        // Anchor itself omitted (§23/04 §2).
        if hop > 0 {
            emitted_entities.push((current, hop));
        }

        if include_statements {
            push_statements(
                rtxn,
                current,
                hop,
                config.max_branching,
                &mut emitted_statements,
            )?;
        }

        if hop >= depth {
            // Reached the depth cap; this entity is emitted but
            // we don't expand further.
            continue;
        }

        let children = fetch_children(rtxn, current, direction, relation_types, config)?;
        for (relation, neighbour) in children {
            if emitted_relations_seen.insert(relation.id) {
                emitted_relations.push((relation.id, hop));
            }
            if !visited.contains(&neighbour) {
                visited.insert(neighbour);
                frontier.push_back((neighbour, hop + 1));
            }
        }
    }

    Ok(rank_and_truncate(
        emitted_entities,
        emitted_relations,
        emitted_statements,
        config.top_k,
    ))
}

// ---------------------------------------------------------------------------
// Path.
// ---------------------------------------------------------------------------

fn run_path(
    rtxn: &ReadTransaction,
    from: EntityId,
    to: EntityId,
    max_depth: u8,
    config: &GraphRetrieverConfig,
) -> Result<Vec<RankedItem>, GraphError> {
    if entity_get(rtxn, from).map_err(map_entity_err)?.is_none() {
        return Err(GraphError::AnchorNotFound(from));
    }
    if entity_get(rtxn, to).map_err(map_entity_err)?.is_none() {
        return Err(GraphError::AnchorNotFound(to));
    }

    // Single-source BFS from `from`. Tracks parent for each
    // discovered entity so we can reconstruct the path once
    // `to` is dequeued (§23/04 §3 v1).
    let mut visited: HashSet<EntityId> = HashSet::new();
    visited.insert(from);
    let mut parents: HashMap<EntityId, (EntityId, RelationId)> = HashMap::new();
    let mut frontier: VecDeque<(EntityId, u8)> = VecDeque::new();
    frontier.push_back((from, 0));

    let mut found = false;
    while let Some((current, hop)) = frontier.pop_front() {
        if current == to {
            found = true;
            break;
        }
        if hop >= max_depth {
            continue;
        }
        let children = fetch_children(rtxn, current, Direction::Both, None, config)?;
        for (relation, neighbour) in children {
            if visited.insert(neighbour) {
                parents.insert(neighbour, (current, relation.id));
                frontier.push_back((neighbour, hop + 1));
                if neighbour == to {
                    // Don't break here — let the outer loop's
                    // pop_front pick `to` up and exit cleanly.
                }
            }
        }
    }

    let mut entities: Vec<(EntityId, u8)> = Vec::new();
    let mut relations: Vec<(RelationId, u8)> = Vec::new();
    if found {
        // Walk back from `to` via `parents` to assemble the
        // path. Both endpoints emitted with score 1.0; the
        // path-length affects scores of intermediates only.
        entities.push((from, 0));
        entities.push((to, 0));
        let mut cursor = to;
        let mut depth_from_target: u8 = 1;
        while let Some((parent, rel_id)) = parents.get(&cursor) {
            relations.push((*rel_id, depth_from_target.saturating_sub(1)));
            if *parent != from {
                entities.push((*parent, depth_from_target));
            }
            cursor = *parent;
            depth_from_target = depth_from_target.saturating_add(1);
            if cursor == from {
                break;
            }
        }
    } else {
        // No path; §23/04 §3 emits both anchors with score 0.5.
        entities.push((from, 1));
        entities.push((to, 1));
    }

    Ok(rank_and_truncate(
        entities,
        relations,
        Vec::new(),
        config.top_k,
    ))
}

// ---------------------------------------------------------------------------
// Children expansion.
// ---------------------------------------------------------------------------

fn fetch_children(
    rtxn: &ReadTransaction,
    entity: EntityId,
    direction: Direction,
    relation_types: Option<&[RelationTypeId]>,
    config: &GraphRetrieverConfig,
) -> Result<Vec<(Relation, EntityId)>, GraphError> {
    let mut filter = RelationListFilter {
        relation_type: None,
        current_only: true,
        limit: config.max_branching as usize,
    };

    let mut relations: Vec<Relation> = Vec::new();
    match direction {
        Direction::Outgoing => {
            relations.extend(fetch_dir(
                rtxn,
                entity,
                true,
                &mut filter,
                relation_types,
                config,
            )?);
        }
        Direction::Incoming => {
            relations.extend(fetch_dir(
                rtxn,
                entity,
                false,
                &mut filter,
                relation_types,
                config,
            )?);
        }
        Direction::Both => {
            relations.extend(fetch_dir(
                rtxn,
                entity,
                true,
                &mut filter,
                relation_types,
                config,
            )?);
            relations.extend(fetch_dir(
                rtxn,
                entity,
                false,
                &mut filter,
                relation_types,
                config,
            )?);
        }
    }

    // Deterministic child sort by RelationId bytes (§23/04 §2 +
    // §4 — reproducible truncation at branching cap).
    relations.sort_by_key(|r| r.id.to_bytes());
    relations.dedup_by_key(|r| r.id);

    let truncated = if relations.len() > config.max_branching as usize {
        tracing::warn!(
            target: "brain_ops::graph_retriever",
            anchor = ?entity,
            cap = config.max_branching,
            actual = relations.len(),
            "graph retriever branching truncated",
        );
        relations.truncate(config.max_branching as usize);
        true
    } else {
        false
    };
    let _ = truncated;

    let pairs = relations
        .into_iter()
        .map(|r| {
            let neighbour = if r.from_entity == entity {
                r.to_entity
            } else {
                r.from_entity
            };
            (r, neighbour)
        })
        .collect();
    Ok(pairs)
}

fn fetch_dir(
    rtxn: &ReadTransaction,
    entity: EntityId,
    outgoing: bool,
    filter: &mut RelationListFilter,
    relation_types: Option<&[RelationTypeId]>,
    config: &GraphRetrieverConfig,
) -> Result<Vec<Relation>, GraphError> {
    let _ = config;
    let mut out: Vec<Relation> = Vec::new();
    if let Some(types) = relation_types {
        if types.is_empty() {
            return Ok(out);
        }
        for ty in types {
            filter.relation_type = Some(*ty);
            let rows = if outgoing {
                relation_list_from(rtxn, entity, filter).map_err(map_relation_err)?
            } else {
                relation_list_to(rtxn, entity, filter).map_err(map_relation_err)?
            };
            out.extend(rows);
        }
    } else {
        filter.relation_type = None;
        let rows = if outgoing {
            relation_list_from(rtxn, entity, filter).map_err(map_relation_err)?
        } else {
            relation_list_to(rtxn, entity, filter).map_err(map_relation_err)?
        };
        out.extend(rows);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

fn push_statements(
    rtxn: &ReadTransaction,
    subject: EntityId,
    hop: u8,
    cap: u32,
    out: &mut Vec<(StatementId, u8)>,
) -> Result<(), GraphError> {
    let filter = StatementListFilter {
        subject: Some(subject),
        predicate: None,
        kind: None,
        current_only: true,
        min_confidence: None,
        limit: cap as usize,
    };
    let rows = statement_list(rtxn, &filter).map_err(map_statement_err)?;
    for s in rows {
        if matches!(s.subject, SubjectRef::Entity(_)) {
            out.push((s.id, hop));
        }
    }
    Ok(())
}

fn rank_and_truncate(
    entities: Vec<(EntityId, u8)>,
    relations: Vec<(RelationId, u8)>,
    statements: Vec<(StatementId, u8)>,
    top_k: usize,
) -> Vec<RankedItem> {
    let mut items: Vec<RankedItem> =
        Vec::with_capacity(entities.len() + relations.len() + statements.len());
    for (id, d) in entities {
        items.push(RankedItem {
            id: RankedItemId::Entity(id),
            rank: 0,
            score: proximity_score(d),
            snippet: None,
        });
    }
    for (id, d) in relations {
        items.push(RankedItem {
            id: RankedItemId::Relation(id),
            rank: 0,
            score: proximity_score(d),
            snippet: None,
        });
    }
    for (id, d) in statements {
        items.push(RankedItem {
            id: RankedItemId::Statement(id),
            rank: 0,
            score: proximity_score(d),
            snippet: None,
        });
    }
    items.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| id_sort_key(&a.id).cmp(&id_sort_key(&b.id)))
    });
    items.truncate(top_k);
    for (i, it) in items.iter_mut().enumerate() {
        it.rank = (i as u32) + 1;
    }
    items
}

fn id_sort_key(id: &RankedItemId) -> [u8; 17] {
    let mut key = [0u8; 17];
    match id {
        RankedItemId::Memory(m) => {
            key[0] = 0;
            key[1..].copy_from_slice(&m.raw().to_be_bytes());
        }
        RankedItemId::Statement(s) => {
            key[0] = 1;
            key[1..].copy_from_slice(&s.to_bytes());
        }
        RankedItemId::Entity(e) => {
            key[0] = 2;
            key[1..].copy_from_slice(&e.to_bytes());
        }
        RankedItemId::Relation(r) => {
            key[0] = 3;
            key[1..].copy_from_slice(&r.to_bytes());
        }
    }
    key
}

fn map_entity_err(e: EntityOpError) -> GraphError {
    GraphError::IndexUnavailable(format!("entity_get: {e}"))
}

fn map_relation_err(e: RelationOpError) -> GraphError {
    GraphError::IndexUnavailable(format!("relation_list: {e}"))
}

fn map_statement_err(e: StatementOpError) -> GraphError {
    GraphError::IndexUnavailable(format!("statement_list: {e}"))
}

#[cfg(test)]
mod tests;
