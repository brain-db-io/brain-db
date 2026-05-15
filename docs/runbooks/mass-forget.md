# RB-9: Mass FORGET tombstone aftermath

**Linked alert:** `BrainHighTombstoneRatio` (tombstone_ratio > 0.30
for 1 h)

## Symptoms

After a large delete operation (e.g. an agent decommission that
hard-FORGETs thousands of memories), recall latency rises and recall
quality drops. Spec §06/05 §2 + §11/04 §3.

## Why this happens

HNSW doesn't natively support deletion. Brain uses **tombstones**:
deleted memories stay in the graph but are marked unreachable. The
search path skips them but still visits adjacent nodes; the index
gradually becomes inefficient.

The `hnsw_maintenance` worker rebuilds when the tombstone ratio
crosses `0.3` (spec §06/05 §4). If the worker is slow or wasn't
running, the ratio can climb.

## Steps

1. **Confirm the cause.**
   ```promql
   brain_hnsw_tombstone_ratio
   brain_hnsw_node_count
   brain_hnsw_tombstone_count
   ```
   Sanity-check: `tombstone_count / node_count == tombstone_ratio`.

2. **Check the worker.** If it's stuck, see [RB-5](worker-stuck.md):
   ```promql
   time() - brain_worker_last_run_unixtime{worker="hnsw_maintenance"}
   ```

3. **Trigger a rebuild manually.** Don't wait for the worker:
   ```bash
   brain-cli rebuild-ann --shard <id>
   ```
   Spec §06/05 §4 sets the rebuild time at ~10s of seconds for
   1 M memories on reference hardware. Reads keep working during
   the rebuild against the old index.

4. **Verify the ratio drops.**
   ```promql
   brain_hnsw_tombstone_ratio
   ```
   Should fall to near zero after the rebuild. The
   `brain_hnsw_node_count` reflects only the still-active set.

5. **Spot-check recall.** Send a known cue through RECALL and
   confirm the expected memories return:
   ```bash
   brain-cli recall --addr <data-addr> "<cue>" --k 10
   ```

## Escalate if

The rebuild completes but tombstone ratio climbs again within an
hour. That implies a continuous FORGET stream — either operator-
driven or a bug; investigate the agent emitting the deletes.
