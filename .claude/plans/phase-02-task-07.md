# Phase 2 — Task 2.7: `WalReader` over a directory of segments

**Classification:** moderate. Pure-Rust file reading layered on top of 2.1's `WalRecord::decode_one` and 2.6's segment header. The subtle part is **when a CRC failure means "WAL tail; stop cleanly" vs "mid-segment corruption; refuse to proceed"**.

**Spec:** `spec/05_storage_arena_wal/05_wal_records.md` §§1, 17 (segment header + record packing), `08_recovery.md` §§4, 10 (recovery scan algorithm + failure modes).

## 1. Scope

This task delivers `WalReader` — an iterator-shaped scan over all `*.wal` segments in a directory, yielding records in strict LSN order.

In:

- `WalReader::open(dir, shard_uuid)` — lists `*.wal` files, sorts by segment_seq, validates every segment header (magic / format / shard_uuid / CRC), checks the seq sequence is contiguous.
- `impl Iterator for WalReader { type Item = Result<WalRecord, WalReadError>; }` — streams records in LSN order; ends cleanly at the tail of the last segment (whether truncated or CRC-failed); errors hard on mid-segment corruption.
- `last_decoded_lsn() -> Option<u64>` accessor so 2.10's recovery driver can pick up.
- `segments() -> &[SegmentInfo]` for diagnostics.

Out:

- Checkpoint-aware "start at LSN N" filtering (2.10).
- Mutating state from records (recovery's `apply_*` calls — 2.10).
- HNSW rebuild (2.10 + 6.x).
- `SUBSCRIBE` semantics (later phase).
- Open-for-append on the *last* segment (2.6 covered fresh-create; tail-of-existing is part of 2.9's `Wal::open`).
- Direct I/O / mmap. Reader is plain buffered `std::fs::read` per segment — explicit reasoning in §4.1.

## 2. Spec quotes that bind the design

> **§05/04 §4** (segment names): "Segment names are 10-digit zero-padded sequence numbers."
>
> **§05/08 §4** (recovery scan): pseudo-code excerpt —
> ```
> if record_header.lsn != current_lsn { return Err(...); }     // strict LSN order
> ...
> if computed_crc != footer.payload_crc32c {                   // CRC failed
>     log::info!("WAL truncation detected at LSN {}", current_lsn);
>     return Ok(current_lsn - 1);                               // accept loss
> }
> ```
>
> **§05/08 §10.1** (missing segment): "If a segment file is missing in the middle of the segment sequence … recovery refuses to start."
>
> **§05/08 §10.3** (mid-segment corruption): "If a record's CRC fails in the middle of a segment (not at the end), this is unusual … The substrate … refuses to proceed."
>
> **§05/05 §1** (segment header) — already implemented in 2.6; reader validates the same fields against the same CRC range `[0..48]`.

## 3. Disambiguation: "tail" vs "mid-segment"

The reader must distinguish two failure cases that look similar from `WalRecord::decode_one`'s view:

| What `decode_one` returns | At end of last segment | Elsewhere |
|---|---|---|
| `Ok(DecodeOutcome::Truncated)` | **Accept — clean tail.** Last record was being written at crash. | **Error — corruption.** Sealed segment shouldn't have a partial record. |
| `Err(WalRecordError::CrcMismatch)` | **Accept — tail per spec §05/08 §4.** | **Error — spec §10.3.** |
| `Err(WalRecordError::UnknownRecordType)` / `NonZeroReserved` / `PayloadTooLarge` | **Error.** These can't be confused with truncation. | **Error.** |

Plus a third check at the *boundary* between segments:

- Segment seq sequence must be contiguous (no gap). Spec §10.1.
- Next segment's `starting_lsn` must equal `last_decoded_lsn + 1`. (Spec §05/08 §4's "strict LSN order" applied across the segment boundary.)

A violation of either is a hard error.

## 4. Architecture

### 4.1 Why read each segment into memory rather than mmap or `BufReader`

| Option | Verdict | Why |
|---|---|---|
| **A. `std::fs::read` each segment, slice with cursor (chosen)** | ✓ | Decodes via existing `WalRecord::decode_one(&[u8])` directly. One segment in RAM at a time (≤ 256 MiB cap). Simple; no unsafe. |
| B. `mmap` each segment read-only | ✗ | Adds another mmap site (current crate has one, in `arena/file.rs`). For sequential read once, mmap saves a memcpy at the cost of more unsafe and more `// SAFETY:` comments. The savings don't matter for recovery, which is bound by replay throughput (~100K records/sec) not memcpy bandwidth. |
| C. `BufReader<File>` with a staging buffer | ✗ | Records are variable-length; `decode_one` is slice-based. Would need to read 32 bytes (header), peek `payload_length`, read 8+payload_length more bytes, decode. Workable but more code than (A). |
| D. Stream the whole WAL via one buffered reader across segments | ✗ | Confuses segment boundaries; segment headers would have to be skipped inline. (A) keeps boundaries explicit. |

`std::fs::read` (option A) is the natural choice for one-shot recovery. Memory peak is one segment (256 MiB cap from `WAL_SEGMENT_SIZE_BYTES`); fine on any realistic Linux server.

### 4.2 Types

```rust
pub struct WalReader {
    segments: Vec<SegmentInfo>,    // sorted by segment_seq, contiguous after open()
    shard_uuid: [u8; 16],
    current_idx: usize,             // index into `segments`
    current: Option<LoadedSegment>, // loaded lazily on first iteration into a segment
    expected_next_lsn: u64,
    last_decoded_lsn: Option<u64>,
    finished: bool,                 // sticky: set when we emit None or an Err
}

#[derive(Debug, Clone)]
pub struct SegmentInfo {
    pub path: PathBuf,
    pub segment_seq: u64,
    pub starting_lsn: u64,
    pub file_size: u64,
}

struct LoadedSegment {
    bytes: Vec<u8>,
    cursor: usize,                  // byte offset; starts at WAL_SEGMENT_HEADER_LEN
}
```

`finished` is a one-way switch. Once the reader emits `None` (clean end) or an `Err`, subsequent `next()` calls return `None` (fused iterator behavior). Implement `std::iter::FusedIterator` to advertise this.

### 4.3 `open` flow

```rust
pub fn open(dir, shard_uuid) -> Result<Self, WalReadError> {
    // 1. List *.wal entries, parse segment_seq from filename.
    //    (Spec §05/04 §4: 10-digit zero-padded.)
    let mut infos = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("wal") { continue; }
        let segment_seq = parse_segment_seq_from_filename(&path)?;
        let info = read_and_validate_segment_header(&path, shard_uuid, segment_seq)?;
        infos.push(info);
    }

    // 2. Sort by segment_seq.
    infos.sort_by_key(|info| info.segment_seq);

    // 3. Validate the seq sequence is contiguous (no gaps).
    for w in infos.windows(2) {
        if w[1].segment_seq != w[0].segment_seq + 1 {
            return Err(WalReadError::SegmentSequenceGap {
                after: w[0].segment_seq, found: w[1].segment_seq,
            });
        }
    }

    // 4. Compute initial expected LSN.
    let expected_next_lsn = infos.first().map_or(1, |s| s.starting_lsn);
    let finished = infos.is_empty();

    Ok(Self { segments: infos, shard_uuid, current_idx: 0, current: None,
              expected_next_lsn, last_decoded_lsn: None, finished })
}
```

`read_and_validate_segment_header` opens the file, reads 4096 bytes (or errors if file is shorter), checks magic / format_version / shard_uuid, computes CRC over `[0..48]` and compares to the stored value. Borrows the constants from `wal/segment.rs`.

The filename → segment_seq parse: split on `.`, take the stem, `parse::<u64>()`. Spec doesn't strictly require the filename match the header's `segment_seq`, but operationally they should — we cross-check and emit `FilenameSegmentSeqMismatch` if they differ.

### 4.4 `Iterator::next` flow

```rust
fn next(&mut self) -> Option<Result<WalRecord, WalReadError>> {
    if self.finished { return None; }
    loop {
        // (a) Ensure a segment is loaded.
        if self.current.is_none() {
            if self.current_idx >= self.segments.len() {
                self.finished = true;
                return None;                    // clean end
            }
            let info = &self.segments[self.current_idx];
            if info.starting_lsn != self.expected_next_lsn {
                self.finished = true;
                return Some(Err(WalReadError::LsnGapAtSegmentBoundary {
                    segment_seq: info.segment_seq,
                    expected_lsn: self.expected_next_lsn,
                    found_starting_lsn: info.starting_lsn,
                }));
            }
            let bytes = match std::fs::read(&info.path) {
                Ok(b) => b, Err(e) => { self.finished = true; return Some(Err(e.into())); }
            };
            self.current = Some(LoadedSegment { bytes, cursor: WAL_SEGMENT_HEADER_LEN });
        }

        let seg = self.current.as_mut().unwrap();
        // (b) End of this segment?
        if seg.cursor >= seg.bytes.len() {
            self.current = None;
            self.current_idx += 1;
            continue;
        }

        // (c) Try to decode the next record.
        match WalRecord::decode_one(&seg.bytes[seg.cursor..]) {
            Ok(DecodeOutcome::Record { record, consumed }) => {
                let lsn = record.lsn.raw();
                if lsn != self.expected_next_lsn {
                    self.finished = true;
                    return Some(Err(WalReadError::LsnGap {
                        in_segment: self.segments[self.current_idx].segment_seq,
                        expected_lsn: self.expected_next_lsn,
                        found_lsn: lsn,
                    }));
                }
                seg.cursor += consumed;
                self.expected_next_lsn = lsn + 1;
                self.last_decoded_lsn = Some(lsn);
                return Some(Ok(record));
            }
            Ok(DecodeOutcome::Truncated) | Err(WalRecordError::CrcMismatch { .. }) => {
                let is_last = self.current_idx + 1 >= self.segments.len();
                self.finished = true;
                if is_last {
                    tracing::info!(
                        segment_seq = self.segments[self.current_idx].segment_seq,
                        last_lsn = ?self.last_decoded_lsn,
                        "WAL tail truncation (clean end)"
                    );
                    return None;                // accept tail; clean end
                } else {
                    return Some(Err(WalReadError::MidSegmentCorruption {
                        segment_seq: self.segments[self.current_idx].segment_seq,
                    }));
                }
            }
            Err(other) => {
                // UnknownRecordType / NonZeroReserved / PayloadTooLarge — always an error.
                self.finished = true;
                return Some(Err(WalReadError::RecordError {
                    in_segment: self.segments[self.current_idx].segment_seq,
                    expected_lsn: self.expected_next_lsn,
                    source: other,
                }));
            }
        }
    }
}
```

### 4.5 Errors

```rust
#[derive(thiserror::Error, Debug)]
pub enum WalReadError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("WAL segment {path:?} too small ({size} bytes; minimum is the 4 KB header)")]
    SegmentTooSmall { path: PathBuf, size: u64 },

    #[error("invalid segment header magic in {path:?}: expected b\"BWAL\", got {found:?}")]
    InvalidSegmentMagic { path: PathBuf, found: [u8; 4] },

    #[error("unsupported segment format_version {version} in {path:?}")]
    UnsupportedSegmentFormatVersion { path: PathBuf, version: u32 },

    #[error("segment header CRC mismatch in {path:?}: stored {stored:#010x}, computed {computed:#010x}")]
    SegmentHeaderCrcMismatch { path: PathBuf, stored: u32, computed: u32 },

    #[error("shard_uuid mismatch in {path:?}: expected {expected:?}, header says {found:?}")]
    SegmentShardUuidMismatch { path: PathBuf, expected: [u8; 16], found: [u8; 16] },

    #[error("filename segment_seq {filename_seq} doesn't match header segment_seq {header_seq} in {path:?}")]
    FilenameSegmentSeqMismatch { path: PathBuf, filename_seq: u64, header_seq: u64 },

    #[error("filename {filename:?} is not a valid 10-digit segment_seq")]
    InvalidSegmentFilename { filename: String },

    #[error("segment sequence gap: segment {found} appears after {after} (expected {})", after + 1)]
    SegmentSequenceGap { after: u64, found: u64 },

    #[error("LSN gap at segment boundary: segment {segment_seq} starts at LSN {found_starting_lsn}, expected {expected_lsn}")]
    LsnGapAtSegmentBoundary { segment_seq: u64, expected_lsn: u64, found_starting_lsn: u64 },

    #[error("LSN gap in segment {in_segment}: expected LSN {expected_lsn}, record has {found_lsn}")]
    LsnGap { in_segment: u64, expected_lsn: u64, found_lsn: u64 },

    #[error("mid-segment corruption in segment {segment_seq} (spec §05/08 §10.3)")]
    MidSegmentCorruption { segment_seq: u64 },

    #[error("record error in segment {in_segment} at expected LSN {expected_lsn}: {source}")]
    RecordError { in_segment: u64, expected_lsn: u64, #[source] source: WalRecordError },
}
```

### 4.6 Public surface and re-exports

```
crates/brain-storage/src/wal/
├── kinds.rs
├── mod.rs       (update — re-export reader types)
├── payload.rs
├── reader.rs    (NEW — this task)
├── record.rs
└── segment.rs
```

Re-exports: `WalReader`, `WalReadError`, `SegmentInfo`.

## 5. Trade-offs

| Question | Choice | Why |
|---|---|---|
| Reader takes `dir` or list of paths? | `dir` | Matches spec §05/08 §4 ("substrate enumerates `wal/*.wal` segments"). Tests build a tempdir with the desired files. |
| Strict LSN ordering check? | Yes | Spec §05/08 §4 explicitly checks `record_header.lsn != current_lsn`. Catches WAL corruption / accidental file swaps. |
| Filename-vs-header `segment_seq` cross-check? | Yes | Cheap; catches "renamed segments" bugs. |
| Lazy load segment files? | Yes (load on first iter into the segment) | Memory peak is one segment, not all of them. |
| Treat empty directory as an error? | No (returns immediately-empty iterator) | Recovery (2.10) decides whether an empty WAL is acceptable (it isn't, but that's policy). 2.7 is mechanism. |
| Implement `FusedIterator`? | Yes | Once we emit `None` or `Err`, the iterator is done. Advertising this lets adapters like `Iterator::fuse` short-circuit. |
| Implement `Drop` to close files? | Not needed | We `std::fs::read` into a `Vec<u8>` and drop the File. No long-lived handles. |

## 6. Risks

- **Memory peak per segment.** `std::fs::read` allocates the whole segment. Capped at 256 MiB by `WAL_SEGMENT_SIZE_BYTES`. Acceptable.
- **Filename parsing is fragile.** A stray `0000000000.wal.bak` from an operator backup would attempt to parse as a segment. The spec is explicit (§05/04 §4) about the format; we reject anything that isn't a 10-digit zero-padded `*.wal`. Files we can't parse are reported as `InvalidSegmentFilename` rather than silently skipped.
- **Mid-segment Truncated.** Indicates a payload_length field whose value would overrun the segment. We treat this as mid-segment corruption (same handling as CRC mismatch). Surfacing it as a distinct variant if we ever see it in practice is a follow-up.
- **No checkpoint coupling here.** A future caller (2.10) may want to start at LSN > 1, skipping old segments. Not handled in 2.7; recovery driver in 2.10 will filter segments before constructing the reader, or we'll add `WalReader::open_with_start_lsn`.
- **`tracing::info!` on clean tail.** Side-effecting log inside `Iterator::next`. Acceptable for a one-shot recovery path; not for a high-frequency hot loop. Doc-comment notes this.

## 7. Test plan

All tests use `tempfile::TempDir`, write segments via 2.6's `WalSegment::create_new` + `append_record` + `flush`, then open `WalReader` over the directory.

### Open (5)

1. Empty directory → reader opens; iterator returns `None` immediately; `last_decoded_lsn()` is `None`.
2. One valid empty segment (header only, no records) → iterator returns `None`; `last_decoded_lsn()` is `None`.
3. `open` with wrong `shard_uuid` → `SegmentShardUuidMismatch`.
4. `open` with corrupted segment magic → `InvalidSegmentMagic`.
5. `open` with sequence gap (segments 0 and 2 present, 1 missing) → `SegmentSequenceGap`.

### Round-trip (4)

6. **Write 1000 records, read them all back.** (The phase doc's done-when criterion.) Each record's fields match exactly.
7. Records distributed across **two** segments (manually create two segments with continuous LSNs) — iterator yields all in order.
8. Records distributed across **three** segments — same.
9. `last_decoded_lsn()` after iteration equals the last record's LSN.

### Tail truncation (2)

10. Write N records to the only segment, then truncate the file to the middle of the last record (file-level `set_len`) → iterator yields N-1 records, then `None`. No error. (Tail truncation is the expected case after a crash.)
11. Write N records, then corrupt the CRC of the last record (flip a byte in the footer) → iterator yields N-1 records, then `None`. No error per spec §05/08 §4 "If CRC fails, this is the truncation point" — when on the last segment.

### Mid-segment corruption (2)

12. Two segments: segment 0 has records 1..=N, segment 1 has records N+1..=M. Truncate the **middle** of segment 0 (not the last) → iterator yields records up to the truncation, then `Err(MidSegmentCorruption)`. (Tests the "truncation in a sealed segment is corruption" rule.)
13. Two segments, corrupt a record's CRC in the middle of segment 0 → `Err(MidSegmentCorruption)`.

### LSN ordering (2)

14. Records out of order within a segment (hand-craft a record with the wrong LSN) → `Err(LsnGap)`.
15. Segment 1's `starting_lsn` doesn't match segment 0's tail + 1 → `Err(LsnGapAtSegmentBoundary)`.

### Filename hygiene (1)

16. A segment file whose filename's segment_seq doesn't match the header's → `Err(FilenameSegmentSeqMismatch)`.

**Total: 16 tests.** Test #6 (1000-record round-trip) is the phase doc's load-bearing criterion.

## 8. Estimated commit shape

One commit on `feature/brain-storage`:

> `feat(brain-storage): WalReader over a directory of segments (sub-task 2.7)`

Body covers:
- The iterator shape and the tail-vs-mid-segment rule (§3 above).
- LSN-ordering checks within and across segments.
- Filename/header `segment_seq` cross-check.
- The "load one segment at a time" memory profile.
- `tracing::info!` on clean tail (and rationale).
- Phase doc 2.7 entry: check the box.

Files touched:
- `crates/brain-storage/src/wal/reader.rs` (new, ~480 lines including tests).
- `crates/brain-storage/src/wal/mod.rs` (re-export).
- `docs/phases/phase-02-storage.md` (mark 2.7 done).

No new deps. Verify gate: `cargo fmt --all -- --check && ./scripts/check-skills.sh && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p brain-storage --all-targets` inside the dev container.

---

PLAN READY: see `.claude/plans/phase-02-task-07.md` — confirm to proceed.
