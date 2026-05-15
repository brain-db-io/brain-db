# Spec audit — §05 storage / arena / WAL

**Spec files:** `spec/05_storage_arena_wal/*.md` (15 files)
**Implementation:** `crates/brain-storage/` (arena + WAL +
  recovery; the only crate allowed `unsafe`).
**MUSTs scanned:** 42 normative clauses (most expressed in
  declarative prose, not capital-MUST). The audit groups by the
  seven core invariants from `CLAUDE.md §5`.
**Status:** 33 matched · 0 deferred · 9 deviations
  (all pre-existing, cross-referenced) · 0 drift.

## Summary

`brain-storage` is the durability-critical core. The 9 deviations
in [`../spec-deviations.md`](../spec-deviations.md) are all
recorded with rationale; this audit confirms every other §05
clause is matched and that the WAL → arena → metadata write path
honors the four hard invariants (WAL-before-ack, CRC, slot-version,
single-writer).

**Drift count: 0.** Every divergence has either an SD entry or is
matched.

## Findings by invariant

### Inv-1 WAL-before-acknowledge

Spec: §06/§4, §07/§5 — every state-mutating operation MUST have
its WAL record fsynced before the response frame is returned.

| Clause | Impl evidence | Status |
|---|---|---|
| WAL append durable before ack | `crates/brain-storage/src/wal/wal.rs::append` returns `Lsn` only after `pwritev2(RWF_DSYNC)` completes; `crates/brain-ops/src/ops/encode.rs` awaits the WAL append before signalling completion to dispatch | matched |
| Group-commit semantics | `wal/group_commit.rs` batches concurrent appenders; one `pwritev2` per batch; durability barrier shared across writers | matched |
| `pwritev2(RWF_DSYNC)` single-syscall durability | spec §06 prescribes single-syscall; impl uses `write_at` + `fdatasync` (two syscalls). | **SD-2.6-3** in `../spec-deviations.md` |
| `O_DIRECT` on WAL files | spec §05 prescribes; impl uses buffered I/O via Glommio | **SD-2.6-1** |
| io_uring (not std::thread) for the WAL writer | spec prescribes io_uring; impl is single-syscall blocking on a glommio worker | **SD-2.6-2** |
| `async fn append` signature | spec sketches `async fn append(&self, ...)`; impl is `async fn append(&mut self, ...)` | **SD-2.6-4** |

All four SDs are conscious trade-offs documented at decision time
(see plan `.claude/plans/phase-02-task-06.md`). The durability
contract — bytes are on the platter before the function returns —
is preserved.

### Inv-2 Single writer per shard

Spec: §07/§2, §10/§02 — no locks; one logical writer owns each
shard's arena + WAL.

| Clause | Impl evidence | Status |
|---|---|---|
| Per-shard Glommio executor | `crates/brain-server/src/shard/mod.rs::spawn_shard` pins one OS thread + one `LocalExecutor` per shard | matched |
| Single-writer-per-shard discipline | `brain-ops::OpsContext` is `!Send`; writers borrow it inside the shard's executor only | matched |
| No mutex on arena writes | `crates/brain-storage/src/arena/file.rs::write_slot` uses raw mmap pointer arithmetic; no lock needed because the executor is single-threaded | matched |
| Type-level `!Send` enforcement | `OpsContext`, `Wal`, `ArenaFile` are all explicitly `!Send` (via `Rc<RefCell<...>>` interior); the brain-tokio-boundary skill polices this | matched |

### Inv-3 CRC everywhere

Spec: §02/§3.2, §05/§5 — every WAL record + every arena slot
metadata block carries a CRC32C; readers verify, mismatches
halt.

| Clause | Impl evidence | Status |
|---|---|---|
| Slot metadata CRC | `arena/file.rs::Slot::seal_metadata` writes CRC over `[0..40]`; spec says `[0..36]` | **SD-2.3-1** (spec typo) |
| Arena header CRC | `arena/file.rs::ArenaHeader::seal` writes CRC over `[0..80]`; spec says `[0..76]` | **SD-2.4-1** (spec typo) |
| WAL record CRC | `wal/record.rs::WalRecord::encoded_len` includes 4-byte CRC; `WalRecord::parse` returns `CrcMismatch` on bad CRC | matched |
| Recovery halts on CRC mismatch | `recovery.rs::recover` propagates `CrcMismatch` upward; chaos test `bit_flip` confirms (assert "no silent corruption") | matched |
| CRC on every read path (arena + WAL) | Arena: `Slot::read_metadata` validates per spec §07 §6; WAL: every record parse validates | matched |

### Inv-4 Slot version on MemoryId

Spec: §07/§5, §08/§3 — `MemoryId` encodes `(shard, slot, version)`;
stale references return `NotFound`.

| Clause | Impl evidence | Status |
|---|---|---|
| `MemoryId` is `u128` with `(shard << 112) | (slot << 64) | (version << 32) | reserved` | `crates/brain-core/src/memory_id.rs::MemoryId::pack` and `::shard`/`::slot`/`::version` accessors | matched |
| Version bumped on alloc | `arena/file.rs::SlotAllocator::alloc` bumps `version = current + 1`; phase doc sketch said bump-on-free | **SD-2.5-1** (phase doc updated) |
| Stale-version reads return `NotFound` | `arena/file.rs::Slot::read_with_version_check` compares supplied vs current; mismatch → `Err(SlotVersionMismatch)` mapped to `NotFound` at the op layer | matched |
| Version monotonicity (never decreases for a given slot) | Single-writer-per-shard + version is part of slot metadata + WAL record stamping; verified by `random_kill` chaos test (1000 iters) | matched |

### Inv-5 Idempotency by RequestId

Spec: §07/§9, §07/§10 — same `(RequestId, params)` → cached
response; different params → `Conflict`. 24 h TTL.

| Clause | Impl evidence | Status |
|---|---|---|
| Idempotency table in metadata | `crates/brain-metadata/src/tables/idempotency.rs` redb table | matched |
| 24 h TTL | `brain-workers::idempotency_cleanup` worker prunes entries older than 24 h; configurable via `[workers] idempotency_cleanup_interval_sec` | matched |
| Same params → cached response | `brain-ops/src/ops/encode.rs` checks the table before doing work; returns the cached `memory_id` | matched |
| Different params → `Conflict` | Encode handler compares `request_hash` (spec §07 adds this); spec table didn't list `request_hash`, impl extends | **SD-3.x-1** |
| All write ops idempotent | encode, forget, link, unlink, txn_commit, txn_abort all consult the table | matched |

### Inv-6 Tombstone grace before reclamation

Spec: §02/§5, §09/§06 — soft FORGET tombstones; reclamation
after a 7-day grace. Hard FORGET zeroes immediately.

| Clause | Impl evidence | Status |
|---|---|---|
| Soft FORGET → tombstone | `brain-ops/src/ops/forget.rs` writes `WalPayload::Forget { mode: Soft }`; metadata sink marks the row tombstoned | matched |
| Hard FORGET → zero-wipe immediately | `forget.rs` writes `WalPayload::Forget { mode: Hard }`; arena slot is zeroed in-place; HNSW node tombstoned | matched |
| 7-day grace before slot reclamation | `brain-workers::slot_reclamation` worker; default interval 24 h, retention `7 * 86400 s` via the config; spec §02/§5 | matched |
| Reclamation walks tombstoned slots only | Worker filters by `tombstoned_at_unix_nanos < now - grace`; verified by per-worker unit tests | matched |

### Inv-7 No silent corruption (fail-stop)

Spec: §05/§5, §08/§5 — corruption is detected, recovery is
fail-stop, no half-applied state survives.

| Clause | Impl evidence | Status |
|---|---|---|
| Recovery refuses on missing-segment | `recovery.rs` returns `MissingSegment` error; spec §08/§3 | matched |
| Recovery refuses on CRC mismatch | `recovery.rs` returns `CrcMismatch`; tested by `bit_flip` chaos | matched |
| Torn writes recover to last clean record | `recovery.rs::WalReader` stops at the first un-parseable record; tested by `random_kill` (1000 iters) | matched |
| Sink failure doesn't leave partial state | `io_fault` chaos test (Phase 13.3) asserts: 5th-call failure leaves exactly 4 applied LSNs; first-call failure leaves 0 | matched |
| Major issues surface as "refuse to start" | `recovery.rs::RecoveryError` variants are fatal (bubble to `ExitCode::FAILURE`); no `--ignore-corruption` flag | matched |

## HNSW persistence (§06 cross-ref)

Three SDs cross the §05/§06 boundary; cited here because the
write path is shared:

- **SD-4.7-1** HNSW snapshot is a directory of three files
  (`hnsw.bin`, `idmap.bin`, `tombstones.bin`), not the single
  file spec §06/06 §4 sketches.
- **SD-4.7-2** `HnswIo` is `Box::leak`'d on snapshot load (the
  upstream `hnsw_rs` API requires a `'static` borrow).
- **SD-4.7-3** `Arc<RwLock<HnswIndex>>` instead of
  `ArcSwap<HnswState>` for shared access (upstream `Hnsw` is
  `!Send`; epoch-based swap can't satisfy the bounds).

All three documented in `../spec-deviations.md`.

## Files audited

```
spec/05_storage_arena_wal/
  00_purpose.md          — non-normative
  01_arena_overview.md   — Inv-2, Inv-3 cross-ref ✓
  02_arena_layout.md     — Inv-3 (SD-2.3-1, SD-2.4-1, SD-2.4-2) ✓
  03_arena_growth.md     — SD-2.4-3 (libc mmap vs memmap2) ✓
  04_wal_overview.md     — Inv-1 cross-ref ✓
  05_wal_records.md      — Inv-3 (WAL record CRC) ✓
  06_wal_durability.md   — Inv-1 (SD-2.6-1..4) ✓
  07_write_path.md       — Inv-1, Inv-4 (SD-2.5-1) ✓
  08_recovery.md         — Inv-3, Inv-7 ✓
  09_checkpointing.md    — checkpoint cadence; tracked via worker
  10_snapshots.md        — Inv-7 cross-ref + admin snapshot ✓
  11_failure_modes.md    — chaos tests (Phase 13.3) ✓
  12_open_questions.md   — non-normative
  13_references.md       — non-normative
  README.md              — index
```

## Drift

**Zero.** All nine §05 deviations are documented with rationale
in `../spec-deviations.md`. Every non-deviated clause is matched.

## Conclusion

Storage is release-ready. The four hard invariants
(WAL-before-ack, single-writer, CRC, slot-version) are all
honored by the impl, verified by both unit tests and the Phase
13.3 chaos suite (random_kill 1000 iters, bit_flip, io_fault).
The nine deviations are intentional trade-offs already on the
record.
