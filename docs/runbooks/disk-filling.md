# RB-4: Disk filling

**Linked alert:** `BrainDiskFilling` *(deferred — metric set in
spec §14/05 §4 requires `node_filesystem_free_bytes` from
node_exporter, not brain-server itself)*

## Symptoms

`df -h` projects the data partition full within 24 hours, or
`node_exporter` `predict_linear(node_filesystem_free_bytes[1h], 24*3600) < 0`.

## Steps

1. **Identify the largest consumers.**
   ```bash
   du -sh /var/lib/brain/data/*
   ```
   Typical layout per shard: `arena.bin` (largest), `wal/` (active
   + retained segments), `metadata.redb`, `snapshots/`.

2. **Check WAL retention.** Spec §05/03 §6 sets default retention
   to 64 segments × 64 MiB = 4 GiB / shard. If `wal/` is larger,
   the retention worker may be stuck:
   ```promql
   brain_worker_last_run_unixtime{worker="wal_retention"}
   ```
   If stale: restart the worker, or restart the substrate.

3. **Delete old snapshots.** Each snapshot is a full copy.
   ```bash
   brain-cli snapshot list
   brain-cli snapshot delete --id <id>
   ```
   Keep at least the last clean one.

4. **Free old slots.** Spec §05/02 §5 tombstone-grace defaults to
   7 days. After the grace window, the `slot_reclamation` worker
   should reclaim. If you need to free space sooner, hard-FORGET
   the affected memories (spec §09/06).

5. **Add disk.** LVM extend is the cheapest path. The substrate
   doesn't move data between disks; for migration the operator
   stops the substrate, copies the data directory, and updates
   `[storage] data_dir`.

## Escalate if

The data directory is growing without correspondingly increased
write traffic and the workers are healthy. Likely a substrate bug
in the retention path — capture a snapshot of `wal/` for
engineering.
