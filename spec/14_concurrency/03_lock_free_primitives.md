# 14.03 Lock-Free Primitives (ArcSwap + crossbeam-epoch)

> **TL;DR.** Two concrete crates do the lock-free heavy lifting: ArcSwap for atomic snapshot publication, crossbeam-epoch for safe memory reclamation. This section covers both — usage patterns, lifetimes, and the discipline that keeps them sound.

## ArcSwap

[`arc-swap`](https://github.com/vorner/arc-swap) is a Rust library providing atomic swap of `Arc<T>`. Brain uses it for publication.

## 1. The library

`arc-swap` provides:

- `ArcSwap<T>`: a wrapper around `Arc<T>` with atomic load/store.
- Lock-free, wait-free operations.
- Optimized for read-mostly workloads (load is the hot path).

GitHub: [vorner/arc-swap](https://github.com/vorner/arc-swap).

## 2. The API

```rust
let arcswap: ArcSwap<MyType> = ArcSwap::new(Arc::new(initial_value));

// Read
let arc: Arc<MyType> = arcswap.load_full();

// Write
arcswap.store(Arc::new(new_value));

// Swap (returns the previous Arc)
let old: Arc<MyType> = arcswap.swap(Arc::new(new_value));
```

## 3. The performance

`load_full()`:
- Returns a fresh Arc; the refcount is incremented.
- ~50-100 ns on modern hardware.

`load()` (a "loose" load):
- Returns a `Guard<T>` that's cheaper to acquire but holds a reference for the guard's lifetime.
- ~10-20 ns.

`store()`:
- Atomic write; replaces the Arc.
- The old Arc's refcount drops; freed when refcount reaches zero.
- ~50-100 ns.

For Brain's frequencies (millions of loads/sec, thousands of stores/sec), these costs are negligible.

## 4. Where Brain uses ArcSwap

| Use site | Purpose |
|---|---|
| Per-shard HNSW reference | Publish HNSW state changes |
| Per-shard configuration | Hot-reload settings |
| Routing table | Cluster reconfiguration |

These are read-mostly: reads happen per-request; stores happen rarely (publication, config reload, rebalancing).

## 5. The HNSW use case

```rust
struct ShardState {
    hnsw: ArcSwap<HnswState>,
    // ...
}

// Read path
fn search(&self, query: &[f32]) -> Vec<...> {
    let hnsw = self.hnsw.load_full();
    // hnsw is an Arc; valid for the duration of this function
    hnsw.search(query, ...)
}

// Write path
fn publish_new_hnsw(&self, new_state: HnswState) {
    let new_arc = Arc::new(new_state);
    self.hnsw.store(new_arc);
}
```

The reader's `load_full()` returns an Arc; while held, the Arc keeps the HnswState alive. Even if the writer publishes a new state, the reader's old state remains valid until dropped.

## 6. The Arc semantics

`Arc<T>`:
- Atomically refcounted shared ownership.
- `Clone`: increments refcount.
- `Drop`: decrements refcount; if reaches zero, drops the inner T.

For Brain's HNSW:
- The writer holds one Arc (for ongoing mutations or as the current "active" state).
- Each in-flight reader holds an Arc (returned by load_full).
- When old states are no longer referenced, they're freed.

## 7. The "load vs load_full" choice

`load_full()`:
- Returns `Arc<T>`.
- Increments refcount.
- Hold as long as you need.

`load()`:
- Returns a `Guard<T>` — a lightweight reference.
- The guard's lifetime bounds how long the reference is held.
- Cheaper acquisition but harder ergonomics.

Brain uses `load_full()` mostly. The 50 ns overhead is invisible compared to the work that follows.

## 8. The "store_full" alternative

ArcSwap also has variants like `swap` (returns the old value) and `compare_exchange` (CAS-style). Brain uses simple `store` mostly.

For coordinated swaps where Brain requires to verify the previous state, `compare_exchange` is available. Used rarely.

## 9. The atomicity guarantee

`store` and `load` are atomic in the C++/Rust memory model sense:

- A store has release semantics.
- A load has acquire semantics.

This means:
- Writes done before the store are visible to readers after the load.
- No partial state is observable.

For Brain's purposes, this is exactly what's required.

## 10. The lock-free progression

ArcSwap is **lock-free**:
- Operations complete in bounded time regardless of contention.
- A slow store doesn't block fast loads.

It's also **wait-free** for readers:
- A load completes regardless of what the writer is doing.

This gives predictable read latency. No "wait for the writer to finish" stalls.

## 11. The cost model

For Brain's typical frequency:
- Reads (load_full): ~10K/sec/shard. ~1 ms total CPU time.
- Stores: ~100/sec/shard (publications). ~0.01 ms total CPU time.

The library's overhead is < 1% of Brain's total cost.

## 12. The memory cost

Each ArcSwap holds:
- A pointer to the current Arc.
- ~16 bytes per ArcSwap.

The Arc itself holds the data plus refcount (~16 bytes overhead).

For Brain's number of ArcSwaps (a handful per shard): negligible memory.

## 13. The interaction with cloning

For HNSW, full deep-cloning is expensive. Brain does not clone the HNSW on every store; Brain uses structural sharing where possible.

When a major change requires a new HNSW (e.g., a maintenance rebuild), the new HNSW is built in the background, then a single ArcSwap.store publishes it. The old HNSW lives until refcounts drop.

## 14. The "stuck reader" risk

If a reader holds an Arc forever (a leak or a stuck task), the underlying state is never freed. Memory grows.

Brain mitigates by:
- Bounded read transaction lifetime.
- Task timeouts.
- Periodic monitoring of refcounts (an unusual count is a warning signal).

For typical workloads, readers complete in milliseconds; the issue doesn't arise.

## 15. The thread-safety properties

ArcSwap is `Send + Sync`. It can be shared across threads.

In Brain, ArcSwaps are typically per-shard; they're accessed only by the shard's executor's tasks. Brain does not usually share them across shards.

For routing tables (which all shards consult), there's a shared ArcSwap. Loads from many threads concurrently are fine.

## 16. The library's stability

ArcSwap is a mature library:
- v1.x stable for years.
- Used by many crates (tokio, etc.).
- Well-tested.

Brain pins to a specific version in `Cargo.toml`.

## 17. The alternatives considered

Alternatives to ArcSwap:

- **Manual `AtomicPtr`**: more control but harder to use safely.
- **`RwLock<Arc<T>>`**: simpler API but with locking overhead.
- **`crossbeam::atomic::AtomicCell<Arc<T>>`**: similar to ArcSwap but less optimized for this case.

ArcSwap is the most ergonomic and most optimized.

## 18. The "no need for ArcSwap" case

For data that doesn't change (e.g., compiled regex patterns), a plain `Arc<T>` is sufficient. Brain uses ArcSwap only when atomic publication is needed.

For data that changes frequently within a single thread (e.g., the writer's local state), no atomicity needed; just a regular variable.

## 19. The "ArcSwap only for shared mutable state" rule

ArcSwap is for state that's shared across tasks AND mutated. If state is shared but immutable, plain Arc works. If state is mutable but private, regular variable works.

This rule keeps usage clear: ArcSwap appearing in code signals "this is shared mutable state with publication semantics".

## 20. The summary

ArcSwap gives Brain:

- Lock-free, wait-free atomic publication.
- Simple API.
- Compatible with Rust's ownership model via Arc.

It's the right primitive for "atomically swap the current version of this data structure". Combined with the single-writer-per-shard discipline and Arc's automatic refcounting, it provides Brain's publication mechanism.

---

## crossbeam-epoch

[`crossbeam-epoch`](https://github.com/crossbeam-rs/crossbeam) provides epoch-based reclamation for lock-free data structures.

## 1. The library

Part of the crossbeam project. Provides:

- `Atomic<T>`: an atomic pointer with safe reclamation semantics.
- `Owned<T>`: ownership of an unpublished item.
- `Shared<'g, T>`: a shared reference within a guard.
- `Guard`: a reader's pin to an epoch.
- `pin()`: enter an epoch (returns a Guard).

GitHub: [crossbeam-rs/crossbeam](https://github.com/crossbeam-rs/crossbeam).

## 2. The use case in Brain

Brain uses crossbeam-epoch primarily for:

- Internal HNSW node management during incremental cleanup.
- Other lock-free data structures within a single shard's scope (e.g., free lists for slot allocation).

For most Brain code, ArcSwap + Arc handle reclamation. crossbeam-epoch is for cases where Arc isn't ergonomic (e.g., for plain pointers or for fine-grained items within a structure).

## 3. The basic pattern

```rust
use crossbeam_epoch::{self as epoch, Atomic, Owned, Shared};

struct LockFreeStructure {
    head: Atomic<Node>,
}

// Reader
fn read(&self) {
    let guard = epoch::pin();
    let head = self.head.load(Ordering::Acquire, &guard);
    // ... use head ...
}

// Writer (single-writer per shard in Brain)
fn write(&self, item: Node) {
    let guard = epoch::pin();
    let new = Owned::new(item);
    let old = self.head.swap(new, Ordering::AcqRel, &guard);
    unsafe {
        guard.defer_destroy(old);   // Deferred free
    }
}
```

The reader pins to an epoch. The writer swaps in a new value and tags the old for deferred destruction. The old is freed once all readers have advanced past the current epoch.

## 4. The slot free list

One concrete use: the arena's slot free list.

When a slot is reclaimed, it's added to the free list. Allocators take from the list.

```rust
struct FreeList {
    head: Atomic<FreeNode>,
}

fn push(&self, slot: SlotId) {
    let guard = epoch::pin();
    let new = Owned::new(FreeNode { slot, next: Atomic::null() });
    loop {
        let head = self.head.load(Ordering::Acquire, &guard);
        new.next.store(head, Ordering::Release);
        if self.head.compare_exchange(head, new, Ordering::AcqRel, Ordering::Acquire, &guard).is_ok() {
            break;
        }
    }
}
```

(In Brain, the writer-per-shard discipline obviates the CAS loop — the writer is the only mutator. Simplified accordingly.)

## 5. The single-writer simplification

With single-writer-per-shard, much of crossbeam-epoch's complexity goes unused:

- No CAS loops needed (no concurrent writers).
- The writer can use simple atomics with the writer-only access pattern.

Brain uses crossbeam-epoch for the **reclamation** aspect — its tracking of "when is it safe to free?" — even though Brain does not need its full lock-free machinery.

## 6. The epoch advance

The library maintains a global epoch counter. Periodically:

- The library advances the global epoch.
- Threads' "old epoch" values can be advanced.
- Memory tagged for deferred destruction in epochs that all threads have left can be freed.

The advance is automatic (every few hundred operations) but can be triggered explicitly if Brain wants forced cleanup.

## 7. The "guard scope" rule

A Guard's lifetime defines what's safe to access:

```rust
let guard = epoch::pin();
let shared: Shared<'_, T> = atomic.load(Ordering::Acquire, &guard);
// shared is valid here
drop(guard);
// shared is invalid here (compiler enforces via lifetime)
```

The `'g` lifetime on `Shared<'g, T>` is bound by the Guard. The compiler prevents using a `Shared` after its Guard is dropped.

This is part of Rust's safe-by-default API for unsafe-ish concurrent code.

## 8. The "unsafe defer_destroy" caveat

`Guard::defer_destroy` is an unsafe operation:

```rust
unsafe {
    guard.defer_destroy(old);
}
```

The unsafety is because the caller asserts that the data won't be freed multiple times. Brain's writer-per-shard discipline ensures this — only one task is calling defer_destroy on any given item.

Every use of `defer_destroy` in the codebase is audited. There's no place where it could be called twice.

## 9. The cost of pinning

`epoch::pin()`:
- Atomic load + atomic store (track per-thread state).
- ~20-50 ns.

Cheap enough to call per-operation. Brain calls it per-search and per-write.

## 10. The cost of advance

Epoch advance:
- Check all threads' epochs (~10 ns × thread count).
- Atomic increment of global counter.
- Possibly free a batch of deferred items.

The cost is amortized across many operations. Per-advance: ~50-100 ns.

## 11. The "garbage queue"

Each thread maintains a per-thread garbage queue:

- Items defer_destroy'd while pinned in epoch E go to the thread's queue with epoch tag E.
- When the global epoch advances past E (and all threads have advanced), items in queue with tag E can be freed.

This is per-thread to avoid contention. The library handles the bookkeeping.

## 12. The "retire" pattern

For dropping non-pointer resources (e.g., closing a file when no readers reference the descriptor):

```rust
guard.defer(|| close_file(fd));
```

The closure runs after the epoch advances. Equivalent to defer_destroy but for arbitrary cleanup.

## 13. The interaction with Loom

Loom is a Rust concurrency model checker. crossbeam-epoch has Loom-tested versions for verifying correctness in test environments.

Brain's tests run under Loom for the lowest-level concurrent code paths. This catches subtle ordering bugs.

## 14. The "epoch bound" trade-off

Epoch-based reclamation has a delay: items aren't freed immediately when defer_destroy'd. They wait for the epoch to advance.

For typical Brain workloads:
- Average delay: ~10-100 µs (until next epoch advance).
- Worst case: bounded by the slowest reader's pin duration.

For most use cases, this delay is fine. For memory-pressured workloads, the delay can cause peak memory to be slightly higher than steady-state.

## 15. The "no GC pause" claim, revisited

crossbeam-epoch doesn't have stop-the-world pauses:

- Readers proceed without waiting.
- Writers proceed without waiting (with one exception: if reclamation must happen and all readers are pinned, the writer waits — but this is rare).

Compared to traditional GC, no large pauses; just small periodic increments.

## 16. The "memory ordering" awareness

crossbeam-epoch APIs require explicit memory orderings:

```rust
let head = self.head.load(Ordering::Acquire, &guard);
self.head.store(new, Ordering::Release, &guard);
```

The orderings:
- Acquire: prevents reordering of subsequent reads.
- Release: prevents reordering of preceding writes.
- AcqRel: both.
- SeqCst: full barrier (most expensive; rarely needed).

Brain uses Acquire/Release for normal paths; SeqCst for extra-careful cases.

## 17. The "default features" choice

crossbeam-epoch has features for:

- `std`: standard library (default; Brain uses this).
- `nightly`: nightly compiler features (Brain does not use).
- `loom`: Loom integration for testing (enabled in test builds).

Brain enables std and loom-in-test.

## 18. The library version

Brain pins to a specific version of crossbeam-epoch in Cargo.toml. The library has been stable for years; updates are mostly bug fixes and performance improvements.

## 19. The alternative: hazard pointers

Hazard pointers are an alternative to epochs:

- Each reader publishes a "hazard pointer" to the data it's about to read.
- Writers check all hazard pointers before freeing.

Pros: more precise (only the specific item is protected, not all data in an epoch).

Cons: more complex API; harder to use safely.

Brain chose epochs over hazards for ergonomics. The precision difference doesn't matter for Brain's workload.

## 20. The summary

crossbeam-epoch provides safe lock-free reclamation:

- Readers pin cheaply.
- Writers tag old data for deferred free.
- Memory is freed when no readers can possibly access it.

Brain uses it for fine-grained reclamation within data structures. The high-level publication uses ArcSwap. Together, they form Brain's no-locks reader path.

---

*Continue to [`04_yields.md`](04_yields.md) for cooperative yielding.*
