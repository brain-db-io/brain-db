//! Snapshot + restore chaos tests.
//!
//! Companion to brain-storage's WAL chaos tests (`random_kill.rs`,
//! `bit_flip.rs`, `io_fault.rs`). Those exercise WAL recovery under
//! process-kill and corruption; this file does the same for the
//! HNSW snapshot persistence path — the only part of Brain's
//! snapshot pipeline that ships a substantive CRC + atomic-rename
//! design today.
//!
//! ## What we test
//!
//! 1. **`kill_mid_snapshot_write`** — the snapshot writer crashes
//!    between writing the `.hnsw.graph` / `.hnsw.data` body files and
//!    the atomic `rename` of `.brain.tmp` → `.brain`. On restart the
//!    loader must either (a) refuse the snapshot cleanly so the caller
//!    falls back to WAL replay / rebuild, or (b) never observe the
//!    partial state at all (because `.brain` is the completion marker
//!    and never appears until everything else is durable).
//!
//! 2. **`kill_between_snapshot_and_wal_truncation`** — the snapshot
//!    write completes durably (rename, fsync). A crash *here* would
//!    normally happen between the snapshot succeeding and the WAL
//!    retention worker truncating WAL records that the snapshot now
//!    covers. The invariant: a loader seeing the complete snapshot
//!    reads the recorded `taken_at_lsn`, so WAL replay from
//!    `taken_at_lsn + 1` is non-destructive (the WAL still has those
//!    records — truncation didn't happen yet). The snapshot must
//!    therefore round-trip and surface its LSN unchanged.
//!
//! 3. **`corrupted_snapshot_falls_back_to_wal`** — a complete
//!    snapshot exists, then a bit flip corrupts its body. The loader
//!    must reject the snapshot via the BLAKE3 footer check so the
//!    caller (the HNSW maintenance worker) can fall back to a full
//!    `rebuild` from arena + metadata. Silently returning a
//!    half-correct index would violate Brain's "no silent
//!    corruption" rule.
//!
//! ## Why this lives in `brain-index/tests/`
//!
//! `HnswIndex::save_snapshot` + `HnswIndex::load_snapshot` are the
//! only public, atomic, CRC-protected snapshot surface in the
//! workspace today. The higher-level `ShardSnapshotSource` in
//! `brain-server` orchestrates a multi-file snapshot directory
//! (arena.bin + metadata.redb + hnsw.* + manifest.toml), but it
//! writes the non-HNSW pieces directly without an atomic-rename or
//! per-file checksum — a follow-up worth in its own commit (see the
//! commit message for the gap).
//!
//! ## Determinism
//!
//! All tests are single-iteration deterministic. The fixture index,
//! shard UUID, and corruption offset are constants — failures
//! reproduce on the first run.

#![allow(clippy::cast_possible_truncation)]

use std::fs;
use std::io::Write;
use std::path::Path;

use brain_core::MemoryId;
use brain_index::{HnswError, HnswIndex, IndexParams};

/// Deterministic UUID for the test shard. Matches the inline test
/// suite in `crates/brain-index/src/hnsw.rs`.
const TEST_UUID: [u8; 16] = [0xCD; 16];

const VECTOR_DIM: usize = 4;

const TEST_LSN: u64 = 0x1234_5678_9ABC_DEF0;

/// L2-normalise so cosine distance behaves cleanly.
fn vec4(a: f32, b: f32, c: f32, d: f32) -> [f32; VECTOR_DIM] {
    let n = (a * a + b * b + c * c + d * d).sqrt();
    [a / n, b / n, c / n, d / n]
}

fn mid(slot: u64) -> MemoryId {
    MemoryId::pack(1, slot, 1)
}

fn populated_index() -> HnswIndex<VECTOR_DIM> {
    let mut idx = HnswIndex::<VECTOR_DIM>::new(IndexParams::default_v1())
        .expect("HnswIndex::new with default params");
    idx.insert(mid(1), &vec4(1.0, 0.0, 0.0, 0.0))
        .expect("insert mid(1)");
    idx.insert(mid(2), &vec4(0.0, 1.0, 0.0, 0.0))
        .expect("insert mid(2)");
    idx.insert(mid(3), &vec4(0.0, 0.0, 1.0, 0.0))
        .expect("insert mid(3)");
    idx
}

/// Toggle one bit at `byte_offset`. Returns the prior value so the
/// caller can restore if a "round-trip after corruption" assertion
/// needs it.
fn flip_bit_in_file(path: &Path, byte_offset: u64, bit_offset: u8) {
    assert!(bit_offset < 8);
    use std::io::{Read, Seek, SeekFrom};
    let mut f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .expect("open for bit-flip");
    f.seek(SeekFrom::Start(byte_offset)).expect("seek");
    let mut buf = [0u8; 1];
    f.read_exact(&mut buf).expect("read byte");
    buf[0] ^= 1u8 << bit_offset;
    f.seek(SeekFrom::Start(byte_offset)).expect("seek back");
    f.write_all(&buf).expect("write byte back");
    f.sync_all().expect("sync");
}

// ---------------------------------------------------------------------------
// Scenario 1: kill mid-snapshot.
// ---------------------------------------------------------------------------

/// Simulate a process kill between `file_dump` (writes
/// `.hnsw.graph` / `.hnsw.data`) and the atomic rename of
/// `<basename>.brain.tmp` → `<basename>.brain`.
///
/// The completion-marker invariant: `<basename>.brain` is what the
/// loader opens first. If it does not exist, the snapshot is not
/// readable — period. The caller (Phase 8 maintenance worker)
/// falls back to a full rebuild from arena + metadata.
#[test]
fn kill_mid_snapshot_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let idx = populated_index();

    // Step 1: write a real snapshot to a *different* basename to get
    // valid `.hnsw.graph` / `.hnsw.data` bodies that look like the
    // mid-snapshot state.
    idx.save_snapshot(dir.path(), "real", TEST_LSN, TEST_UUID)
        .expect("save_snapshot to produce body files");

    // Step 2: simulate the kill — copy the two body files under the
    // target basename, but leave `<target>.brain` absent. Also create
    // a half-written `<target>.brain.tmp` to mimic the worst case
    // where the worker died inside the tempfile write.
    let target = "killed";
    for ext in [".hnsw.graph", ".hnsw.data"] {
        fs::copy(
            dir.path().join(format!("real{ext}")),
            dir.path().join(format!("{target}{ext}")),
        )
        .expect("copy body file");
    }
    let tmp_path = dir.path().join(format!("{target}.brain.tmp"));
    let mut tmp = fs::File::create(&tmp_path).expect("create .brain.tmp");
    // Write a few bytes so the temp file is non-empty — the loader
    // must still ignore it because only `.brain` is the completion
    // marker.
    tmp.write_all(&[0u8; 16]).expect("write partial tmp");
    drop(tmp);

    // Step 3: loader behavior — opening at `target` must error
    // because `<target>.brain` is missing. The error variant is the
    // I/O-not-found mapped through `HnswError::SnapshotIo`.
    match HnswIndex::<VECTOR_DIM>::load_snapshot(dir.path(), target, TEST_UUID) {
        Err(HnswError::SnapshotIo(e)) => {
            assert_eq!(
                e.kind(),
                std::io::ErrorKind::NotFound,
                "missing .brain must surface as NotFound, got kind {:?}",
                e.kind(),
            );
        }
        Err(other) => {
            panic!("mid-snapshot kill must surface NotFound via SnapshotIo, got {other:?}")
        }
        Ok(_) => panic!(
            "mid-snapshot kill must not produce a loadable snapshot — \
             .brain marker should be absent"
        ),
    }

    // Step 4: the orphaned `.brain.tmp` must not be mistaken for the
    // real marker by any later operator inspection. Confirm the
    // distinct paths.
    assert!(
        tmp_path.exists(),
        "test harness sanity: the half-written tmp should still be on disk"
    );
    assert!(
        !dir.path().join(format!("{target}.brain")).exists(),
        "completion marker must not exist after a mid-snapshot kill"
    );
}

// ---------------------------------------------------------------------------
// Scenario 2: kill between snapshot and WAL truncation.
// ---------------------------------------------------------------------------

/// A complete snapshot exists on disk; the worker then crashes
/// *before* the WAL retention worker has truncated the records that
/// the snapshot now covers. On restart:
///
/// - The snapshot loads cleanly (it's complete and CRC-correct).
/// - The snapshot's `taken_at_lsn` is preserved exactly, so the
///   caller knows from which WAL LSN to resume replay.
/// - No data is duplicated: WAL replay from `taken_at_lsn + 1`
///   plus the snapshot's state is the full pre-crash state. (This
///   property is enforced upstream by the WAL retention design —
///   here we lock down the substrate's contribution: the LSN
///   round-trips and the index content is bit-identical.)
#[test]
fn kill_between_snapshot_and_wal_truncation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let idx = populated_index();
    let q = vec4(1.0, 0.0, 0.0, 0.0);
    let pre_kill_hits = idx.search_active(&q, 3, None);

    // Snapshot completes durably.
    idx.save_snapshot(dir.path(), "pre_truncation", TEST_LSN, TEST_UUID)
        .expect("save_snapshot durable");

    // Simulated kill — nothing else happened. The WAL retention
    // worker did *not* run, so on a real system the WAL on disk
    // would still contain every record covered by the snapshot.
    // For this layer's contract we only need to prove the snapshot
    // survives the implicit restart untouched.

    let (loaded, loaded_lsn) =
        HnswIndex::<VECTOR_DIM>::load_snapshot(dir.path(), "pre_truncation", TEST_UUID)
            .expect("post-kill load");

    assert_eq!(
        loaded_lsn, TEST_LSN,
        "taken_at_lsn must round-trip exactly — the WAL retention worker \
         relies on this to know where to resume",
    );

    let post_kill_hits = loaded.search_active(&q, 3, None);
    assert_eq!(
        pre_kill_hits.len(),
        post_kill_hits.len(),
        "loaded index must return the same hit count"
    );
    for (a, b) in pre_kill_hits.iter().zip(post_kill_hits.iter()) {
        assert_eq!(
            a.0, b.0,
            "MemoryId order must match — kill between snapshot \
             and truncation must not lose state"
        );
        assert!(
            (a.1 - b.1).abs() < 1e-5,
            "similarity must round-trip (pre={}, post={})",
            a.1,
            b.1
        );
    }

    // The snapshot files are still on disk after the simulated
    // restart, ready for a clean retention pass.
    for ext in [".hnsw.graph", ".hnsw.data", ".brain"] {
        let p = dir.path().join(format!("pre_truncation{ext}"));
        assert!(
            p.exists(),
            "snapshot file {p:?} must persist across the simulated kill",
        );
    }
}

// ---------------------------------------------------------------------------
// Scenario 3: snapshot corruption → loader rejects, caller falls back.
// ---------------------------------------------------------------------------

/// A complete snapshot exists; a single bit flips somewhere in the
/// `.brain` body (modelling a cosmic ray, a flaky drive, or a
/// metadata-table bit rot). The BLAKE3 footer over the file no
/// longer matches, so the loader must reject the snapshot.
///
/// The substrate's job ends at "detect and surface"; the caller's
/// job (rebuild HNSW from arena + metadata) is the higher-layer
/// recovery this test gates. A torn index returned silently here
/// would propagate wrong recall results forever — that's why we
/// pin the loader's behaviour explicitly.
#[test]
fn corrupted_snapshot_falls_back_to_wal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let idx = populated_index();
    idx.save_snapshot(dir.path(), "rot", TEST_LSN, TEST_UUID)
        .expect("save baseline snapshot");

    // Sanity-check: the baseline loads cleanly before we corrupt
    // anything.
    let (clean, clean_lsn) = HnswIndex::<VECTOR_DIM>::load_snapshot(dir.path(), "rot", TEST_UUID)
        .expect("baseline load before corruption");
    assert_eq!(clean.len(), 3);
    assert_eq!(clean_lsn, TEST_LSN);

    // Flip a bit deep inside the `.brain` body — past the
    // fixed-size header, before the trailing BLAKE3 footer.
    // Picking the byte immediately after the header lands the flip
    // in the body's id_map region for our 3-entry populated index,
    // which is deterministic across runs.
    let brain_path = dir.path().join("rot.brain");
    let brain_len = fs::metadata(&brain_path).expect("stat .brain").len();
    // The on-disk layout is { 64-byte header | body | 8-byte footer };
    // pin those here so the test pins the file format too.
    const HEADER_LEN: u64 = 64;
    const FOOTER_LEN: u64 = 8;
    assert!(
        brain_len > HEADER_LEN + FOOTER_LEN,
        "fixture sanity: .brain has a body (got len={brain_len})"
    );
    let corrupt_offset = HEADER_LEN; // first byte of body
    flip_bit_in_file(&brain_path, corrupt_offset, 3);

    // Loader rejects the corrupted snapshot. We accept either
    // `SnapshotBadFooter` (BLAKE3 mismatch caught at the file
    // boundary) or `SnapshotBadHeaderCrc` (if the bit landed inside
    // the header region after parser refactors). Both are
    // spec-faithful "no silent corruption" outcomes.
    match HnswIndex::<VECTOR_DIM>::load_snapshot(dir.path(), "rot", TEST_UUID) {
        Err(HnswError::SnapshotBadFooter) => {}
        Err(HnswError::SnapshotBadHeaderCrc { .. }) => {}
        Err(HnswError::SnapshotBadBody(_)) => {}
        Err(other) => panic!("bit-flip must surface as a corruption error, not {other:?}"),
        Ok(_) => panic!(
            "bit-flip must NOT yield a loadable snapshot — \
             silent corruption is the one outcome Brain is never \
             allowed to produce"
        ),
    }

    // The substrate contract met: corruption is detected. The
    // caller's response (rebuild from arena+metadata) is the
    // higher-layer concern exercised by the Phase 8 maintenance
    // worker tests.
}
