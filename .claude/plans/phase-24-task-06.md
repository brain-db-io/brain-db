# Plan: Phase 24 — Task 06, Entity GC worker

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1
**Depends on:** 24.0 (§27/03 §"Entity GC worker"),
                §18 entity layer.

---

## 1. Scope

**Off-by-default** periodic worker that tombstones entities
with no active inbound references after a grace period.
Reversible during grace by `ENTITY_RESTORE` (or by inbound
reference resurrection within the grace window).

Default off because automatic entity tombstoning can surprise
operators — entities are user-created records, not derived
state.

Concrete deliverables:

1. **`brain-metadata::entity_gc_ops`** (new module) — pure
   ops:
   - `inbound_reference_count(rtxn, entity_id) -> usize` —
     sums statements (subject), relations (from / to), and
     pending audit anchors.
   - `scan_eligible_entities(rtxn, grace_seconds, now, batch_cap)
     -> Vec<(EntityId, EligibilityReason)>`.
   - `tombstone_with_reason(wtxn, entity_id, reason, now)` —
     soft tombstone, writes audit row.
2. **`brain-workers/src/workers/entity_gc.rs`** (new) —
   `EntityGcWorker` running on the Low priority lane, default
   cadence daily, default disabled.
3. **Config**:
   - `BRAIN_ENTITY_GC_ENABLED` (default `false`).
   - `BRAIN_ENTITY_GC_GRACE_SECONDS` (default 2 592 000 = 30 d).
   - `BRAIN_ENTITY_GC_PERIOD_SECONDS` (default 86 400).
4. **Reversal hook** on `ENTITY_RESTORE` (admin op landing
   separately, but the cascade is: clear the tombstone flag,
   audit row).
5. **Metrics**: `sweeper_swept_total{worker="entity_gc"}`, `sweeper_skipped_total{worker, reason}` (reasons: `still_referenced`, `within_grace`).

## 2. Spec references

- `spec/18_entities/00_purpose.md` — entity lifecycle.
- `spec/25_provenance_versioning/00_purpose.md` §"Retention"
  — tombstone grace.
- `spec/27_knowledge_workers/03_sweeper_workers.md` (24.0)
  §"Entity GC worker" — eligibility predicate.

## 3. External validation

| Item | Source | Status |
|---|---|---|
| `ENTITIES_TABLE` | `brain-metadata::tables::knowledge::entity` | shipped |
| Statement-by-subject index | `brain-metadata::statement_ops::statement_list_by_subject` | shipped |
| Relation-by-from/to indexes | `brain-metadata::relation_ops::relation_list_*` | shipped |
| Entity tombstone op | `brain-metadata::entity_ops::entity_tombstone` | shipped |

## 4. Architecture sketch

```
brain-metadata/src/entity_gc_ops.rs                   (new)
  pub enum EligibilityReason {
      NoInboundReferences,
  }

  pub fn inbound_reference_count(
      rtxn: &ReadTransaction,
      entity_id: EntityId,
  ) -> Result<usize, Err> {
      let stmt_count = count_statements_by_subject(rtxn, entity_id)?;     // active only
      let rel_from = count_relations_by_from(rtxn, entity_id)?;
      let rel_to = count_relations_by_to(rtxn, entity_id)?;
      Ok(stmt_count + rel_from + rel_to)
  }

  pub fn scan_eligible(
      rtxn: &ReadTransaction,
      grace_seconds: u64,
      now_ns: u64,
      batch_cap: usize,
  ) -> Result<Vec<(EntityId, EligibilityReason)>, Err> {
      let mut out = Vec::new();
      let cutoff_ns = now_ns.saturating_sub(grace_seconds * 1_000_000_000);
      for entry in entities_table.iter()? {
          let row = entry?.value();
          if row.tombstoned { continue }
          if row.created_at_unix_nanos > cutoff_ns { continue }   // too new
          if inbound_reference_count(rtxn, row.entity_id())? > 0 { continue }
          out.push((row.entity_id(), EligibilityReason::NoInboundReferences));
          if out.len() == batch_cap { break }
      }
      Ok(out)
  }

brain-workers/src/workers/entity_gc.rs                (new)
  pub struct EntityGcWorker { config: EntityGcConfig }
  pub struct EntityGcConfig {
      pub enabled: bool,
      pub grace_seconds: u64,
      pub batch_cap: usize,
      pub dry_run: bool,
  }
  impl Worker for EntityGcWorker {
      fn run<'a>(&'a self, ctx: &'a WorkerContext) -> ... {
          if !self.config.enabled { return Ok(()); }
          // 1. rtxn — collect eligible entity ids.
          // 2. wtxn — tombstone each + audit row.
          // 3. commit.
      }
  }
```

### Reversal during grace

Entity tombstones are already soft (grace per §25/00). When
an inbound statement / relation is created targeting a
tombstoned-within-grace entity, the entity ops layer
(landing separately, but documented here) calls
`entity_restore(wtxn, entity_id)` which clears the flag and
writes an audit row.

The GC worker does NOT hard-delete; that's the **substrate's**
existing tombstone-grace-then-reclaim flow, which already
handles entities. v1's GC worker only tombstones.

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Off by default (this plan) | Safe; no surprise data loss | Operator must opt in | ✓ — matches spec |
| On by default with high grace (180 d) | Frees memory automatically | Surprises operators who use entities as persistent anchors | rejected |
| Inbound-count scan per tick | Accurate | O(n × m) scans | acceptable at 100K entity scale; index-driven counts are sub-µs each |
| Pre-computed inbound counter | Faster | Couples every write to a counter update; many edge cases | defer |
| Hard-delete (skip tombstone) | Less storage | Loses reversibility; spec mandates tombstone-first | follow spec |

## 6. Risks / open questions

- **Risk:** A new statement creation racing with the GC scan: scan sees zero inbound, but a stmt commits before our tombstone wtxn. **Mitigation:** the tombstone wtxn re-checks `inbound_reference_count` under the wtxn (single redb writer → consistent); if non-zero, skip with `still_referenced` metric.
- **Risk:** Entity GC tombstones an entity that's the subject of a tombstoned-but-within-grace statement. **Mitigation:** the inbound count includes pending-tombstone rows; only fully-removed references release the entity.
- **Open question:** Should "alias-only" mentions (entity_mentions tied to a memory) count as inbound? **Resolution:** yes — they're audit anchors. Include in `inbound_reference_count`.

## 7. Test plan

Unit tests in `entity_gc_ops`:
- `inbound_count_sums_statements_and_relations`.
- `scan_eligible_excludes_recent_entities` (created within grace).
- `scan_eligible_excludes_referenced_entities`.
- `scan_eligible_respects_batch_cap`.

Unit tests in `entity_gc.rs`:
- Disabled worker is no-op.
- Active worker tombstones eligible entities.
- Re-check inside wtxn skips if races caught one.

Integration test `brain-workers/tests/entity_gc_e2e.rs`:
- Create 10 entities; reference 5 from statements.
- Wait past grace (manipulate clock); run GC.
- Assert 5 tombstoned, 5 untouched.
- Create a new statement referencing one of the tombstoned-within-grace; assert restore-on-reference works.

## 8. Commit shape

```
feat(metadata,workers): 24.6 — entity GC worker (off by default)

- brain-metadata/src/entity_gc_ops.rs (new): inbound count +
  scan + tombstone helpers.
- brain-workers/src/workers/entity_gc.rs (new): off-by-default
  Low-priority worker; daily cadence.
- brain-workers/src/config.rs: entity-GC config keys.
- Tests: 4 unit (ops) + 3 unit (worker) + 1 E2E.

Verified: cargo zigbuild --target x86_64-unknown-linux-gnu
--workspace --tests; cargo clippy -- -D warnings.
```

## 9. Confirmation

1. **Off by default** — `BRAIN_ENTITY_GC_ENABLED` opt-in.
2. **Tombstone only** (no hard delete); substrate grace-then-reclaim flow handles eventual reclamation.
3. **Inbound count includes**: active statements (subject), active relations (from / to), entity_mentions.
4. **Race-safe**: wtxn re-checks inbound count before tombstoning.
5. **Re-check on inbound reference** restores tombstoned-within-grace entities (separate handler; documented here).
