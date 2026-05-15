# RB-5: Worker stuck

**Linked alert:** `BrainWorkerStuck` (last_run > 2 h ago)

## Symptoms

A background worker hasn't completed a cycle in 2 hours.
`brain_worker_last_run_unixtime{worker="<name>"}` is stale.

## Steps

1. **Identify which worker.** The alert label tells you:
   ```promql
   time() - brain_worker_last_run_unixtime > 7200
   ```

2. **Check worker logs.** Each worker tags its events with a
   logger path like `brain.worker.decay`:
   ```bash
   journalctl -u brain-server --since "4h ago" \
     | grep '"target":"brain_workers::decay'
   ```

3. **Look for errors in the recent cycles.**
   ```promql
   rate(brain_worker_errors_total{worker="<name>"}[1h])
   ```
   A non-zero error rate without a successful run = stuck.

4. **Worker-specific causes:**
   - `decay`, `access_boost`: redb contention. Check
     `brain_metadata_*` metrics.
   - `hnsw_maintenance`: rebuild in progress can hold the worker
     mid-cycle for tens of seconds; not a bug.
   - `snapshot`: disk full → see [RB-4](disk-filling.md).
   - `wal_retention`: WAL segment lock; usually clears on next cycle.

5. **Try restarting the worker.** The CLI surface for live
   stop/start is deferred (Phase 12 tracker
   `phase-11/scheduler-control`); for v1 the only way to restart a
   stuck worker is to restart the substrate.

6. **Restart the substrate.** Coordinated restart per [RB-3](memory-pressure.md)
   step 4. The worker restarts in its `idle` state and runs its
   next scheduled cycle.

## Escalate if

The same worker gets stuck repeatedly across restarts. Capture the
last 4 hours of logs filtered by that worker's target, plus a
recent OTel trace if available.
