# Plan: Phase 24 — Task 10, Schema-on / schema-off e2e test

**Status:** awaiting-confirmation
**Date:** 2026-05-17
**Author:** Claude (autonomous)
**Estimated commits:** 1
**Depends on:** 24.1, 24.8, 24.9.

---

## 1. Scope

Executable validation of the runbook in 24.9. Start with a
clean deployment + sample memories under no schema; declare a
schema; run backfill; verify hybrid query; verify substrate
primitives still work as before.

Phase doc says `tests/schema_toggle_e2e.sh`. v1 ships **both**
a Bash script driver (for CI / acceptance) **and** a Rust
integration test (`crates/brain-server/tests/schema_toggle_e2e.rs`)
that asserts the same behaviour through the in-process
harness — faster and finer-grained than the Bash run.

Concrete deliverables:

1. **`crates/brain-server/tests/schema_toggle_e2e.rs`** (new)
   — Rust integration test driving the full sequence via the
   support harness:
   1. spawn shard, no schema.
   2. ENCODE 30 memories with a mix of text content.
   3. RECALL — expect substrate path
      (`contributing_retrievers == []`).
   4. SCHEMA_UPLOAD trivial schema with a pattern extractor.
   5. RECALL — expect hybrid path
      (`contributing_retrievers` populated;
      `fused_score > 0`).
   6. Submit a backfill request covering all 30 memories.
   7. Wait for backfill to complete (poll progress).
   8. STATEMENT_LIST — expect statements created by the
      extractor against memories indexed by the pattern.
   9. RECALL one of the original memories — verify the
      substrate primitives are unaffected (memory still
      retrievable; vector / HNSW intact).
   10. Restart the server (server.stop() + new start_in()
       with same data dir); the SchemaGate re-seeds from
       metadata; recall is still hybrid.
2. **`scripts/schema-toggle-e2e.sh`** (new) — Bash driver
   that runs the corresponding sequence against a
   stand-alone `brain-server` binary using `brain-cli`.
   Used by 24.11's full acceptance suite.
3. **Fixture helpers** in
   `crates/brain-server/tests/support_harness/schema_toggle.rs`
   (new) — `seed_memories(client, n, prefix)`,
   `wait_for_backfill(client, request_id, timeout)`.

## 2. Spec references

- `spec/31_complete_acceptance/00_purpose.md` §"Schema-on /
  schema-off transitions acceptance" — the four bullet
  points this test must validate.
- `spec/28_knowledge_wire_protocol/08_schema_optional_mode.md`
  §1–§5 — gate semantics + transparent RECALL routing.

## 3. External validation

| Item | Source | Status |
|---|---|---|
| Support harness | `crates/brain-server/tests/support_harness/mod.rs` | shipped |
| `RECALL_REQ` transparent routing | 23.11 | shipped |
| Backfill request shape | 24.1 | new |
| `STATEMENT_LIST` filter | shipped + 24.4 ext | mixed |
| `brain-cli` covering all the above | post-v1 likely; v1 we use wire frames directly in Rust + a thin CLI wrapper in Bash | minor CLI gaps may exist; document in plan |

## 4. Architecture sketch

```
crates/brain-server/tests/schema_toggle_e2e.rs        (new)

#![cfg(target_os = "linux")]
mod support_harness;
use support_harness::{start_in, schema_toggle::*};
// ... usual wire helpers

#[tokio::test(flavor = "current_thread")]
async fn end_to_end_substrate_to_hybrid_and_back() {
    // Step 1: substrate-only deployment.
    let data_dir = TempDir::new().unwrap();
    {
        let server = start_in(data_dir.path(), 1).await;
        let mut client = connect_and_handshake(&server).await;

        // Step 2: encode memories.
        let memories = seed_memories(&mut client, 30, "prod_incident_").await;

        // Step 3: substrate RECALL (no hybrid metadata).
        let resp = recall(&mut client, "prod incident").await;
        for r in &resp.results {
            assert!(r.contributing_retrievers.is_empty());
            assert_eq!(r.fused_score, 0.0);
        }

        // Step 4: declare schema.
        let upload = schema_upload(&mut client, TRIVIAL_PATTERN_SCHEMA).await;
        assert!(upload.validation_errors.is_empty());

        // Step 5: hybrid RECALL.
        let resp = recall(&mut client, "prod incident").await;
        assert!(resp.results.iter().any(|r| !r.contributing_retrievers.is_empty()));
        assert!(resp.results.iter().any(|r| r.fused_score > 0.0));

        // Step 6-7: backfill + wait.
        let request_id = backfill_all(&mut client, "all").await;
        wait_for_backfill(&mut client, request_id, Duration::from_secs(30)).await;

        // Step 8: statements created.
        let stmts = statement_list(&mut client, ListFilters::all()).await;
        assert!(!stmts.items.is_empty());

        // Step 9: substrate primitive unaffected.
        let recalled = recall(&mut client, "prod incident").await;
        assert_eq!(recalled.results.len(), memories.len().min(recalled.top_k));

        server.stop().await;
    }

    // Step 10: restart; gate re-seeds.
    {
        let server = start_in(data_dir.path(), 1).await;
        let mut client = connect_and_handshake(&server).await;
        let resp = recall(&mut client, "prod incident").await;
        // Gate still set; recall still hybrid.
        assert!(resp.results.iter().any(|r| !r.contributing_retrievers.is_empty()));
        server.stop().await;
    }
}

scripts/schema-toggle-e2e.sh                          (new)
#!/usr/bin/env bash
set -euo pipefail
# Mirrors the Rust test, via brain-cli.
# Used by 24.11's acceptance suite.
```

### Why both forms

The Rust test is the **CI gate** (fast, deterministic, no
external binaries). The Bash script is the **acceptance
artifact** — it exercises the actual `brain-server` binary +
`brain-cli` operator surface that operators use in the
runbook. 24.11's `full-acceptance.sh` chains the Bash form.

## 5. Trade-offs considered

| Alternative | Pros | Cons | Verdict |
|---|---|---|---|
| Rust + Bash (this plan) | CI speed + binary-level coverage | Two files | ✓ |
| Rust only | Fastest | Doesn't exercise the binary the operator runs | rejected — acceptance gate wants binary coverage |
| Bash only | Acceptance-style | Slow; flaky; no fine-grained assertions | rejected |
| Test the runbook commands verbatim | Direct evidence | CLI gaps in v1; some steps don't have CLI commands | document gaps; Rust covers them |
| Skip restart step | Less complexity | Misses the gate-re-seed regression surface | keep |

## 6. Risks / open questions

- **Risk:** CLI gaps mean the Bash script can't faithfully mirror the runbook. **Mitigation:** Document gaps in the Bash script's comments; Rust test covers them. If the gap is large enough that the runbook is misleading, file an issue for v1.0.x CLI hardening.
- **Risk:** Backfill polling races with the test's wait — items finish faster than the polling loop. **Mitigation:** poll with `Duration::from_millis(50)` interval; cap at 30 s total.
- **Open question:** Should the test cover the LLM-extractor tier? **Resolution:** no — keeps the test fixture-free. LLM tier validation lives in 21.x integration tests (mocked) and phase-14 acceptance (real LLM).

## 7. Test plan

This sub-task IS the test. Validated by:

- `cargo test -p brain-server --test schema_toggle_e2e` passes.
- `bash scripts/schema-toggle-e2e.sh` against a built
  `brain-server` binary passes (validated in 24.11).

## 8. Commit shape

```
test(server,scripts): 24.10 — schema-toggle end-to-end test

- crates/brain-server/tests/schema_toggle_e2e.rs (new):
  Rust integration test driving substrate → hybrid →
  backfill → verify → restart sequence.
- crates/brain-server/tests/support_harness/schema_toggle.rs
  (new): fixture helpers (seed_memories, wait_for_backfill).
- scripts/schema-toggle-e2e.sh (new): Bash driver against
  the standalone binary; used by 24.11.

Verified: cargo zigbuild --target x86_64-unknown-linux-gnu
-p brain-server --tests; cargo clippy -- -D warnings.
```

## 9. Confirmation

1. **Both Rust + Bash forms** — Rust for CI, Bash for acceptance suite.
2. **10-step sequence** matches the 24.9 runbook (validate → declare → backfill → verify → restart).
3. **Restart step included** to catch SchemaGate re-seed regressions.
4. **LLM tier excluded** from this test (covered in 21.x + phase-14).
5. **CLI gaps documented** in the Bash script's comments, not as blocking issues.
