//! Per-shard Glommio executor + on-disk arena.
//!
//! One OS thread per shard hosts a `glommio::LocalExecutor` (single-threaded,
//! io_uring-driven) that owns the shard's `ArenaFile` + `SlotAllocator`. The
//! Tokio connection layer talks to a shard through a `flume::Sender<ShardRequest>`;
//! replies come back through per-call `flume::Sender<...>` carried in the
//! request. Flume's `send_async` / `recv_async` are reactor-agnostic — both
//! ends `.await` natively under whichever runtime drives them.
//!
//! On-disk layout (spec §12/01 §2):
//!
//! ```text
//!   <data_dir>/<shard_id>/
//!     arena.bin       mmap'd by ArenaFile
//!     shard.uuid      16 raw bytes; generated once on first open
//! ```
//!
//! Lifecycle is a two-handle split:
//!
//! ```text
//!   spawn_shard() ─▶ (ShardHandle, ShardJoiner)
//!                       │              │
//!                       │              │  (single-ownership;
//!                       │              │   not cloneable)
//!                       ▼              ▼
//!                 clone freely;   used by graceful
//!                 each clone      shutdown to await
//!                 owns a Sender   the thread's exit
//!                       │
//!                       ▼  (drop every clone)
//!                 channel closes ─▶ shard_main_loop exits ─▶ joiner.join() returns
//! ```
//!
//! Spec §01/04 (layers), §01/05 (hardware: io_uring, CPU pinning),
//! §10/02 (single writer per shard), §12/01 §3 (shard UUID permanence).
//! Audit `phase-09-glommio-port.md` §7 locks flume as the boundary primitive;
//! §8.2 defers the in-shard `Rc<Cell<bool>>` shutdown flag to 9.7.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};

use brain_core::{ShardId, SlotVersion};
use brain_storage::arena::{
    AllocError, ArenaFile, ArenaOpenError, SlotAllocator, DEFAULT_INITIAL_CAPACITY_SLOTS,
};
use flume::{Receiver, Sender};
use glommio::{ExecutorJoinHandle, LocalExecutorBuilder, Placement};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Request type — extended by 9.10 with `Frame { req, reply_tx }`.
// ---------------------------------------------------------------------------

pub(crate) enum ShardRequest {
    /// Trivial round-trip. The shard replies with `()`.
    Ping { reply_tx: Sender<()> },
    /// Allocate a fresh slot. Returns `(slot_idx, slot_version)`.
    AllocSlot {
        reply_tx: Sender<Result<(u64, SlotVersion), ShardOpError>>,
    },
}

// ---------------------------------------------------------------------------
// Spawn config
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct ShardSpawnConfig {
    pub channel_capacity: usize,
    pub pin_cpu: Option<usize>,
    /// Root data directory. Per-shard subdir is `<data_dir>/<shard_id>/`.
    pub data_dir: PathBuf,
    /// Initial arena capacity in slots. The arena grows on demand via
    /// `ArenaFile::grow_to` (wired in a later sub-task).
    pub arena_initial_capacity_slots: u64,
}

impl ShardSpawnConfig {
    /// Construct with arena under `data_dir`, all other knobs defaulted.
    #[must_use]
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            channel_capacity: 1024,
            pin_cpu: None,
            data_dir: data_dir.into(),
            arena_initial_capacity_slots: DEFAULT_INITIAL_CAPACITY_SLOTS,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Spawn-time / lifecycle errors. Returned by `spawn_shard` and the
/// handle's send/recv helpers.
#[derive(Debug, thiserror::Error)]
pub enum ShardError {
    #[error("shard has shut down or is unreachable")]
    ShardDisconnected,

    #[error("failed to launch Glommio executor: {0}")]
    Spawn(String),

    #[error("failed to join shard executor thread: {0}")]
    Join(String),

    #[error("failed to open arena: {0}")]
    ArenaOpen(#[from] ArenaOpenError),

    #[error("failed to create shard directory at {path}: {source}")]
    DirCreate {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to read/write shard.uuid at {path}: {source}")]
    UuidFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl ShardError {
    fn dir_create(path: PathBuf, source: std::io::Error) -> Self {
        Self::DirCreate { path, source }
    }
    fn uuid_file(path: PathBuf, source: std::io::Error) -> Self {
        Self::UuidFile { path, source }
    }
}

/// In-shard, op-time errors. Sent back through `reply_tx` for per-request
/// failures (vs. `ShardError` which is spawn-time). Future variants:
/// `WalAppend`, `MetadataConflict`, ...
#[derive(Debug, thiserror::Error)]
pub enum ShardOpError {
    #[error("arena allocation failed: {0}")]
    ArenaFull(#[from] AllocError),
}

// ---------------------------------------------------------------------------
// Public handle types
// ---------------------------------------------------------------------------

/// Cloneable, `Send + Sync` handle the connection layer (Tokio) holds.
/// Each clone holds a `flume::Sender`. When every clone drops, the
/// shard's request channel closes and the executor's main loop exits.
/// The thread itself is awaited through [`ShardJoiner::join`].
#[derive(Clone)]
pub struct ShardHandle {
    shard_id: ShardId,
    tx: Sender<ShardRequest>,
}

impl ShardHandle {
    #[must_use]
    pub fn shard_id(&self) -> ShardId {
        self.shard_id
    }

    /// Round-trip Ping. Returns once the shard has replied.
    pub async fn ping(&self) -> Result<(), ShardError> {
        let (reply_tx, reply_rx) = flume::bounded::<()>(1);
        self.tx
            .send_async(ShardRequest::Ping { reply_tx })
            .await
            .map_err(|_| ShardError::ShardDisconnected)?;
        reply_rx
            .recv_async()
            .await
            .map_err(|_| ShardError::ShardDisconnected)?;
        Ok(())
    }

    /// Ask the shard's allocator for a fresh slot. Returns the slot
    /// index and its version stamp.
    pub async fn alloc_slot(&self) -> Result<(u64, SlotVersion), AllocSlotError> {
        let (reply_tx, reply_rx) = flume::bounded(1);
        self.tx
            .send_async(ShardRequest::AllocSlot { reply_tx })
            .await
            .map_err(|_| AllocSlotError::ShardDisconnected)?;
        reply_rx
            .recv_async()
            .await
            .map_err(|_| AllocSlotError::ShardDisconnected)?
            .map_err(AllocSlotError::Op)
    }
}

/// Caller-facing error for [`ShardHandle::alloc_slot`]. Either the shard
/// is gone (lifecycle) or the allocator declined the request (op-time).
#[derive(Debug, thiserror::Error)]
pub enum AllocSlotError {
    #[error("shard has shut down or is unreachable")]
    ShardDisconnected,
    #[error(transparent)]
    Op(#[from] ShardOpError),
}

/// One-shot ownership of the shard's OS thread. Returned alongside
/// [`ShardHandle`] from [`spawn_shard`]. Call [`ShardJoiner::join`] *after*
/// every `ShardHandle` clone has been dropped to wait for the executor
/// thread to exit cleanly. Forgetting to call `join()` leaks the thread.
pub struct ShardJoiner {
    shard_id: ShardId,
    handle: Option<ExecutorJoinHandle<()>>,
}

impl ShardJoiner {
    /// Block the current thread until the shard's executor exits.
    pub fn join(mut self) -> Result<(), ShardError> {
        let Some(h) = self.handle.take() else {
            return Ok(());
        };
        match h.join() {
            Ok(()) => {
                info!(shard_id = self.shard_id, "shard joined cleanly");
                Ok(())
            }
            Err(e) => Err(ShardError::Join(e.to_string())),
        }
    }
}

impl Drop for ShardJoiner {
    fn drop(&mut self) {
        if self.handle.is_some() {
            warn!(
                shard_id = self.shard_id,
                "ShardJoiner dropped without calling join(); thread will leak"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Per-shard owned state
// ---------------------------------------------------------------------------

struct Shard {
    shard_id: ShardId,
    arena: ArenaFile,
    allocator: SlotAllocator,
}

// ---------------------------------------------------------------------------
// Public spawn entry point
// ---------------------------------------------------------------------------

/// Open the shard's data directory + arena, then launch its `LocalExecutor`
/// on a dedicated OS thread.
pub fn spawn_shard(
    shard_id: ShardId,
    cfg: ShardSpawnConfig,
) -> Result<(ShardHandle, ShardJoiner), ShardError> {
    // ---- 1. Directory layout ------------------------------------------------
    let dir = cfg.data_dir.join(shard_id.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| ShardError::dir_create(dir.clone(), e))?;

    // ---- 2. UUID (generate or read existing) -------------------------------
    let uuid_path = dir.join("shard.uuid");
    let shard_uuid = read_or_generate_uuid(&uuid_path)?;

    // ---- 3. Arena open / create -------------------------------------------
    let arena_path = dir.join("arena.bin");
    let arena = ArenaFile::open(&arena_path, shard_uuid, cfg.arena_initial_capacity_slots)?;
    let allocator = SlotAllocator::rebuild_from_arena(&arena);
    info!(
        shard_id,
        path = %arena_path.display(),
        capacity = arena.capacity_slots(),
        used = allocator.used_count(),
        "arena opened"
    );

    // ---- 4. Spawn the Glommio executor ------------------------------------
    let (tx, rx) = flume::bounded::<ShardRequest>(cfg.channel_capacity);
    let placement = match cfg.pin_cpu {
        Some(cpu) => Placement::Fixed(cpu),
        None => Placement::Unbound,
    };
    let shard = Shard {
        shard_id,
        arena,
        allocator,
    };
    let join_handle = LocalExecutorBuilder::new(placement)
        .name(&format!("brain-shard-{shard_id}"))
        .spawn(move || async move {
            shard_main_loop(shard, rx).await;
        })
        .map_err(|e| ShardError::Spawn(e.to_string()))?;
    let handle = ShardHandle { shard_id, tx };
    let joiner = ShardJoiner {
        shard_id,
        handle: Some(join_handle),
    };
    Ok((handle, joiner))
}

// ---------------------------------------------------------------------------
// Shard main loop
// ---------------------------------------------------------------------------

async fn shard_main_loop(mut shard: Shard, rx: Receiver<ShardRequest>) {
    info!(
        shard_id = shard.shard_id,
        "shard executor entering main loop"
    );
    while let Ok(req) = rx.recv_async().await {
        match req {
            ShardRequest::Ping { reply_tx } => {
                if reply_tx.send_async(()).await.is_err() {
                    warn!(
                        shard_id = shard.shard_id,
                        "Ping reply dropped (caller gone)"
                    );
                }
            }
            ShardRequest::AllocSlot { reply_tx } => {
                let out = shard
                    .allocator
                    .alloc(&mut shard.arena)
                    .map_err(ShardOpError::from);
                if reply_tx.send_async(out).await.is_err() {
                    warn!(
                        shard_id = shard.shard_id,
                        "AllocSlot reply dropped (caller gone)"
                    );
                }
            }
        }
    }
    if let Err(e) = shard.arena.msync_all() {
        warn!(
            shard_id = shard.shard_id,
            error = %e,
            "msync_all at shutdown failed"
        );
    }
    info!(
        shard_id = shard.shard_id,
        "shard main loop exiting (channel closed)"
    );
}

// ---------------------------------------------------------------------------
// UUID helper
// ---------------------------------------------------------------------------

fn read_or_generate_uuid(path: &Path) -> Result<[u8; 16], ShardError> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() == 16 => {
            let mut out = [0u8; 16];
            out.copy_from_slice(&bytes);
            Ok(out)
        }
        Ok(other) => Err(ShardError::uuid_file(
            path.to_owned(),
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("shard.uuid expected 16 bytes, got {}", other.len()),
            ),
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let uuid = uuid::Uuid::now_v7();
            let bytes = *uuid.as_bytes();
            std::fs::write(path, bytes)
                .map_err(|source| ShardError::uuid_file(path.to_owned(), source))?;
            Ok(bytes)
        }
        Err(source) => Err(ShardError::uuid_file(path.to_owned(), source)),
    }
}

// ---------------------------------------------------------------------------
// Compile-time invariants
// ---------------------------------------------------------------------------

const _: fn() = || {
    fn require_send_sync<T: Send + Sync>() {}
    require_send_sync::<ShardHandle>();
    require_send_sync::<Sender<ShardRequest>>();
    fn require_send<T: Send>() {}
    require_send::<ShardJoiner>();
};

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn shard_handle_is_send_sync_compile_check() {
        // Statically asserted above; this test exists so the file's
        // intent is discoverable from `cargo test` output.
    }

    #[test]
    fn shard_spawn_config_new_uses_arena_default_capacity() {
        let cfg = ShardSpawnConfig::new("/tmp/example");
        assert_eq!(cfg.channel_capacity, 1024);
        assert_eq!(cfg.pin_cpu, None);
        assert_eq!(
            cfg.arena_initial_capacity_slots,
            DEFAULT_INITIAL_CAPACITY_SLOTS
        );
    }

    #[test]
    fn read_or_generate_uuid_creates_file_when_absent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("shard.uuid");
        let uuid = read_or_generate_uuid(&path).expect("generate");
        let on_disk = std::fs::read(&path).unwrap();
        assert_eq!(on_disk.len(), 16);
        assert_eq!(&on_disk[..], &uuid[..]);
    }

    #[test]
    fn read_or_generate_uuid_returns_existing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("shard.uuid");
        let canonical = [0xAB_u8; 16];
        std::fs::write(&path, canonical).unwrap();
        let uuid = read_or_generate_uuid(&path).expect("read existing");
        assert_eq!(uuid, canonical);
    }

    #[test]
    fn read_or_generate_uuid_rejects_short_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("shard.uuid");
        std::fs::write(&path, b"short").unwrap();
        let err = read_or_generate_uuid(&path).expect_err("should reject");
        assert!(matches!(err, ShardError::UuidFile { .. }));
    }

    #[test]
    fn spawn_unbound_and_join() {
        let dir = TempDir::new().unwrap();
        let cfg = ShardSpawnConfig::new(dir.path());
        let (handle, joiner) =
            spawn_shard(0, cfg).expect("Glommio spawn should succeed with Unbound placement");
        assert_eq!(handle.shard_id(), 0);
        drop(handle);
        joiner.join().expect("shard should join cleanly");
    }
}
