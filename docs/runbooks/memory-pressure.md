# RB-3: Memory pressure / OOM

**Linked alert:** `BrainHighMemoryPressure` (RSS > 8 GiB sustained;
tune for your host)

## Symptoms

`process_memory_resident_bytes` is approaching the host's RAM
budget. Risk of the kernel OOM-killing brain-server.

## Steps

1. **Identify which process / shard is heavy.** If you run one
   brain-server per host this is straightforward; for multi-tenant
   hosts:
   ```bash
   ps -eo rss,comm,pid --sort -rss | head
   ```

2. **Check for ongoing HNSW rebuild.** A rebuild temporarily doubles
   the index footprint:
   ```promql
   brain_hnsw_rebuild_in_progress  # deferred — falls back to logs
   ```
   If a rebuild is in progress: wait for it. RSS drops once the
   swap completes.

3. **Check write rate vs data growth.** The arena grows with data
   (~1.6 KB / memory). If RSS growth outpaces
   `brain_hnsw_node_count` growth, suspect a leak — capture a heap
   snapshot for engineering.

4. **Short-term mitigation.** Restart the substrate:
   ```bash
   systemctl restart brain-server
   ```
   Memory drops; the WAL recovery on restart costs seconds-to-
   minutes depending on retention.

5. **Medium-term mitigation.** Scale up RAM on the host, or scale
   out by sharding the data across more brain-server instances
   (each instance owns N shards; horizontal scaling shrinks N per
   instance).

## Escalate if

RSS grows without proportional data growth across two consecutive
restarts. That's a leak; engineering needs a heap snapshot:
```bash
# attach jemalloc heap profiler or run under valgrind massif
```
