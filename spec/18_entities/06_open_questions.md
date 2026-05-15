# 18.06 Open Questions

Entity-specific open questions. Wire-shape open questions live in [`../28_knowledge_wire_protocol/09_open_questions.md`](../28_knowledge_wire_protocol/09_open_questions.md).

## Active

### Q1 — Cross-shard merge coordination

[`./03_merge.md`](./03_merge.md) §9. Entities on different shards (multi-shard deployment) need a coordinated commit for merge. Two strategies:

(a) **Two-phase commit** across the affected shards. Holds locks across network hops; sensitive to partial failure.
(b) **Authoritative shard for the survivor** — survivor's shard runs the merge; the merged entity's shard is notified async and updates its row + redirects. Eventually consistent; queries through the merged id during the window may not yet see the redirect.

**Target:** phase 16.7. **Status:** open. **Likely outcome:** (b) for the v1.0 simplicity argument; (a) if benchmarks show the consistency window is unacceptable.

---

### Q2 — Multi-hop unmerge ordering

[`./04_unmerge.md`](./04_unmerge.md) §6: chain `merged → survivor → other`. Should unmerge of `merged` be allowed if `survivor` is itself merged?

Current spec: reject unless upstream is unmerged first. Operators do nested unmerges.

Alternative: allow, and re-route any statements that survivor → other took from the merged → survivor step. Complex; error-prone.

**Target:** phase 16.7. **Status:** open. **Likely outcome:** keep the rejection; force operator-driven ordering.

---

### Q3 — Cross-type merge

Should merging entities of different `entity_type` be allowed? Current v1.0 stance: forbidden (`ENTITY_TYPE_MISMATCH`).

Use case: extraction misclassified an entity. The fix is currently: `ENTITY_TOMBSTONE` the wrong one, `ENTITY_CREATE` a fresh one in the correct type, re-extract or manually re-author statements. Painful.

Alternative: allow cross-type merge with attribute drop (attributes valid in source type but not destination type are dropped).

**Target:** post-v1.0. **Status:** deferred.

---

### Q4 — Reverse-merge semantics

[`./04_unmerge.md`](./04_unmerge.md) §7: after `merged → survivor` was merged and then unmerged, can the operator re-merge them?

Currently: yes. Each merge is a fresh audit row.

Alternative: track repeated cycles; refuse after N (defaults: maybe 3) to prevent thrashing.

**Target:** post-v1.0. **Status:** deferred. **Likely outcome:** stay permissive; trust the operator.

---

### Q5 — `mention_count` truthfulness

[`./05_garbage_collection.md`](./05_garbage_collection.md) §3 recomputes `mention_count` during GC eligibility check because the stored counter may be stale (e.g. memories were forgotten without decrementing).

Should the stored `mention_count` be **maintained eagerly** (every `STATEMENT_CREATE` / `STATEMENT_TOMBSTONE` / `FORGET_MEMORY` updates it) or **lazily** (recomputed periodically)?

Eager: simpler queries that need an accurate count, but write amplification on every memory / statement op.
Lazy: cheap writes, but consumers must treat the stored count as approximate.

**Current spec:** eager during `STATEMENT_CREATE`, lazy during `FORGET_MEMORY` (resolver mention updates pass through entities once per memory).

**Target:** phase 17 (when statement ops land). **Status:** open. **Likely outcome:** lazy with a periodic reconcile worker.

---

### Q6 — Embedding refresh policy

When an entity's `canonical_name` changes (rename or merge contribution), `embedding_version` bumps and the phase-21 embedding worker eventually re-embeds. **When** does the worker run?

Options:

(a) On a schedule (e.g. every minute, scan for entities with `embedding_version > current_embedded_version`).
(b) Event-driven (subscribe to entity events, re-embed reactively).
(c) On read (lazy — re-embed when the resolver actually needs the embedding for this entity).

**Target:** phase 21. **Status:** open. **Likely outcome:** (a) for predictability; (c) is too query-time-expensive.

---

### Q7 — Attribute conflict policies per attribute vs per type

[`./03_merge.md`](./03_merge.md) §6 lists 5 conflict policies (`survivor_wins`, `merged_wins`, `newest_wins`, `concat_text`, `reject_merge`). Currently the policy is **per entity type** (declared in the type's schema definition).

Should it be **per attribute**? E.g. for `Person`: `name = survivor_wins`, `bio = concat_text`, `email = reject_merge`.

**Target:** phase 19 (schema DSL extension). **Status:** open. **Likely outcome:** per-attribute granularity in schema DSL.

---

### Q8 — Resolver tier 4 (LLM) integration with merge

The resolver's tier 4 ([`./01_resolution.md`](./01_resolution.md) §"Tier 4") is currently a stub. When phase 21 wires it up, the LLM may suggest merges with confidence in the auto-merge band. Should auto-merge from tier 4 be:

(a) Allowed (auto-merge if confidence ≥ 0.95).
(b) Always queued for human review regardless of confidence (LLM-sourced merges are inherently lower-trust).

**Target:** phase 21. **Status:** open. **Likely outcome:** (b) for safety; reconsider after empirical accuracy data.

---

### Q9 — GC-driven entity hard-delete and privacy

[`./05_garbage_collection.md`](./05_garbage_collection.md) §5: the wire protocol intentionally has no immediate hard-delete opcode. Privacy / GDPR / "right to erasure" requests are operator-driven offline operations.

Is this sufficient? Or should there be a `ENTITY_RETRACT` opcode (analogous to `STATEMENT_RETRACT`) that immediately tombstones and queues for accelerated reclamation?

**Target:** post-v1.0. **Status:** deferred. Discussion may move to the compliance section once one exists.

## Resolved

(none yet — §18 backfill is recent)
