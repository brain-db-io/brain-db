# Plan: Phase 2 — Storage (Arena + WAL + Recovery)

**Status:** awaiting-confirmation (revised for Linux + Glommio day-1)
**Date:** 2026-05-10
**Author:** Claude (autonomous)
**Estimated commits:** 12–14 (one per sub-task; 2.8 / 2.10 may split)

---

## 1. Scope

Build the durable storage layer for `brain-storage`:

- A memory-mapped vector arena (`arena.bin`) with the spec'd 4 KiB header and 1600-byte slots.
- A write-ahead log with rotating segments, group commit via `pwritev2(RWF_DSYNC)`, and crash recovery.
- A `recover(...)` driver that applies WAL records past the last checkpoint, idempotently.
- A `MetadataSink` trait (real impl in Phase 3, test fake here).
- A 1000-iteration random-kill recovery test confirming durability of every acked operation.

**Acceptance**: kill the shard mid-workload at any byte offset; on restart, every operation that returned `Ok(lsn)` is visible, no other operations are visible, arena + WAL are consistent.

### Linux + Glommio from day one

Per AUTONOMY §22, `brain-storage` is **Linux-only**. The crate gates everything behind `#[cfg(target_os = "linux")]`; non-Linux hosts get a `compile_error!` pointing at `DEV_SETUP.md`. We use libc Linux syscalls directly (`pwritev2(RWF_DSYNC)`, `mmap`, `mremap`, `fallocate`, `madvise`) — no portability shims, no `std::fs` fallbacks.

**Glommio integration**: storage's public API (`Wal::append`) returns `impl Future`. In Phase 2 the future is driven by simple async machinery (the test runtime); in Phase 9, the same API is wired into Glommio's `LocalExecutor` without touching the storage code. The platform is locked from day 1; the runtime wiring is one commit at the boundary.

**What that means for me (Claude on darwin)**:

- `cargo build / test / clippy` for `brain-storage` cannot run natively on macOS — the `compile_error!` fires by design.
- I run cargo via the Linux dev container per `DEV_SETUP.md`. CI is the authoritative test gate.
- `cargo check --target x86_64-unknown-linux-gnu` validates compilation locally before I push for a container test cycle.

**Out of scope (deferred):**

- Glommio executor wiring at I/O call sites. Phase 9 swaps `tokio::spawn`-flavored helpers for `glommio::spawn_local` and replaces the test runtime; storage API stays put.
- redb integration for metadata. The `MetadataSink` trait abstracts it; Phase 3 implements.
- Snapshot / backup workflows.
- ANN-index persistence (Phase 4) and embedding-model fingerprint validation (Phase 5).

## 2. Spec references

- `spec/01_system_architecture/05_hardware.md` §1.1 — locks Linux + libc syscalls; rejects portable abstractions.
- `spec/05_storage_arena_wal/00_purpose.md` — invariants overview.
- `spec/05_storage_arena_wal/01_arena_overview.md` — arena role.
- `spec/05_storage_arena_wal/02_arena_layout.md` — 4 KiB header, 1600-byte slots, 1536-byte vector + 64-byte metadata, header CRC over bytes [0..76]. Storage = **little-endian**.
- `spec/05_storage_arena_wal/03_arena_growth.md` — `fallocate` + `mremap` policy, doubling.
- `spec/05_storage_arena_wal/04_wal_overview.md` — segments + LSN.
- `spec/05_storage_arena_wal/05_wal_records.md` — record framing, kinds, payload schemas.
- `spec/05_storage_arena_wal/06_wal_durability.md` — `pwritev2(RWF_DSYNC)`, group-commit window, ack-after-fsync invariant.
- `spec/05_storage_arena_wal/07_write_path.md` — encode/forget/link write paths.
- `spec/05_storage_arena_wal/08_recovery.md` — recovery algorithm + torn-tail handling.
- `spec/05_storage_arena_wal/09_checkpointing.md` — checkpoint records + replay-start LSN.
- `spec/05_storage_arena_wal/11_failure_modes.md` — halt vs. log-and-continue.
- `spec/02_data_model/03_identifiers.md` §2.1 — `MemoryId` slot/version interplay (Phase 1 settled).
- `spec/16_benchmarks_acceptance/06_durability_criteria.md` — random-kill acceptance.

Binding constraints (CLAUDE.md §5 invariants this phase makes operational):

- **#1 WAL-before-acknowledge** — `Wal::append` returns `Ok(lsn)` only after `pwritev2(RWF_DSYNC)` returns.
- **#2 Single writer per shard** — `Wal` and `Arena` are `!Send` per-shard handles.
- **#3 CRC everywhere** — every WAL record + every arena slot carries CRC32C; mismatch → halt.
- **#4 Slot version on `MemoryId`** — reclaim bumps version; fresh writers stamp the new version.
- **#5 Idempotency by RequestId** — recovery is idempotent on `(lsn, request_id)` (Phase 3 wires the dedupe table; Phase 2 designs the trait).
- **#6 Tombstone grace** — soft FORGET preserves slot bytes for the grace window; hard FORGET zeroes immediately.
- **#7 No silent corruption** — CRC mismatch on replay → `StorageError::Corruption`; never overwrite a stored CRC.

## 3. External validation

Web-searched (May 2026):

### `memmap2 0.9+`

- **Source:** [docs.rs/memmap2](https://docs.rs/memmap2). Used by redb, sled, and other Rust databases.
- **API:** `Mmap` / `MmapMut` / `MmapRaw`; `MmapOptions::map(&file)`; `Mmap::remap(new_size, advise)` calls Linux `mremap(2)` with the supplied `RemapOptions`.
- **Constraint:** all file-backed map constructors are `unsafe` because UB if the underlying file is modified out-of-process while mapped. Brain controls the file lifetime per shard; the unsafe contract is satisfied.
- **Choice:** `memmap2` over rolling our own — well-audited, tracks Linux `mremap` semantics directly.

### `pwritev2(RWF_DSYNC)`

- **Source:** [Linux pwritev2(2)](https://man.archlinux.org/man/pwritev2.2.en); [libc::pwritev2](https://docs.diesel.rs/master/libc/fn.pwritev2.html); [Rustix PR #489](https://github.com/bytecodealliance/rustix/pull/489) for cross-libc support.
- **Kernel requirement:** ≥ 4.7 — universal on supported targets (we require 5.15+).
- **Group-commit pattern (validated against [QEMU file-posix.c](https://www.mail-archive.com/qemu-block@nongnu.org/msg119106.html)):** queue pending records, drain periodically (or when full), single `pwritev2(RWF_DSYNC)` for the batch's iovecs, signal all waiters via oneshot channels.

### Workspace dep additions

- `memmap2 = "0.9"` — new workspace dep, used only by `brain-storage`.
- Direct `libc = "0.2"` workspace dep — already transitive; promote so we can call `pwritev2` and reference `RWF_DSYNC` directly.
- `nix = "0.29"` — *optional*, for nicer wrappers; deferred unless libc gets awkward.

## 4. Architecture sketch

```text
crates/brain-storage/src/
├── lib.rs                       cfg(linux)-gated; non-linux compile_error!
├── error.rs                     StorageError; From into brain_core::Error
├── arena/
│   ├── mod.rs                   Arena public type
│   ├── header.rs                4 KiB ArenaHeader (Pod, header_crc32c)
│   ├── slot.rs                  1600-byte Slot (Pod, vector + metadata + crc)
│   ├── file.rs                  ArenaFile: open / mmap / mremap-grow
│   └── allocator.rs             SlotAllocator: free list + version bump
├── wal/
│   ├── mod.rs                   Wal public type
│   ├── record.rs                WalRecord framing + Lsn
│   ├── kinds.rs                 WalRecordKind variants per spec §05/05
│   ├── segment.rs               WalSegment writer (no fsync)
│   ├── reader.rs                WalReader iterator over a directory
│   ├── group_commit.rs          GroupCommitter; pwritev2(RWF_DSYNC) batched
│   └── checkpoint.rs            Checkpoint write + locate-latest
├── recovery.rs                  Recovery driver + MetadataSink trait
└── tests/
    └── random_kill.rs           1000-iter durability test
```

`brain-storage/src/lib.rs` opens with:

```rust
#![cfg(target_os = "linux")]
// On non-Linux: see DEV_SETUP.md. brain-storage uses io_uring,
// O_DIRECT, pwritev2(RWF_DSYNC), and mremap; it does not compile
// outside Linux by design (spec §01/05 §1.1).
#![cfg_attr(
    not(target_os = "linux"),
    deny(non_existent_attributes_to_force_a_compile_error_above)
)]
// ... rest of module declarations ...
```

(The `cfg_attr` redundancy is belt-and-suspenders — `#![cfg(target_os = "linux")]` already strips the crate on non-Linux. We pair it with a sibling `lib_stub.rs` that contains a `compile_error!` so the crate emits a *friendly* error instead of "crate is empty".)

Public surface (target end-of-phase, all Linux):

```rust
pub use arena::{Arena, ArenaHeader, Slot, SlotIndex, SlotMeta, SlotAllocator};
pub use error::StorageError;
pub use recovery::{recover, MetadataSink, RecoveryReport};
pub use wal::{Lsn, Wal, WalReader, WalRecord, WalRecordKind};

pub struct Arena {
    file: ArenaFile,                 // mmap'd; !Send
    allocator: SlotAllocator,
}

pub struct Wal {
    segments: SegmentSet,
    committer: GroupCommitter,       // owns the fsync queue
    next_lsn: Lsn,
}

impl Wal {
    pub async fn append(&self, record: WalRecord) -> Result<Lsn, StorageError>;
    pub fn reader(&self) -> WalReader;
}
```

Both `Arena` and `Wal` are `!Send` — single-writer-per-shard. The `async fn append` returns a future that's `!Send` too (matches `brain-glommio-rules`).

## 5. Sub-task breakdown

| # | Sub-task | Plan needed (per AUTONOMY §21)? |
|---|---|---|
| 2.1 | `Lsn` newtype + `WalRecord` framing | trivial — proceed with one-line summary |
| 2.2 | `WalRecordKind` enum | substantial — payload schemas per spec §05/05; **plan** |
| 2.3 | Arena slot byte layout | substantial — Pod + 4 KiB header + 1600-byte slot; **plan** |
| 2.4 | Arena file: open / mmap / `mremap`-grow | substantial — first `unsafe` outside protocol; **plan** |
| 2.5 | Slot allocator + version bumping | substantial — invariant #4; **plan** |
| 2.6 | WAL segment writer (no fsync) | trivial — proceed |
| 2.7 | `WalReader` over a directory | substantial — torn-tail handling; **plan** |
| 2.8 | Group commit with `pwritev2(RWF_DSYNC)` | substantial — invariant #1, libc syscall; **plan** |
| 2.9 | `Wal` public type | trivial — composition; proceed |
| 2.10 | Recovery driver | substantial — algorithm + idempotency; **plan** (likely splits 2 commits) |
| 2.11 | Random-kill recovery test | substantial — proptest harness, 1000 iter; **plan** |
| 2.12 | Checkpoint writer | trivial — proceed |

Substantial sub-tasks each get `.claude/plans/phase-02-task-NN.md` before implementation. Trivial sub-tasks proceed with a one-liner.

## 6. Trade-offs considered

| Alternative | Verdict |
|---|---|
| **Chosen:** Linux-only crate, libc syscalls, Glommio API shape from day 1, runtime wiring deferred to Phase 9. | ✓ Spec-faithful; no portable shims to throw away; testable in CI + Linux containers. |
| Storage code uses Glommio's `DmaFile` directly (Phase 2). | rejected — couples storage tests to a Glommio executor; the runtime call point is in `brain-server`. Storage stays runtime-agnostic, just Linux-syscall-aware. |
| Cross-platform `std::fs` shims today, swap to libc later. | rejected per AUTONOMY §22 — generates rework when Phase 9 lands; we'd have to revisit every test. |
| Single big WAL file (no segments). | rejected — segments are spec'd (§05/04). |
| Synchronous `Wal::append` API. | rejected — group commit must batch. |
| `Wal` and `Arena` `Send + Sync`. | rejected — invariant #2; Glommio integration depends on `!Send`. |

## 7. Risks / open questions

- **`mremap` failure with `MAY_MOVE`.** If `mremap` returns a new address (common; `MAY_MOVE` allows it), every cached `&Slot` pointer dangles. Plan: callers don't hold `&Slot` across grow boundaries; the crate API forbids it via lifetimes. Hard-fail on `mremap` errors with `StorageError::Corruption` (invariant #7).
- **`O_DIRECT` against arbitrary mounts.** Some filesystems (overlayfs, tmpfs) reject `O_DIRECT`. Tests use a known-good FS (ext4 / xfs) inside the container; document in `DEV_SETUP.md` (already done).
- **Random-kill test approximation.** Real `kill -9` from inside a Rust test isn't possible. We approximate via `mem::forget` on handles + manual `recover(...)`. Catches recovery-correctness bugs but not OS-level fsync bugs. Real chaos lives in Phase 11.
- **Group-commit window tuning.** Default 5 ms; configurable per `Wal`; revisited under benchmarking.
- **WAL record CRC byte order.** Spec doesn't pin; we choose **little-endian** to match storage. Documented in 2.1's plan.

## 8. Test plan

Per phase-doc Done-when:

| Phase exit item | Test |
|---|---|
| 2.1 `WalRecord` round-trip | Per-kind round-trip + truncated detection in `wal/record.rs` tests |
| 2.2 `WalRecordKind` round-trips | Per-variant rkyv round-trip in `wal/kinds.rs` tests |
| 2.3 Slot layout, size, alignment | Compile-time `size_of` / `align_of` asserts + CRC round-trip |
| 2.4 Arena open/grow/reopen | tempfile-backed test under Linux container |
| 2.5 Allocator alloc/free/version bump | Property test on alloc/free sequences |
| 2.6 Segment append + read | Round-trip via `WalReader` |
| 2.7 Reader handles truncated tail | Truncate file mid-record; reader stops cleanly |
| 2.8 Group commit batching | 100 concurrent appends → ≤ 5 fsyncs → all complete |
| 2.9 Wal end-to-end | Append via `Wal::append` → read via `wal.reader()` |
| 2.10 Recovery idempotency | Recover twice → identical post-state |
| 2.11 1000-iteration random-kill | proptest at N=100 ops, 1000 iterations, 0 failures |
| 2.12 Checkpoint | Write checkpoint → recovery skips records ≤ durable_lsn |

Concurrency / unsafe coverage:

- **Miri** runs against `brain-storage` in CI on nightly (Linux). Locked by phase exit checklist.
- **`unsafe` audit** via the `rust-unsafe-checker` skill at every diff.
- **Loom** for any lock-free / atomic-ordering code (not expected this phase).

## 9. Commit shape

One commit per sub-task in numerical order. Each commit:

- Compiles and tests pass (in the Linux container — that's the gate now).
- References the sub-task's plan (substantial sub-tasks).
- Marks `[ ]` → `[x]` in the phase doc.

2.8 (group commit) and 2.10 (recovery) likely split each into 2 commits with natural breakpoints — surfaced in their per-task plans.

## 10. Phase exit checklist (preview)

- All 12 sub-tasks done.
- `just verify` green inside the Linux container.
- Random-kill test passes 1000 iterations.
- Miri passes on `brain-storage` (nightly Linux).
- Every `unsafe` block has a `// SAFETY:` per AUTONOMY §15.
- `cargo doc -p brain-storage` clean.
- Tag `phase-2-complete`.

## 11. After confirmation

1. Land the platform-alignment commits (AUTONOMY §22, DEV_SETUP, README, this plan).
2. **You provision the Linux dev container** per `DEV_SETUP.md` §B.1 / B.2 (OrbStack or Docker). Test that `just verify` runs cleanly in it on the current `feature/brain-protocol` codebase.
3. Add `memmap2` and `libc` to workspace deps in a small prep commit on `feature/brain-storage`.
4. For each substantial sub-task: write a per-task plan; pause for confirmation; implement; CI gates the test result.
5. After 2.12: walk the phase exit checklist; merge `feature/brain-storage` → `dev` → `main`; tag `phase-2-complete`.

## 12. Confirmation

This revision answers the platform decision. Awaiting "go" to land the platform-alignment commits and start Phase 2 sub-task 2.1.

---

## Appendix A — Sources cited

- [memmap2 docs](https://docs.rs/memmap2)
- [Linux pwritev2(2)](https://man.archlinux.org/man/pwritev2.2.en)
- [libc::pwritev2 (Rust binding)](https://docs.diesel.rs/master/libc/fn.pwritev2.html)
- [Rustix PR #489 — pwritev2 ABIs](https://github.com/bytecodealliance/rustix/pull/489)
- [QEMU file-posix.c — RWF_DSYNC + FUA](https://www.mail-archive.com/qemu-block@nongnu.org/msg119106.html) — reference impl of pwritev2(RWF_DSYNC)
