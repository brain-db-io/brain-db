# Brain — monitoring assets

Deployment-time observability artifacts. Not docs (those are
in [`../docs/`](../docs/)); these are JSON / YAML config the
operator imports into their Prometheus + Grafana +
Alertmanager stack.

## Layout

- [`dashboards/`](dashboards/) — Grafana dashboard JSON, one
  file per board (overview, per-shard, HNSW, storage, errors,
  network, workers, capacity). See
  [`dashboards/README.md`](dashboards/README.md) for the
  per-dashboard purpose.
- [`alerts/`](alerts/) — Alertmanager rule files. Currently
  one file: `brain-rules.yml` covering the rule taxonomy in
  [`../docs/guides/observability.md`](../docs/guides/observability.md).

## Importing

### Grafana

```bash
# Pick a dashboard and POST it to Grafana's API:
curl -X POST -H "Content-Type: application/json" \
     -H "Authorization: Bearer $GRAFANA_TOKEN" \
     --data @monitoring/dashboards/overview.json \
     https://grafana.example/api/dashboards/db
```

Or via the UI: Dashboards → Import → upload the JSON.

### Prometheus / Alertmanager

Drop `alerts/brain-rules.yml` into your Prometheus
`rule_files:` glob. Alerts annotate their runbook URLs back
into [`../docs/runbooks/`](../docs/runbooks/) for the
on-call rotation.

## See also

- [`../docs/guides/observability.md`](../docs/guides/observability.md)
  — operator-facing setup guide.
- [`../spec/18_observability/`](../spec/18_observability/00_purpose.md)
  — authoritative metric / tracing / logging design.
- [`../docs/runbooks/`](../docs/runbooks/) — per-alert
  procedures.
