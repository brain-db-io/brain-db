# Sub-task 8.13 — Snapshot worker

**Spec:** `spec/11_background_workers/08_misc_workers.md` §6 (and `spec/05_storage_arena_wal/10_snapshots.md` per phase doc — file doesn't exist, §08 §6 is canonical)
**Phase doc:** `docs/phases/phase-08-workers.md` §8.13
**Done when:** Periodic snapshot trigger writes a checkpoint and copies storage files to a snapshot directory.

---

## 1. Honest scope

Snapshot infrastructure v1 has:
- `brain_index::SharedHnsw::save_snapshot(dir, basename, ...)` — exists ✓
- `brain_storage::wal::write_checkpoint` — exists ✓ (but brain-ops's writer doesn't drive it)
- No arena snapshot — no arena.
- No metadata-redb snapshot helper — redb has its own copy-on-write but no public "save to dir" wrapper.
- No full-shard snapshot orchestration — Phase 9.

Spec §6 explicitly marks the worker as **"off by default — many deployments prefer external backup tooling. The substrate's built-in snapshot worker is a convenience."** Perfect fit for the pluggable-seam pattern.

Plan picks the same shape as 8.12 / 8.8 / 8.5:

- `SnapshotSource` trait: `take_snapshot() -> SnapshotId`, `list_snapshots() -> Vec<SnapshotDesc>`, `delete_snapshot(SnapshotId)`.
- `DisabledSnapshotSource` default returns `Disabled` on every method.
- `SnapshotWorker` (enabled=false per spec §6.2):
  - Take snapshot via source.
  - List snapshots, apply retention policy (count + age), delete the oldest excess.
  - Default retention: keep 7 newest, drop anything > 30 days.

---

## 2. Types

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapshotId(pub u64);  // monotonic id, source-assigned

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotDesc {
    pub id: SnapshotId,
    pub taken_at_unix_nanos: u64,
    pub size_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub max_count: usize,          // default 7 (spec §6.2)
    pub max_age: Duration,         // default 30d (spec §6.2)
}
```

`decide_retention(snapshots, now, policy) -> Vec<SnapshotId>` — pure function returning ids to delete. Sort by `taken_at`, keep newest `max_count`, drop snapshots older than `max_age`.

---

## 3. Source trait

```rust
#[derive(Debug, thiserror::Error)]
pub enum SnapshotSourceError {
    #[error("snapshot source disabled")]
    Disabled,
    #[error("snapshot source failed: {0}")]
    Failed(String),
}

pub type TakeFuture<'a>   = Pin<Box<dyn Future<Output = Result<SnapshotId,            SnapshotSourceError>> + Send + 'a>>;
pub type ListFuture<'a>   = Pin<Box<dyn Future<Output = Result<Vec<SnapshotDesc>,     SnapshotSourceError>> + Send + 'a>>;
pub type DeleteFuture<'a> = Pin<Box<dyn Future<Output = Result<(),                    SnapshotSourceError>> + Send + 'a>>;

pub trait SnapshotSource: Send + Sync + 'static {
    fn take_snapshot(&self) -> TakeFuture<'_>;
    fn list_snapshots(&self) -> ListFuture<'_>;
    fn delete_snapshot(&self, id: SnapshotId) -> DeleteFuture<'_>;
}

pub struct DisabledSnapshotSource;
```

---

## 4. `SnapshotWorker`

```rust
pub struct SnapshotWorker {
    config: WorkerConfig,            // enabled=false by default per WorkerKind::Snapshot
    retention: RetentionPolicy,
    source: Arc<dyn SnapshotSource>,
}

impl SnapshotWorker {
    pub fn new(source: Arc<dyn SnapshotSource>) -> Self;
    pub fn with_config(self, cfg: WorkerConfig) -> Self;
    pub fn with_retention(self, p: RetentionPolicy) -> Self;
}
```

Cycle:
```rust
async fn do_cycle(&self, ctx) -> Result<usize, WorkerError> {
    if !self.config.enabled { return Ok(0); }
    let _new_id = match self.source.take_snapshot().await {
        Ok(id) => id,
        Err(SnapshotSourceError::Disabled) => return Ok(0),
        Err(SnapshotSourceError::Failed(e)) => return Err(WorkerError::Ops(format!("snapshot: {e}"))),
    };
    let snapshots = match self.source.list_snapshots().await {
        Ok(v) => v,
        Err(SnapshotSourceError::Disabled) => return Ok(1),
        Err(SnapshotSourceError::Failed(e)) => return Err(WorkerError::Ops(format!("snapshot list: {e}"))),
    };
    let now_nanos = now_unix_nanos();
    let to_delete = decide_retention(&snapshots, now_nanos, self.retention);
    let mut deleted = 0;
    for id in to_delete {
        if ctx.is_shutdown() { break; }
        match self.source.delete_snapshot(id).await {
            Ok(()) => deleted += 1,
            Err(SnapshotSourceError::Disabled) => break,
            Err(SnapshotSourceError::Failed(e)) => {
                return Err(WorkerError::Ops(format!("snapshot delete: {e}")));
            }
        }
    }
    // Return "1" for the new snapshot + count of retention deletions.
    Ok(1 + deleted)
}
```

Returns `1 + deleted` so `processed_total` tracks meaningful work per cycle.

---

## 5. File-by-file plan

| File | Action | Notes |
| ---- | ------ | ----- |
| `crates/brain-workers/src/snapshot.rs` | NEW | Types, source trait, decide_retention, worker |
| `crates/brain-workers/src/lib.rs` | Edit | Re-export |
| `crates/brain-workers/tests/snapshot.rs` | NEW | ~13 tests |

No spec / wire / other-crate changes.

---

## 6. Tests

### `decide_retention` (5)
1. `empty_snapshots_returns_empty`.
2. `under_max_count_keeps_everything` — 3 snapshots, max_count=7 → empty.
3. `over_max_count_drops_oldest` — 10 snapshots, max_count=7 → 3 oldest ids returned.
4. `over_max_age_drops_old_regardless_of_count` — 3 snapshots, two older than max_age → those two returned even though under count cap.
5. `count_and_age_combined` — 10 snapshots, 5 older than max_age, max_count=7 → all 5 old ones returned (count cap doesn't matter; they're already past age).

### Source surface (3)
6. `disabled_source_returns_disabled_on_every_method`.
7. `stub_source_take_returns_monotonic_id`.
8. `failed_source_propagates_as_worker_error`.

### Cycle (3)
9. `disabled_worker_via_config_does_not_take` (the `enabled=false` default means an explicit `enable=true` test is needed too).
10. `enabled_worker_takes_snapshot_and_reports_count`.
11. `enabled_worker_deletes_old_snapshots_per_retention`.

### Worker integration (2)
12. `worker_registers_with_correct_kind_and_default_cadence_disabled`.
13. `default_config_has_enabled_false_per_spec`.

Total: 13 tests.

---

## 7. Risks

| Risk | Mitigation |
| ---- | ---------- |
| `enabled=false` by default means worker is essentially dormant in v1 | Spec §6.2 is explicit; tests bump `enabled=true` to exercise the cycle |
| Spec §1 (recap) says snapshot triggers a checkpoint first | v1's source impl will own that ordering when Phase 9 wires it; the trait surface stays simple (take_snapshot is one call) |
| Returning `1 + deleted` mixes "took one snapshot" with "deleted N" | Convention: `processed_total` is "units of work per cycle"; documented in the trait doc |

---

## 8. Done criteria

- [ ] Types + trait + decide_retention + SnapshotWorker shipped.
- [ ] 13 tests pass first run.
- [ ] `cargo test --workspace` green; clippy + fmt clean.
- [ ] Commit subject: `feat(brain-workers): snapshot worker (sub-task 8.13)`.

~350 LOC impl + ~450 LOC tests. Single commit.

Out of scope (Phase 9): brain_storage / brain_index snapshot orchestration, ADMIN_SNAPSHOT_CREATE / RESTORE handlers, snapshot file format, encryption.
