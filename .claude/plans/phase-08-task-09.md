# Sub-task 8.9 — Edge scrub worker

**Spec:** `spec/11_background_workers/08_misc_workers.md` §1
**Phase doc:** `docs/phases/phase-08-workers.md` §8.9
**Done when:** Edges referencing reclaimed slots (whose version no longer matches) are removed.

---

## 1. Scope

Edge scrub cleans up **dangling edge entries** left behind by slot reclamation (sub-task 8.7). Spec §1.1: when memory M is reclaimed, slot reclamation removes entries keyed at M (`EDGES_OUT[M, *, *]` and `EDGES_IN[M, *, *]`), but the **paired entries from live memories** are left behind:

- `EDGES_OUT[X, kind, M]` — X is alive, edge target is dead M.
- `EDGES_IN[X, kind, M]` — X is alive, edge source is dead M.

The edge-scrub worker iterates both tables and removes rows whose `source` or `target` no longer exists in `MEMORIES`.

In:
- Cycle scans `EDGES_OUT` for each cycle, bounded by batch_size + max_runtime. Removes any row whose source or target is gone (plus the corresponding mirror in `EDGES_IN`).
- Cursor across cycles (so a 3-hour full pass works even with small batch_size).
- Defensive: also check that the row's source is alive. Spec §1 assumes reclamation cleaned `EDGES_OUT[M, *, *]` when M died, but if a bug skipped it, we'll catch the orphan here.
- `EDGES_IN` scan in the same cycle catches dangling incoming edges from dead sources.

Out:
- Pre-computing scrub work at reclamation time (spec §1.4 — "Not implemented in v1; periodic full-scan is simpler").
- `ADMIN_EDGE_SCRUB` manual trigger (Phase 9).
- Audit-log emission (Phase 9).

---

## 2. The cycle

```rust
async fn do_scrub_cycle(&self, ctx) -> Result<usize, WorkerError> {
    let cfg = self.config();
    let metadata = ctx.ops.executor.metadata.clone();
    let cursor = self.cursor.lock();   // Option<EdgeKey>
    let mut removed = 0usize;
    let started = Instant::now();

    // Scan EDGES_OUT in one write txn — we may delete as we go.
    // Use a two-phase pattern (collect candidates with a read txn,
    // then mutate in a wtxn) to keep the mutex-held wtxn small.

    // Phase A: read EDGES_OUT in range above cursor, check endpoint
    //          existence against MEMORIES, collect orphans up to
    //          batch_size.
    let orphans = collect_orphans_edges_out(metadata, *cursor, cfg.batch_size, started, cfg.max_runtime)?;
    let scanned_to_end_out = orphans.scanned_to_end;

    // Phase B: one wtxn deletes the orphans from both EDGES_OUT and
    //          EDGES_IN (mirror).
    if !orphans.victims.is_empty() {
        let mut db = metadata.lock();
        let wtxn = db.write_txn()?;
        {
            let mut out = wtxn.open_table(EDGES_OUT_TABLE)?;
            let mut in_ = wtxn.open_table(EDGES_IN_TABLE)?;
            for (source, kind, target) in &orphans.victims {
                out.remove(&(source.to_be_bytes(), *kind, target.to_be_bytes()))?;
                in_.remove(&(target.to_be_bytes(), *kind, source.to_be_bytes()))?;
                removed += 1;
            }
        }
        wtxn.commit()?;
    }

    // Phase C: same flow for EDGES_IN, catching dangling incoming
    //          edges whose source is dead.
    let orphans_in = collect_orphans_edges_in(...)?;
    // ... (mirror to EDGES_OUT, same idempotent delete)

    // Cursor advance.
    *self.cursor.lock() = if scanned_to_end_out && scanned_to_end_in {
        None  // wrap
    } else {
        last_scanned
    };
    Ok(removed)
}
```

For v1 simplicity the cursor only tracks `EDGES_OUT` position; each cycle does a full pass of `EDGES_IN` (typically smaller after slot reclamation). This is a known v1 trade-off: simpler than two cursors, slightly more work per cycle. Documented.

---

## 3. Helpers

```rust
pub fn is_memory_alive(rtxn: &ReadTransaction, id: MemoryId) -> Result<bool, ...>;
```

A small helper to keep both phases consistent.

---

## 4. `EdgeScrubWorker`

```rust
pub struct EdgeScrubWorker {
    config: WorkerConfig,
    cursor: parking_lot::Mutex<Option<EdgeKey>>,
}

impl EdgeScrubWorker {
    pub fn new() -> Self;
    pub fn with_config(self, cfg: WorkerConfig) -> Self;
}
```

Standard surface; `WorkerKind::EdgeScrub` defaults to 30m interval, batch_size 5_000, max_runtime 5s.

---

## 5. File-by-file plan

| File | Action | Notes |
| ---- | ------ | ----- |
| `crates/brain-workers/src/edge_scrub.rs` | NEW | EdgeScrubWorker, helpers |
| `crates/brain-workers/src/lib.rs` | Edit | Re-export |
| `crates/brain-workers/tests/edge_scrub.rs` | NEW | ~12 tests |

No spec, no wire, no other-crate changes.

---

## 6. Tests

### Cycle (8)
1. `live_to_live_edge_is_kept` — encode A + B, link, scrub → edge stays.
2. `edge_to_dead_target_removed_from_out` — seed (alive_src, kind, dead_tgt) in EDGES_OUT; scrub → removed.
3. `edge_to_dead_target_mirror_in_also_removed` — same setup, plus the mirror in EDGES_IN; scrub → both gone.
4. `edge_from_dead_source_removed_from_in` — seed (alive_tgt, kind, dead_src) in EDGES_IN; scrub → removed.
5. `both_endpoints_dead_edge_removed` — defensive: edge where neither endpoint exists.
6. `batch_size_caps_per_cycle` — seed 50 orphans, batch_size=10 → 10 removed.
7. `cursor_advances_across_cycles` — seed 30 orphans, batch_size=10 → 3 cycles cover all 30.
8. `mixed_live_and_orphan_only_orphans_removed`.

### Worker integration (3)
9. `worker_registers_with_correct_kind_and_default_cadence` — 30m interval.
10. `disabled_worker_via_config_does_not_scrub`.
11. `cycle_processed_count_feeds_metrics`.

### Edge cases (1)
12. `empty_edge_tables_cycle_is_noop`.

Total: 12 tests.

---

## 7. Risks

| Risk | Mitigation |
| ---- | ---------- |
| Holding metadata mutex too long during scan | Two-phase: read txn collects candidates (mutex released), wtxn deletes (mutex re-acquired, brief) |
| Cursor lost on restart → re-scan | Spec §11/00 §10 explicitly allows; idempotent (already-deleted rows are no-ops) |
| EDGES_IN orphans not picked up if EDGES_OUT cursor is in the middle | Each cycle scans EDGES_IN fully; v1 trade-off documented |
| Mutex held across `.await` | Two phases each scope their guard; `yield_now()` between phases only |

---

## 8. Done criteria

- [ ] `EdgeScrubWorker` in `edge_scrub.rs`.
- [ ] 12 tests pass first run.
- [ ] `cargo test --workspace` green; clippy + fmt clean.
- [ ] Commit subject: `feat(brain-workers): edge scrub worker (sub-task 8.9)`.

~350 LOC impl + ~500 LOC tests, single commit.
