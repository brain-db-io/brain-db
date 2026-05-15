# Brain — Grafana dashboards

Reference dashboards Brain ships per spec §14/04. Eight JSON files, each
self-contained and importable into Grafana 9+:

| File | Spec § | Purpose |
|---|---|---|
| `overview.json` | §14/04 §2 | High-level health — "first dashboard to look at" |
| `per-shard.json` | §14/04 §3 | Detailed view of a single shard (variable `$shard`) |
| `storage.json` | §14/04 §4 | Arena, WAL, metadata utilization |
| `hnsw.json` | §14/04 §5 | Index health — node count, tombstones, recall |
| `workers.json` | §14/04 §6 | Background worker status |
| `network.json` | §14/04 §7 | Connections + protocol |
| `errors.json` | §14/04 §8 | Error rate + breakdown |
| `capacity.json` | §14/04 §9 | Utilization + growth |

## Importing

Grafana UI → Dashboards → New → Import → upload one of the JSON files.

Set the Prometheus datasource UID prompt to your Prometheus instance.

For Kubernetes deployments using Grafana's file-provisioning sidecar,
drop these into the dashboard ConfigMap path.

## Metric coverage

The dashboards reference every metric family `brain-server` emits as of
Phase 12.1 (build_info, up, shards_total, request_total / _active /
_duration_ms, worker_cycles_total / _processed_total / _errors_total /
_last_run_unixtime, connections_active / _total, config_info, process_*).

Some panels reference metrics deferred to later sub-tasks (HNSW node
count, storage `_wal_size_bytes`, embedder calls). Those panels render
"No data" until the corresponding metric primitives land — see the
deferred-set comments in `crates/brain-server/src/metrics/mod.rs`.

## Customising

These are starting points (spec §14/04 §10). Operators should:

- Add panels for their specific concerns.
- Tune thresholds and time windows.
- Combine with their own application-level metrics.

JSON changes go through code review (spec §14/04 §15). The CI test
`dashboards_parse_and_reference_valid_metrics` (in
`crates/brain-server/tests/dashboards.rs`) catches typos before merge.

## Grafana version

Tested against Grafana 11.x; the JSON schema is `schemaVersion: 39`.
Older Grafana versions may need adjustments to panel options.
