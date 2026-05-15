# RB-8: Substrate becoming unresponsive

**Linked alert:** composite — `BrainHighLatency` + clients reporting
timeouts. The substrate process is still running but isn't serving.

## Symptoms

- `/healthz` slow or timing out.
- Client requests time out.
- CPU may be at 100 % or 0 % (both are signals).

## Steps

1. **Is the metrics endpoint reachable?** If yes, the admin server
   is alive but data-plane is wedged:
   ```bash
   curl -s --max-time 2 http://<metrics-addr>/healthz
   ```

2. **CPU shape.**
   ```bash
   top -bn1 -p $(pgrep brain-server)
   ```
   - 100 % on one core: per-shard executor starvation — a long-
     running task or worker is hogging the Glommio reactor.
   - 100 % across all cores: insufficient capacity; need to scale.
   - 0 %: deadlock or stuck in I/O. Move to step 4.

3. **Active vs in-flight requests.**
   ```promql
   brain_request_active
   ```
   If non-zero and growing, the dispatch path is queuing. Check
   shard scheduler:
   ```promql
   brain_worker_last_run_unixtime
   ```

4. **Check for I/O wait.**
   ```bash
   iostat -x 1 5
   ```
   High `await` on the data device = disk is the bottleneck.

5. **Capture a profile.** Per-shard CPU profile (deferred
   primitive — Phase 12 tracker `phase-11/glommio-profiler`); for
   now use process-level:
   ```bash
   perf record -F 99 -g -p $(pgrep brain-server) -- sleep 30
   perf report --stdio | head -50
   ```
   The hot frames tell you whether to look at workers, request
   handlers, or storage.

6. **Mitigation.** If the profile shows a worker as hot, restart
   the substrate (per [RB-3](memory-pressure.md) step 4). If
   request handlers are hot, the substrate is under-provisioned —
   shed load (lower the rate at the client) or scale out.

## Escalate if

The substrate is unresponsive across two consecutive restarts.
Capture the profile + recent logs + dashboard screenshots and
escalate.
