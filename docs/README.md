# Brain documentation

Welcome. Pick the path that matches what you're trying to do.

## I'm new to Brain — show me what it does

Start with the [getting-started tutorial](tutorials/01-getting-started.md).
It walks from `cargo install` to a working hybrid query in
about 15 minutes.

For background — what Brain *is*, why it exists, the design — read
[`../README.md`](../README.md) and [`../spec/00_master_overview/`](../spec/00_master_overview/).

## I want to install / configure / run / upgrade

The how-to guides:

| Task | Guide |
|---|---|
| Install Brain | [`guides/install.md`](guides/install.md) |
| Configure a deployment | [`guides/configure.md`](guides/configure.md) |
| Operate Brain day-to-day | [`guides/operate.md`](guides/operate.md) |
| Upgrade an existing deployment | [`guides/upgrade.md`](guides/upgrade.md) |
| Wire observability (metrics + traces + logs) | [`guides/observability.md`](guides/observability.md) |

## Something's wrong — I need a runbook

[`runbooks/README.md`](runbooks/README.md) — RB-1 through RB-11.
Alert annotations point here.

## I need reference numbers — perf targets, error codes, etc.

The [`reference/`](reference/) directory:

- [`reference/performance.md`](reference/performance.md) — latency
  and throughput targets per phase.

## I'm contributing — build, test, debug

The [`development/`](development/) directory:

- [`development/usage/`](development/usage/) — build, run, debug,
  test workflow.
- [`development/spec-deviations.md`](development/spec-deviations.md)
  — places where the implementation knowingly diverges from the
  spec, with rationale.
- [`development/phases/`](development/phases/) — per-phase plans
  and sub-task histories. Read these when you're implementing a
  phase or auditing one.

## Where else to look

- [`../spec/`](../spec/) — the authoritative design. The spec is
  read-only; code disagreements get fixed in the code, not the
  spec.
- [`../ROADMAP.md`](../ROADMAP.md) — phase index.
- [`../CHANGELOG.md`](../CHANGELOG.md) — release history.
- [`../monitoring/`](../monitoring/) — Grafana dashboards +
  Alertmanager rules (deployment assets, not docs).
- [`../CONTRIBUTING.md`](../CONTRIBUTING.md) — how to contribute.

## Layout (Diátaxis)

The doc structure follows the [Diátaxis framework](https://diataxis.fr/):

| Type | Audience | Goal |
|---|---|---|
| `tutorials/` | New users | **Learning** by doing |
| `guides/` | Working users / operators | **Getting things done** |
| `reference/` | All audiences | **Looking things up** |
| `runbooks/` | Operators in an incident | **Resolving a problem** |
| `development/` | Contributors | **Working on Brain itself** |

If a document doesn't fit one of those buckets cleanly, it
probably belongs in `../spec/` (authoritative design) or
inline rustdoc (API reference).
