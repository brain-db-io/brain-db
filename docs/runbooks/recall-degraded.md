# RB-6: HNSW recall degraded

**Linked alert:** `BrainRecallQualityDegraded` (recall < 0.85)

## Symptoms

`brain_hnsw_recall_estimate < 0.85` sustained for 30 minutes.
RECALL results feel "off" — known-good cues miss expected memories.

## Steps

1. **Check tombstone ratio first.** Most recall regressions are
   driven by tombstone accumulation:
   ```promql
   brain_hnsw_tombstone_ratio
   ```
   Threshold to act: `> 0.3`. Above that, the HNSW index has too
   many tombstoned nodes — search visits more candidates and
   recall drops.

2. **Trigger a rebuild.**
   ```bash
   brain-cli rebuild-ann --shard <id>
   ```
   Spec §06/05 §4 sets the rebuild target at ~10s of seconds for
   1 M memories on reference hardware. The substrate stays
   serving during the rebuild (reads keep working against the old
   index; the new index swaps atomically when ready).

3. **Monitor progress.** Rebuild progress isn't yet exposed as a
   gauge (deferred — `phase-12/hnsw-sampling`); use the
   admin endpoint:
   ```bash
   brain-cli stats --addr <metrics-addr> --shard <id>
   ```
   And the request span trace (Phase 12.3) shows the rebuild span.

4. **Verify recall is restored.** After rebuild:
   ```promql
   brain_hnsw_tombstone_ratio
   ```
   Should drop to near zero. Recall estimate (when implemented per
   `phase-12/hnsw-sampling`) should return above 0.95.

5. **Empirical check.** Run a known-good cue through RECALL and
   confirm the expected memory comes back:
   ```bash
   brain-cli recall --addr <data-addr> "<cue>" --k 10
   ```

## Escalate if

Rebuild completes but recall doesn't improve. That points to a
deeper issue — either the embedding model has drifted (the cache
should be invalidated by an `embedder_cache_evict` cycle) or the
HNSW parameters (`m`, `ef_construction`, `ef_search` in config)
are wrong for the data distribution.
