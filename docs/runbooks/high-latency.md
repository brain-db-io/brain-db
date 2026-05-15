# RB-2: High latency on a shard

**Linked alert:** `BrainHighLatency` (p99 > 100 ms for 10 min)

## Symptoms

p99 latency for one or more operations sustained above spec
§16/02 §2 targets. Dashboard `Brain — Overview` shows the
"Latency p99 by op" panel red.

## Steps

1. **Identify which operation is slow.** In Grafana on the
   per-shard dashboard, the latency-by-op panel highlights the
   offender. Or in Prometheus directly:
   ```promql
   histogram_quantile(
     0.99,
     sum by (op, le) (rate(brain_request_duration_ms_bucket[5m]))
   )
   ```

2. **Check resource exhaustion.**
   - CPU: `top -bn1 | head -20` — is one core pinned?
   - Memory: `process_memory_resident_bytes` — close to host limit?
   - Disk I/O: `iostat -x 1` — high `%util` on the data device?

3. **Check HNSW health.**
   ```promql
   brain_hnsw_tombstone_ratio
   brain_hnsw_node_count
   ```
   If `tombstone_ratio > 0.3` sustained, trigger a rebuild:
   ```bash
   brain-cli rebuild-ann --shard <id>
   ```
   (See also [RB-6](recall-degraded.md).)

4. **Check embedder bottleneck.** Deferred metric set — operators
   relying on a hosted embedder should check that backend's
   dashboards. Local model: check `process_cpu_seconds_total` rate
   per shard.

5. **Capture a profile.** Phase 12.3 OTel traces show per-request
   span breakdown:
   ```bash
   # If your OTel backend is Tempo/Jaeger:
   #   open the trace UI, filter by op=<slow_op>, sort by duration.
   ```
   For sub-process profiling:
   ```bash
   perf record -F 99 -g -p $(pgrep brain-server) -- sleep 30
   perf report
   ```

## Escalate if

Issue isn't resolved in 30 minutes. Capture:
- Recent dashboard screenshots (last 1 h).
- One or two slow OTel traces.
- `brain-cli stats --addr <metrics-addr>` output.
- Any deploys / config changes in the last 24 h.
