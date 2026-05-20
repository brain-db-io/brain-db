# `brain statement`

Browse statements — the typed triples (Fact / Preference / Event) that
the extractor pipeline writes when memories carry assertions about
entities. The verb covers `list` and `show`.

**Knowledge layer prerequisite.** Statements exist only when a schema
is declared via `SCHEMA_UPLOAD`. On substrate-only deployments these
subcommands return empty rows or `not found`. The three statement
kinds live in [`spec/17_knowledge_model/02_three_statement_kinds.md`](../../../../spec/17_knowledge_model/02_three_statement_kinds.md);
their storage shape is in [`spec/19_statements/03_storage.md`](../../../../spec/19_statements/03_storage.md).

---

## `brain statement list`

Filtered table of statements. At least one filter is recommended —
unfiltered lists return up to `--limit` of the most recent statements
in the shard's statement table.

```
brain statement list
        [--subject <UUID>]
        [--predicate <QNAME>]
        [--object <UUID>]
        [--limit <N>]
```

### Flags

#### `--subject <UUID>`

Filter by subject entity (the left-hand side of the triple). Accepts a
dashed UUID. Passes through to `StatementList.where_subject`.

```bash
brain statement list --subject 0190f8c0-1c1d-7c41-9b88-0acdb5e6f0e1
```

#### `--predicate <QNAME>`

Filter by predicate qname (`works_at`, `prefers`, …). Passes through
to `StatementList.where_predicate`.

```bash
brain statement list --predicate works_at --limit 100
```

#### `--object <UUID>`

**Currently ignored.** The SDK list-builder doesn't expose an object
filter today — the shell emits a `tracing` warning, then runs the
remaining filters without it. See [Gated features](#gated-features).

#### `--limit <N>`

Maximum rows. Default `50`.

### Output

#### Table

```
+--------------+--------------+----------------+------------+--------------------+------+
| id           | kind         | subject        | predicate  | object             | conf |
+--------------+--------------+----------------+------------+--------------------+------+
| 0190f8d0-…   | Fact         | 0190f8c0-…     | works_at   | entity:0190f8e1-…  | 0.95 |
| 0190f8d1-…   | Preference   | 0190f8c0-…     | prefers    | value:"tea"        | 0.78 |
| 0190f8d2-…   | Event        | 0190f8c0-…     | attended   | memory:0x000200…   | 0.62 |
+--------------+--------------+----------------+------------+--------------------+------+
3 rows
```

| Column | Meaning |
|---|---|
| `id` | `StatementId` (UUIDv7 dashed). |
| `kind` | `Fact`, `Preference`, or `Event`. Rendered via Rust `Debug`. |
| `subject` | Subject `EntityId`, or `pending(<audit_uuid>)` if the resolver hasn't merged yet. |
| `predicate` | Predicate qname. |
| `object` | One of `entity:<uuid>` · `value:<repr>` · `memory:0x<32-hex>` · `statement:<uuid>`. |
| `conf` | Confidence in `[0.0, 1.0]`, two-decimal. |

Empty result → `(no rows)`.

#### JSON

```json
[
  { "id": "0190f8d0-…",
    "kind": "Fact",
    "subject": "0190f8c0-…",
    "predicate": "works_at",
    "object": "entity:0190f8e1-…",
    "conf": "0.95" }
]
```

All cells are strings (shared `AdHocTable` shape). Parse `conf` as
float in `jq` if you need numerics.

### Examples

```bash
# All statements whose subject is Priya
brain statement list --subject 0190f8c0-1c1d-7c41-9b88-0acdb5e6f0e1

# Every `works_at` Fact in the agent's shard
brain statement list --predicate works_at -o json \
  | jq '.[] | select(.kind == "Fact") | {sub: .subject, obj: .object}'

# Drill from a statement row to its full card
ID=$(brain statement list --predicate works_at -o json | jq -r '.[0].id')
brain statement show "$ID"
```

---

## `brain statement show`

Single-statement card. Header line carries the human-readable triple;
the body shows the id, confidence, and any evidence memories.

```
brain statement show <ID>
```

### Arguments

`<ID>` — `StatementId` (UUID; dashed or undashed). Names aren't
accepted — find the id via `statement list` first.

### Output

#### Card

```
[Fact] Priya works_at Acme Corp
  id = 0190f8d0-1c1d-7c41-9b88-0acdb5e6f0e1   conf = 0.95

Evidence
  · s2/m17/v1   Priya works at Acme Corp as a staff engineer
```

| Field | Source | Status |
|---|---|---|
| Header `[Kind] Subject Predicate Object` | `StatementHandle` | wired |
| `id`, `conf` | `StatementHandle` | wired |
| `subject_canonical` | Currently the subject **UUID string** — canonical-name lookup not wired yet. See [Gated features](#gated-features). |
| `object` (entity name) | Entity object renders as the entity **UUID** for the same reason. |
| Evidence | `evidence-list-by-statement` wire op | gated — section empty today |

Object types map to renderer shapes:

| `StatementObject` variant | Renderer shape |
|---|---|
| `Entity(id)` | `ObjectRef::Entity { id, name }` — name = id today. |
| `Value(v)` | `ObjectRef::Literal("<Debug>")` — `Value` is rendered via Rust `Debug`. |
| `Memory(id)` | `ObjectRef::Literal("memory 0x<32-hex>")`. |
| `Statement(id)` | `ObjectRef::Literal("statement <uuid>")` — supports nested-statement subjects. |

#### JSON

```json
{ "id": "0190f8d0-…",
  "statement_kind": "Fact",
  "subject_canonical": "0190f8c0-…",
  "predicate": "works_at",
  "object": { "kind": "entity", "id": "0190f8e1-…", "name": "0190f8e1-…" },
  "object_label": "0190f8e1-…",
  "confidence": 0.95,
  "evidence_memories": [] }
```

`object.kind` distinguishes `"entity"` vs `"literal"`; `object_label`
is the bare right-hand-side string for tools that only want a label.

### Examples

```bash
# Just the predicate + confidence
brain statement show "$ID" -o "jsonpath={.predicate}"

# Resolve subject to a name via a follow-up entity show
SUB=$(brain statement show "$ID" -o json | jq -r .subject_canonical)
brain entity show "$SUB"
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `InvalidArgument` | Bad UUID for `--subject` or `show <ID>`. | Re-issue with a parseable id. |
| `Internal: statement not found` | `show` returned `None`. | Confirm the id with `statement list`. |
| `Internal: bad subject id` | `--subject` not a valid UUID. | Use the UUID form, not the canonical name. |
| `ShardUnavailable` | Target shard down. | Wait + retry. |

Full catalogue: [`../errors.md`](../errors.md).

---

## Gated features

| Surface | Blocked on |
|---|---|
| `statement list --object <UUID>` | Object filter on the SDK `StatementList` builder, mapped to the existing server-side index. |
| `statement show` subject / entity-object **names** | Entity canonical-name lookup as part of the show response (or a client-side batched `EntityGet`). |
| `statement show` `Evidence` section | `evidence-list-by-statement` wire op + `StatementHandle.evidence_memories` SDK field. |
| Pending-subject rendering | Once the resolver merges the audit row, the subject column should auto-refresh — needs the audit-watch hook. |

Each emits a `tracing` warning at runtime; nothing panics, output is
just thinner than the final card.

---

## See also

- [`entity.md`](entity.md) — subjects + entity objects
- [`relation.md`](relation.md) — typed edges (the relation analogue of facts)
- [`mention.md`](mention.md) — evidence memories
- [`extract.md`](extract.md) — the pipeline that wrote these statements
- [`../output-formats.md`](../output-formats.md) — table + JSON
- Spec: [`spec/17_knowledge_model/02_three_statement_kinds.md`](../../../../spec/17_knowledge_model/02_three_statement_kinds.md), [`spec/19_statements/`](../../../../spec/19_statements/)
