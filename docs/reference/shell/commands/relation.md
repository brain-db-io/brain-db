# `brain relation`

Browse typed relations — directed edges between two entities (a
`Person --[works_at]--> Org`, an `Author --[wrote]--> Document`).
Relations are the entity-↔-entity analogue of statements; both
materialise during extraction. The verb covers `list`.

**Knowledge layer prerequisite.** Relations exist only when a schema
declares relation types and the extractor pipeline runs. On
substrate-only deployments these subcommands return empty rows. See
[`spec/02_data_model/`](../../../../spec/02_data_model/00_purpose.md) for the
storage and traversal model.

---

## `brain relation list`

Filtered table of relations. Requires `--from`, `--to`, or both —
unfiltered scans of the relation table are deliberately rejected to
stop accidents on large graphs.

```
brain relation list
        (--from <UUID> | --to <UUID> | --from <UUID> --to <UUID>)
        [--type <QNAME>]
        [--limit <N>]
```

### Flags

#### `--from <UUID>`

Source-entity filter. Queries the per-shard outbound-relations index
(`relation_by_from`). Pass an `EntityId` UUID, dashed or undashed.

```bash
brain relation list --from 0190f8c0-1c1d-7c41-9b88-0acdb5e6f0e1
```

#### `--to <UUID>`

Target-entity filter. Queries the inbound index (`relation_by_to`).

```bash
brain relation list --to 0190f8e1-1c1d-7c42-9b88-12cb8e2a4d0c
```

#### `--from <UUID> --to <UUID>` (both)

Server-side cross-filter isn't exposed. The shell runs the `--from`
side server-side, then filters the result set **client-side** by
`to_entity == --to`, and emits a `tracing` warning. Effective when the
`--from` cardinality is small; pathological for hub entities. See
[Gated features](#gated-features).

```bash
# Effective for typical entity pairs
brain relation list --from 0190f8c0-… --to 0190f8e1-…
```

#### `--type <QNAME>`

Relation-type filter (`works_at`, `manages`, …). Passes through to
the SDK builder's `with_type`.

#### `--limit <N>`

Maximum rows. Default `50`.

### Output

#### Table

```
+--------------+------------+----------------+----------------+------+
| id           | type       | from           | to             | conf |
+--------------+------------+----------------+----------------+------+
| 0190f9a0-…   | works_at   | 0190f8c0-…     | 0190f8e1-…     | 0.95 |
| 0190f9a1-…   | manages    | 0190f8c0-…     | 0190f8c2-…     | 0.80 |
+--------------+------------+----------------+----------------+------+
2 rows
```

| Column | Meaning |
|---|---|
| `id` | `RelationId` (UUIDv7 dashed). |
| `type` | Relation-type qname. |
| `from` | Source `EntityId`. |
| `to` | Target `EntityId`. |
| `conf` | Confidence in `[0.0, 1.0]`, two-decimal. |

Empty result → `(no rows)`.

#### JSON

```json
[
  { "id": "0190f9a0-…",
    "type": "works_at",
    "from": "0190f8c0-…",
    "to": "0190f8e1-…",
    "conf": "0.95" }
]
```

All cells are strings (shared `AdHocTable` shape).

### Examples

```bash
# What does Priya relate to?
brain relation list --from 0190f8c0-1c1d-7c41-9b88-0acdb5e6f0e1

# Who works_at Acme?
brain relation list --to 0190f8e1-… --type works_at

# Cross-filter: relations between Priya and Acme
brain relation list --from 0190f8c0-… --to 0190f8e1-… -o json | jq '.[].type'

# Chain into entity show on the target
TGT=$(brain relation list --from 0190f8c0-… -o json | jq -r '.[0].to')
brain entity show "$TGT"
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `InvalidArgument` | Bad UUID for `--from` / `--to`. | Re-issue with a parseable id. |
| `Internal: requires --from or --to` | Both omitted. | Add at least one anchor. |
| `Internal: bad entity id` | Argument wasn't a UUID. | Look it up via `entity list` or `entity show`. |
| `ShardUnavailable` | Target shard down. | Wait + retry. |

Full catalogue: [`../errors.md`](../errors.md).

---

## Gated features

| Surface | Blocked on |
|---|---|
| Server-side `--from + --to` cross-filter | A `RelationListBetweenReq` wire op that walks the smaller side of the bipartite index. Falls back to client-side filtering today. |
| Friendly entity-name columns | Batched `EntityGet` follow-up keyed by `from` / `to`, or a server-side denormalised `from_name` / `to_name`. |
| Symmetric-relation collapse | Per-type `Symmetric` annotation surfaced through the SDK so the renderer can pick one canonical direction (see [`spec/02_data_model/02_symmetric.md`](../../../../spec/02_data_model/02_symmetric.md)). |

Each emits a `tracing` warning at runtime where applicable.

---

## See also

- [`entity.md`](entity.md) — the endpoints of a relation
- [`entity.md#brain-entity-neighbors`](entity.md#brain-entity-neighbors) — tree-shaped traversal that consumes the same relations index
- [`statement.md`](statement.md) — Fact/Preference/Event, the per-subject analogue
- [`mention.md`](mention.md) — memory ↔ entity provenance
- [`../output-formats.md`](../output-formats.md) — table + JSON
- Spec: [`spec/02_data_model/`](../../../../spec/02_data_model/00_purpose.md)
