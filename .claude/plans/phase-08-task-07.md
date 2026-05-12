# Sub-task 8.7 — Slot reclamation worker

**Spec:** `spec/11_background_workers/06_slot_reclamation.md`
**Phase doc:** `docs/phases/phase-08-workers.md` §8.7
**Done when:** Tombstones past their grace period have their slots reclaimed (free list updated, version bumped).

---

## 1. Honest scope

Two v1 gaps to acknowledge up front:

1. **No arena, no slot allocator, no free list.** Spec §11/§3 step 6 "push slot to free list" and §3 step 5's "increment slot version" both target arena infrastructure that doesn't exist yet (Phase 9). v1 reclamation can only do the redb-metadata side: delete the MEMORIES row + adjacent edges. Slot-version bumping is moot because v1 doesn't reuse slots — each ENCODE mints a fresh slot from a monotonic counter.

2. **FORGET doesn't stamp `tombstoned_at_unix_nanos`.** v1's `do_forget` marks HNSW + the in-process tombstone set but leaves the metadata row's `tombstoned_at_unix_nanos: Option<u64>` as `None`. Without this stamp the worker has no way to discover which rows are past their grace period. This is a **real v1 bug** surfaced by 8.7 — the consolidation worker's "tombstoned_at filter" check (8.4) silently never excluded freshly-FORGET'd memories.

**Two fixes land here:**

- **Bug fix:** stamp `tombstoned_at_unix_nanos = Some(now)` on the MEMORIES row in `record_and_return_forget` (only when outcome is `Tombstoned`).
- **Worker:** scan for rows with `tombstoned_at_unix_nanos < (now - grace)`, delete the row + adjacent edges in one wtxn each.

The slot-version bump and free-list push are documented as Phase 9 follow-ups.

---

## 2. The cycle

```rust
async fn do_reclaim_cycle(&self, ctx) -> Result<usize, WorkerError> {
    let cfg = self.config();
    let cutoff_nanos = now_unix_nanos().saturating_sub(grace_nanos);
    let started = Instant::now();

    // 1. Collect candidate ids (read txn).
    let candidates = collect_reclamation_candidates(metadata, cutoff_nanos, cfg.batch_size)?;
    if candidates.is_empty() { return Ok(0); }

    // 2. Per spec §8: one wtxn per memory (smaller lock duration).
    let mut reclaimed = 0;
    for id in candidates {
        if started.elapsed() >= cfg.max_runtime { break; }
        if ctx.is_shutdown() { break; }
        if reclaim_one(metadata, id, cutoff_nanos)? {
            reclaimed += 1;
        }
        tokio::task::yield_now().await;
    }
    Ok(reclaimed)
}

fn reclaim_one(metadata, id, cutoff) -> Result<bool, WorkerError> {
    let mut db = metadata.lock();
    let wtxn = db.write_txn()?;
    let did_remove;
    {
        let mut memories = wtxn.open_table(MEMORIES_TABLE)?;
        // Re-check: still tombstoned + still past cutoff (race guard).
        let Some(meta) = memories.get(id.to_be_bytes())?.map(|a| a.value()) else {
            return Ok(false);
        };
        let Some(ts) = meta.tombstoned_at_unix_nanos else {
            return Ok(false);   // un-tombstoned between scan and now
        };
        if ts >= cutoff { return Ok(false); }

        // Delete the row.
        memories.remove(id.to_be_bytes())?;
        did_remove = true;

        // Delete adjacent edges. Spec §6 — both out (source=id) and
        // in (target=id) directions. Dangling references the other
        // way are left for the edge-scrub worker (8.9).
        let mut out = wtxn.open_table(EDGES_OUT_TABLE)?;
        let mut in_ = wtxn.open_table(EDGES_IN_TABLE)?;
        purge_adjacent_edges(&mut out, &mut in_, id)?;
    }
    wtxn.commit()?;
    Ok(did_remove)
}
```

`purge_adjacent_edges` ranges `EDGES_OUT` from `(id, 0, 0..)` to `(id, 0xFF, 0xFF..)` and removes each; same for `EDGES_IN`.

---

## 3. FORGET stamping (bug fix)

```rust
// crates/brain-ops/src/writer.rs — inside record_and_return_forget,
// for the Tombstoned outcome only:

if matches!(outcome, ForgetOutcome::Tombstoned) {
    let mut memories_t = wtxn.open_table(MEMORIES_TABLE)?;
    if let Some(access) = memories_t.get(op.memory_id.to_be_bytes())? {
        let mut meta = access.value();
        drop(access);
        if meta.tombstoned_at_unix_nanos.is_none() {
            meta.tombstoned_at_unix_nanos = Some(created_at);
            memories_t.insert(op.memory_id.to_be_bytes(), meta)?;
        }
    }
}
```

Same wtxn as the idempotency entry insert, so atomic with the FORGET commit. Spec §07/03 §1 already shows the field; we just weren't writing it.

Verify no existing tests break: the per-handler forget tests check `was_already_forgotten` / `memory_id`; the §16/01 §12 correctness test checks RECALL filters tombstones (already works via HNSW). One side-effect: the consolidation worker's "exclude tombstoned" filter now works correctly — already covered by an existing test.

---

## 4. `SlotReclamationWorker`

```rust
pub const DEFAULT_FORGET_GRACE: Duration = Duration::from_secs(7 * 24 * 3600);

pub struct SlotReclamationWorker {
    config: WorkerConfig,    // 10m / batch 1000 / max_runtime 5s
    grace_period: Duration,  // 7d default
}

impl SlotReclamationWorker {
    pub fn new() -> Self;
    pub fn with_config(self, cfg: WorkerConfig) -> Self;
    pub fn with_grace_period(self, d: Duration) -> Self;
}
```

Standard surface; same pattern as the other workers.

---

## 5. File-by-file plan

| File | Action | Notes |
| ---- | ------ | ----- |
| `crates/brain-ops/src/writer.rs` | Edit | Stamp `tombstoned_at_unix_nanos` in `record_and_return_forget` (Tombstoned outcome only) |
| `crates/brain-workers/src/slot_reclaim.rs` | NEW | `SlotReclamationWorker`, helpers, defaults |
| `crates/brain-workers/src/lib.rs` | Edit | Re-export |
| `crates/brain-workers/tests/slot_reclaim.rs` | NEW | ~13 tests |

No spec, no wire, no other crate changes.

---

## 6. Tests (`tests/slot_reclaim.rs`)

### Cycle behaviour (8)
1. `tombstoned_past_grace_is_reclaimed` — seed row with `tombstoned_at = now - 8d`, grace=7d → row removed.
2. `tombstoned_within_grace_is_kept` — seed row with `tombstoned_at = now - 1d` → not reclaimed.
3. `active_memory_never_reclaimed` — `tombstoned_at = None` → not touched.
4. `multiple_eligible_rows_all_reclaimed_within_batch_size`.
5. `batch_size_caps_per_cycle` — seed 50 eligible, batch_size=10 → 10 reclaimed.
6. `adjacent_out_edges_purged` — seed row + outgoing edges; after reclaim, EDGES_OUT has no rows with that source.
7. `adjacent_in_edges_purged` — seed row + incoming edges from other memories; after reclaim, EDGES_IN has no rows with that target.
8. `dangling_edges_other_direction_are_left` — outgoing edge from other memory pointing to reclaimed memory: EDGES_OUT entry survives (spec §6 — edge-scrub worker's job).

### Stamping (FORGET integration) (2)
9. `forget_stamps_tombstoned_at_unix_nanos` — call dispatch FORGET; read row; assert field set.
10. `forget_replay_does_not_overwrite_stamp` — call FORGET twice; second call must not bump the timestamp.

### Worker integration (3)
11. `worker_registers_with_correct_kind_and_default_cadence` — 10m interval.
12. `disabled_worker_via_config_does_not_reclaim`.
13. `custom_grace_period_honoured`.

Total: 13.

---

## 7. Risks

| Risk | Mitigation |
| ---- | ---------- |
| Race: row un-tombstoned between scan and reclaim_one | `reclaim_one` re-checks `tombstoned_at_unix_nanos.is_some() && ts < cutoff` inside the wtxn |
| Mutex held across `.await` | Inner loop releases the lock between reclamations; `yield_now()` outside the lock |
| EDGES_IN may have hundreds of rows for popular memories | Bounded by batch_size + max_runtime; spec §8 accepts per-memory wtxn cost |
| Stamping a row that no longer exists in `record_and_return_forget` (rare: external concurrent delete) | Stamping uses `if let Some(...)` so missing rows silently skip |
| Forget's existing tests assume no metadata write beyond idempotency | Confirmed: tests check wire response shape, not metadata-write count |

---

## 8. Out of scope (Phase 9)

- Free-list / arena slot reuse (no arena in v1).
- Separate SLOT_VERSIONS table or `slot_version` bump beyond what's in MemoryId (no reuse).
- HNSW node cleanup (spec §7 — left for maintenance worker to rebuild).
- `force_reclaim_now=true` flag on FORGET (spec §15) — wire change.
- Audit log emission (spec §17).

---

## 9. Done criteria

- [ ] `record_and_return_forget` stamps `tombstoned_at_unix_nanos`.
- [ ] `SlotReclamationWorker` + helpers in `slot_reclaim.rs`.
- [ ] 13 tests pass first run.
- [ ] `cargo test --workspace` green; clippy + fmt clean.
- [ ] Commit subject: `feat(brain-workers,brain-ops): slot reclamation worker (sub-task 8.7)`.

~350 LOC impl + ~500 LOC tests, single commit.
