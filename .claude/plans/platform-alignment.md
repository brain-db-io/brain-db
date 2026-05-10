# Plan: Lock in Linux + Glommio as Day-1; Align Docs

**Status:** awaiting-confirmation
**Date:** 2026-05-10
**Author:** Claude (autonomous)
**Estimated commits:** 3–4

---

## 1. Scope

User direction: commit to **Glommio on Linux from day one** rather than building portable shims. Update docs/plans/workflow so this stance is locked in and we don't redo work later.

**Out of scope:**

- Spec edits. Per AUTONOMY §2 the spec is read-only to autonomous Claude — and a survey shows the spec is *already* explicit ("Linux server", Glommio, io_uring, pwritev2(RWF_DSYNC), O_DIRECT). No edits needed.
- Retroactive code changes. Phase 1 (`brain-protocol`) doesn't touch I/O or runtime; nothing to revisit.

## 2. Survey: what's already aligned vs. what isn't

| Doc | Status |
|---|---|
| `spec/` | ✓ Already Linux + Glommio explicit |
| `CLAUDE.md` §4 | ✓ "Linux server. Connection layer (Tokio) … Glommio executor (thread-per-core, io_uring) … pwritev2(RWF_DSYNC)" |
| `CLAUDE.md` §9 | ✓ "Don't add Tokio inside a shard"; "Don't use tokio::fs in shard code" |
| `ROADMAP.md` line 215 | ✓ "Linux only. Glommio + io_uring don't run elsewhere." |
| `Cargo.toml` workspace deps | ✓ glommio = "0.9" pinned |
| `.github/workflows/ci.yml` | ✓ ubuntu-latest |
| `README.md` line 50 | ✓ "Glommio … Linux-only" |
| `AUTONOMY.md` | ✗ No platform section — should explicitly require Linux for any code that touches the runtime or libc syscalls |
| `docs/phases/phase-02-storage.md` | △ Mostly aligned (uses `pwritev2`, `mremap`); two phrases still hedge cross-platform — minor cleanup |
| `.claude/plans/phase-02.md` (drafted) | ✗ Argues for `std::fs` + cross-platform fallbacks; needs rewrite to commit to Glommio + Linux |
| Dev-setup doc | ✗ Missing — non-Linux developers (including current Claude session on darwin) need explicit guidance |

## 3. Proposed changes

### 3.1 `AUTONOMY.md` — add §22 "Platform"

A new short section pinning Linux as the supported target for any code that:
- imports `glommio::*`,
- imports `libc::pwritev2` / `libc::mmap` / similar Linux-specific syscalls,
- touches I/O / scheduling / arena / WAL / index persistence.

For non-Linux dev environments (macOS in particular), the section names the supported approaches: Linux dev container (Docker), Linux VM, or cloud Linux box. CI is the verification gate.

### 3.2 `README.md` — extend the Architecture section

Add a "Building and testing" subsection that says:

- Linux x86_64 / aarch64 with kernel ≥ 4.7 is the only supported target.
- macOS / Windows are not supported for build or test of crates that touch the runtime or storage. They *can* build `brain-core`, `brain-protocol`, and `brain-cli` (no runtime/I/O).
- Dev container (`.devcontainer/`) and a documented Docker workflow are the primary paths for non-Linux developers.

### 3.3 `DEV_SETUP.md` (new, top-level)

Concrete how-tos:

- Docker dev container — `docker run -it --rm -v $PWD:/work -w /work rust:1-slim bash` as the minimum; recommended `.devcontainer/devcontainer.json` for editor integration.
- Cross-compile-only check on macOS (`cargo check --target x86_64-unknown-linux-gnu`) — validates compilation without runtime; document the `--target` setup.
- CI as the actual test gate; PR description should note "verified locally in Linux container" when the diff touches runtime/storage code.

### 3.4 `.claude/plans/phase-02.md` — rewrite

Replace the "std::fs + cross-platform fallbacks" stance with:

- Storage code uses libc Linux syscalls directly (`pwritev2(RWF_DSYNC)`, `mmap`, `mremap`, `fallocate`, `open(O_DIRECT)`).
- `brain-storage` crate is gated `#[cfg(target_os = "linux")]` at the top of its lib.rs; non-Linux gets a stub crate that errors at compile time with a clear message ("brain-storage requires Linux"). Or (cleaner) the crate just doesn't compile on non-Linux and CI/dev container is the only path.
- Glommio integration in storage **stays deferred to Phase 9 for the *executor* boundary**, but Linux-specific syscalls land in storage from sub-task 2.4 onward.

The async API of `Wal::append` returns `impl Future`; in Phase 2 we use `std::future` adapters; Phase 9 swaps in Glommio-aware drivers without changing the API.

### 3.5 `docs/phases/phase-02-storage.md` — minor edits

- Sub-task 2.4 "open, mmap, grow" — explicitly call out Linux `mmap` + `mremap` (not "platform-detected").
- Sub-task 2.8 "Group commit" — strike any "fallback" language; pin `pwritev2(RWF_DSYNC)`.
- Sub-task 2.11 "Random-kill recovery test" — note that real chaos uses Linux `kill -9` in CI; in-process `mem::forget` is for dev sanity.

### 3.6 `.claude/skills/brain-glommio-rules/SKILL.md`

Already exists; review for completeness. Likely no change needed — it already enforces the no-Tokio-in-shard rule.

## 4. Workflow consequence — important

I am running in this session on **darwin (macOS)**. With Linux-only storage:

- **I can't run `cargo build` / `cargo test` / `cargo clippy` for `brain-storage`** locally. Glommio's transitive deps (`liburing-sys`, kernel-feature shims) will fail to link or compile on macOS.
- **I can run `cargo check --target x86_64-unknown-linux-gnu`** if a Linux cross-compile toolchain is installed. That validates compilation but not behavior. Setting this up requires `rustup target add x86_64-unknown-linux-gnu` and a Linux linker (`ld-lld` or a sysroot).
- **I cannot run miri, fuzz, or the random-kill test locally.** All of those run in CI.

This means my normal verify loop (AUTONOMY §4: `cargo build && cargo test && cargo clippy`) breaks on darwin for storage code.

**Three options to surface to the user (ask in §6):**

1. **Cross-compile-only on darwin.** I run `cargo check --target x86_64-unknown-linux-gnu` to validate compilation; rely on CI for tests. Requires installing the Linux target locally. Slower iteration; CI cycles are long.
2. **Linux dev container.** User provisions a Docker container or remote Linux box; I run cargo via that. Best iteration speed for me.
3. **CI-only verification.** I write code, commit, push to a branch; CI runs; I read CI output and iterate. Slowest iteration; works without local setup.

Of the three, **(2) is best** if the user has Docker/Colima available. I'd pair it with `cargo check --target x86_64-unknown-linux-gnu` locally for fast feedback before container test runs.

## 5. Implementation phases

Once approved:

1. **Commit 1** — `chore(workflow): pin Linux + Glommio as day-1 platform`
   - `AUTONOMY.md` §22 added.
   - `README.md` "Building and testing" subsection added.
   - `DEV_SETUP.md` written.
2. **Commit 2** — `chore(plans): rewrite phase-02 plan for Linux + Glommio from day 1`
   - `.claude/plans/phase-02.md` rewritten per §3.4.
   - `docs/phases/phase-02-storage.md` minor cleanup per §3.5.
   - Skills audit per §3.6 (probably no edit).
3. **(If user picks option 1 or 2)** Commit 3 — `chore(workflow): document cross-compile setup` or `chore(workflow): add .devcontainer`.

Each commit is small (docs only); no code or tests.

## 6. Confirmation needed on three points

1. **Spec edits.** I won't touch `spec/` per AUTONOMY §2 unless you tell me to. Survey shows spec is already aligned, so I propose no spec edits. Confirm.
2. **Dev-environment path** for Claude on darwin (§4):
   - **(a)** Cross-compile-only — slow but no infra.
   - **(b)** Linux dev container — recommended; needs Docker / Colima.
   - **(c)** CI-only — slowest; no local setup.
3. **Storage crate gating** (§3.4): hard `#[cfg(target_os = "linux")]` (crate refuses to compile elsewhere) or `cfg`-gated stubs that emit a friendly compile-time error message? Stubs are slightly nicer for `cargo check --workspace` on macOS (the workspace still resolves) but adds noise.

## 7. Confirmation

Awaiting "go" or specific revisions on the three points in §6.
