# Sub-task 8.11 — Statistics update worker

**Spec:** `spec/11_background_workers/08_misc_workers.md` §3
**Phase doc:** `docs/phases/phase-08-workers.md` §8.11
**Done when:** Histograms of salience, edge degree, age etc. are updated; planner can query them.

---

## 1. Honest scope

Spec §3.1 lists per-shard stats:

| Stat | v1 plumbing |
| ---- | ----------- |
| `memory_count` | MEMORIES_TABLE iter ✓ |
| `tombstone_count` | SharedHnsw::tombstone_count() ✓ |
| `tombstone_ratio` | derived ✓ |
| `arena_used_bytes` | **no arena** — `None` |
| `arena_capacity_bytes` | **no arena** — `None` |
| `wal_size_bytes` | **no WAL hookup** — `None` |
| `metadata_size_bytes` | **no path on OpsContext** — `None` |
| `oldest_memory_age` | MEMORIES_TABLE iter ✓ (min `created_at`) |
| `newest_memory_age` | MEMORIES_TABLE iter ✓ (max `created_at`) |

The phase doc's bigger ask ("Histograms of salience, edge degree, age etc.") is Phase 9 admin tooling. v1 ships the **count + age** layer plus the framework for filesystem-stat-based fields. Stale fields are `Option<_>` with documented `None` semantics.

Out of scope:
- Filesystem `metadata.redb` size — `OpsContext` doesn't hold the path.
- WAL/arena byte counts — no v1 plumbing.
- Full histograms (salience distribution, edge degree distribution) — Phase 9.
- `ADMIN_STATS` wire handler — Phase 9.

---

## 2. The `Stats` snapshot

```rust
// crates/brain-workers/src/statistics.rs

#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub memory_count: u64,
    pub tombstone_count: u64,
    pub tombstone_ratio: f32,
    pub oldest_memory_age_nanos: Option<u64>,
    pub newest_memory_age_nanos: Option<u64>,
    /// v1: `None`. Phase 9 wires arena.
    pub arena_used_bytes: Option<u64>,
    pub arena_capacity_bytes: Option<u64>,
    /// v1: `None`. Phase 9 wires WAL.
    pub wal_size_bytes: Option<u64>,
    /// v1: `None`. Phase 9 wires metadata file path.
    pub metadata_size_bytes: Option<u64>,
    /// Worker wall-clock for this snapshot.
    pub computed_at_unix_nanos: u64,
}
```

The age fields are **nanos since creation** (now - created_at), not the raw timestamp, so consumers can render "X seconds ago" without an extra wall-clock read.

---

## 3. `StatisticsUpdateWorker`

```rust
pub struct StatisticsUpdateWorker {
    config: WorkerConfig,
    cache: Arc<RwLock<Stats>>,
}

impl StatisticsUpdateWorker {
    pub fn new() -> Self;
    pub fn with_config(self, cfg: WorkerConfig) -> Self;
    /// Read the most recent snapshot. Cheap (read-lock on Arc<RwLock>).
    pub fn snapshot(&self) -> Stats;
    /// Clone the cache handle so callers (Phase 9 admin handlers) can
    /// read the same Stats the worker writes.
    pub fn cache_handle(&self) -> Arc<RwLock<Stats>>;
}
```

The cache is `Arc<parking_lot::RwLock<Stats>>` so callers can hold a clone without affecting the worker. Default cycle: 5min (matches `WorkerKind::Statistics`).

### Cycle

```rust
async fn do_stats_cycle(&self, ctx) -> Result<usize, WorkerError> {
    let now = now_unix_nanos();
    let metadata = ctx.ops.executor.metadata.clone();
    let index = ctx.ops.executor.index.clone();

    let (memory_count, oldest, newest) = scan_memories(metadata)?;
    let tombstone_count = index.tombstone_count() as u64;
    let total = index.len() as u64;
    let tombstone_ratio = if total == 0 { 0.0 } else { tombstone_count as f32 / total as f32 };

    let new_stats = Stats {
        memory_count,
        tombstone_count,
        tombstone_ratio,
        oldest_memory_age_nanos: oldest.map(|c| now.saturating_sub(c)),
        newest_memory_age_nanos: newest.map(|c| now.saturating_sub(c)),
        arena_used_bytes: None,
        arena_capacity_bytes: None,
        wal_size_bytes: None,
        metadata_size_bytes: None,
        computed_at_unix_nanos: now,
    };
    *self.cache.write() = new_stats;
    Ok(1)   // one snapshot produced
}
```

`scan_memories` does one read-txn iteration of MEMORIES, accumulating count + min/max `created_at`.

---

## 4. File-by-file plan

| File | Action | Notes |
| ---- | ------ | ----- |
| `crates/brain-workers/src/statistics.rs` | NEW | Stats, StatisticsUpdateWorker |
| `crates/brain-workers/src/lib.rs` | Edit | Re-export |
| `crates/brain-workers/tests/statistics.rs` | NEW | ~10 tests |

No spec, wire, or other-crate changes.

---

## 5. Tests

### Cycle (6)
1. `empty_fixture_returns_zero_counts` — snapshot has `memory_count=0`, `tombstone_count=0`, ages `None`.
2. `seeded_memories_reflected_in_count` — 5 seeded → `memory_count=5`.
3. `tombstone_count_reflects_hnsw_state` — 4 inserts, 2 tombstoned via HNSW writer → `tombstone_count=2`, `tombstone_ratio=0.5`.
4. `age_fields_track_min_max_created_at` — seed two memories with explicit `created_at` 100s apart → `newest < oldest`.
5. `cache_updates_across_cycles` — first cycle with 1 memory, second cycle after seeding 2 more → cache reflects new count.
6. `phase_9_fields_stay_none` — `arena_used_bytes` / `wal_size_bytes` / `metadata_size_bytes` are `None`.

### Worker integration (3)
7. `worker_registers_with_correct_kind_and_default_cadence` — 5m interval.
8. `disabled_worker_via_config_does_not_update_cache` — cache stays at default `Stats`.
9. `cache_handle_observes_same_data_as_snapshot` — `cache_handle().read()` matches `snapshot()`.

### Edge case (1)
10. `computed_at_unix_nanos_advances` — two snapshots have monotonically-increasing `computed_at`.

Total: 10 tests.

---

## 6. Risks

| Risk | Mitigation |
| ---- | ---------- |
| Scanning MEMORIES every 5 min for large shards | Spec §3.3 caps cost at ~50 ms; bounded by `max_runtime` from config |
| Mutex held across `.await` | Cache update is synchronous; no `.await` between lock acquire and release |
| Returning `processed=1` per cycle (instead of e.g. `memory_count`) | Convention: `processed_total` tracks cycles-with-real-work; this matches the scheduler's existing semantics for cycle-completion workers |

---

## 7. Done criteria

- [ ] `Stats` + `StatisticsUpdateWorker` shipped.
- [ ] 10 tests pass first run.
- [ ] `cargo test --workspace` green; clippy + fmt clean.
- [ ] Commit subject: `feat(brain-workers): statistics update worker (sub-task 8.11)`.

~250 LOC impl + ~400 LOC tests. Small commit.
