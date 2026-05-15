# RB-10: Network partition (v2 clustering)

**Status:** v2 only. v1 brain-server is single-host per shard; a
"partition" in v1 means clients can't reach the substrate, which is
a deployment-side issue rather than a substrate-side one.

This runbook scaffolds the v2 procedure so the shape exists when
clustering lands.

## Symptoms (v2)

Some shards report `up=1`, others `up=0`. The cluster is in a
degraded state where part of the keyspace is reachable and part
isn't.

## Steps (v2 stub)

1. **Identify the partitioned shards.**
   ```promql
   brain_up == 0
   ```
   By shard label. The majority side will be serving; the minority
   side will be unreachable.

2. **Investigate the network.**
   - Ping reachable from the broker / load-balancer?
   - Recent firewall changes?
   - DNS / service discovery healthy?

3. **Don't fail over yet.** v2 clustering (when it lands) will use
   a quorum-based design — failover happens automatically when the
   majority side observes the minority for `min_partition_secs`.
   Manual failover risks split-brain.

4. **Resolve the network.** Most partitions are transient
   (security group misconfig, expired cert, DNS hiccup). Fix the
   network and the cluster auto-reconciles.

5. **If the partition won't heal:** engage clustering ops. Manual
   intervention to reduce the cluster size to the majority side
   should preserve durability.

## v1 fallback

For v1 (single-host per shard), a "client can't reach
brain-server" symptom is one of:

- TCP listener not bound — see [RB-1](substrate-down.md).
- Process running but unresponsive — see [RB-8](unresponsive.md).
- Network-layer issue between client and host — operator-side
  troubleshooting, not a runbook for brain-server itself.

## Escalate if

The network is healthy and the partition persists. This is a
clustering bug — engineering needs the affected nodes' logs and
gossip state.
