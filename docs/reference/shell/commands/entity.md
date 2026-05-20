# `brain entity`

Browse the knowledge-layer entity table — typed nouns (Person, Org,
…) that the extractor pipeline distilled from your memories. The verb
covers `list`, `show`, and `neighbors`.

**Knowledge layer prerequisite.** Entities materialise only when the
deployment has a schema declared via `SCHEMA_UPLOAD`. On a
substrate-only deployment these subcommands either return empty
results or report `not found`; the substrate's vector store doesn't
carry typed entities. See [`spec/17_knowledge_model/00_purpose.md`](../../../../spec/17_knowledge_model/00_purpose.md).

---

## `brain entity list`

Paginated table of entities, optionally filtered by type or name
prefix. Backed by the wire `EntityListReq` op.

```
brain entity list
        [--type <QNAME>]
        [--limit <N>]
        [--prefix <STR>]
```

### Flags

#### `--type <QNAME>`

Restrict to one entity-type qualified name. Defaults to `Person` when
omitted.

Only `Person` is wired end-to-end today. Arbitrary qnames (`Org`,
custom schema types) need a `SCHEMA_GET`-style op to resolve qname →
`entity_type_id`; the shell panics with a `todo!` when asked for any
other type. See [Gated features](#gated-features).

```bash
brain entity list --type Person --limit 20
```

#### `--limit <N>`

Maximum rows to return. Default `50`. The server may clamp larger
values per [spec §18/02](../../../../spec/18_entities/02_storage.md).

#### `--prefix <STR>`

Server-side canonical-name prefix filter. Useful for autocompletion or
disambiguating large directories:

```bash
brain entity list --type Person --prefix "Pri"
```

### Output

#### Table

```
+--------------------------------------+--------+--------+----------+
| id                                   | name   | type   | mentions |
+--------------------------------------+--------+--------+----------+
| 0190f8c0-…-7c41-9b88-0acdb5e6f0e1    | Priya  | Person | 17       |
| 0190f8c1-…-7c42-9b88-12cb8e2a4d0c    | Bob    | Person | 4        |
+--------------------------------------+--------+--------+----------+
2 rows
```

| Column | Meaning |
|---|---|
| `id` | `EntityId` (UUIDv7 dashed form). |
| `name` | Canonical name — winner of the resolution merge. |
| `type` | The qname filter that produced this row. |
| `mentions` | Count of Mentions edges into this entity (hotness signal). |

Empty result → `(no rows)`.

#### JSON

```json
[
  { "id": "0190f8c0-1c1d-7c41-9b88-0acdb5e6f0e1",
    "name": "Priya",
    "type": "Person",
    "mentions": "17" }
]
```

`AdHocTable` always serialises cells as strings — pre-stringified at
the shell layer because the table renderer is shared with `statement
list`, `relation list`, `mention list`. Coerce in `jq` if you need
numerics.

### Examples

```bash
# Default Person directory
brain entity list

# JSON for a script
brain entity list --type Person --limit 200 -o json | jq '.[].name'

# Prefix-narrowed lookup
brain entity list --prefix "Acme"
```

---

## `brain entity show`

Stacked-card view of a single entity — identity, aliases, statements
it appears in, memories that mention it, outbound and inbound
relations.

```
brain entity show <ID_OR_NAME>
```

### Arguments

`<ID_OR_NAME>` — either a `EntityId` UUID (dashed or not) **or** a
canonical name. UUIDs go straight to `EntityResolveReq.get`; names
go through `EntityResolveReq.resolve`, which can return `NotFound`
or `Ambiguous`. Ambiguity surfacing isn't wired yet — multiple
matches collapse to "not resolved". See [Gated features](#gated-features).

### Output

#### Stacked card

```
Priya  (Person)
  id = 0190f8c0-1c1d-7c41-9b88-0acdb5e6f0e1

Aliases
  · P.
  · Priya R.

Statements
  · [Fact] works_at Acme Corp (conf 0.95)  id=stmt_01a2…
  · [Preference] prefers tea (conf 0.78)  id=stmt_01a3…

Mentioned in
  · s2/m17/v1   Priya works at Acme Corp as a staff engineer

Relations (out)
  · --[works_at]--> Acme Corp

Relations (in)
  · Bob --[knows]--> ·
```

Sections render only when they have rows. Memory ids and entity ids
carry OSC 8 hyperlinks (`brain://recall/…`, `brain://entity/…`) for
iTerm / kitty / WezTerm.

| Section | Source | Status |
|---|---|---|
| Identity (`id`, name, type) | `EntityResolveReq` / `EntityGet` | wired (Person) |
| Aliases | `EntityHandle.aliases` | wired |
| Statements | `statement-list-by-subject` wire op | gated — section empty today |
| Mentioned in | `mention-list-by-entity` wire op | gated — section empty today |
| Relations (out / in) | `relation-list-by-entity` wire op | gated — section empty today |

#### JSON

```json
{ "id": "0190f8c0-…",
  "canonical_name": "Priya",
  "type": "Person",
  "aliases": ["P.", "Priya R."],
  "statements": [],
  "mentioned_in": [],
  "relations_out": [],
  "relations_in": [] }
```

The JSON shape is fixed — empty arrays today, populated once the
follow-up wire ops land. Scripts can write against the final shape
now.

### Examples

```bash
# Lookup by name
brain entity show "Priya"

# Lookup by UUID
brain entity show 0190f8c0-1c1d-7c41-9b88-0acdb5e6f0e1

# Pluck the canonical name
brain entity show "Priya" -o json | jq -r .canonical_name
```

---

## `brain entity neighbors`

Tree-shaped neighborhood rooted at one entity, walking outbound
relations to a fixed depth.

```
brain entity neighbors <ID> [--depth <N>]
```

### Arguments / flags

| Arg / flag | Meaning |
|---|---|
| `<ID>` | Root `EntityId` (UUID). Names not accepted here — disambiguate via `entity show` first. |
| `--depth <N>` | Traversal depth. Default `2`. |

### Output

#### Tree

```
0190f8c0-1c1d-7c41-9b88-0acdb5e6f0e1  (depth 2)
└── (neighborhood traversal not yet wired)
```

Once the wire op lands the tree carries real labels:

```
Priya (Person)
├── Acme Corp (Org) [works_at]
│   └── Bob (Person) [employs]
└── Bob (Person) [knows]
```

`GraphTree` wraps `termtree` — every node is a `GraphNode { label,
children }`. The renderer is depth-agnostic; depth bounding happens
client-side when the traversal response is converted.

#### JSON

```json
{ "label": "0190f8c0-… (depth 2)",
  "children": [
    { "label": "(neighborhood traversal not yet wired)", "children": [] }
  ] }
```

### Status

Placeholder today. The wire op `RelationTraverseReq` exists in
`brain-protocol` but isn't bound to the typed SDK builder, so the
shell emits a warning and renders the stub tree. See [Gated
features](#gated-features).

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `InvalidArgument` | Bad UUID for `show` / `neighbors`. | Re-issue with a parseable id. |
| `Internal: entity not found` | `show` by id returned `None`. | Confirm with `entity list` that the entity still exists. |
| `Internal: entity not resolved` | `show` by name hit `NotFound` or `Ambiguous`. | Pass the UUID, or narrow the name. |
| `ShardUnavailable` | Target shard down. | Wait + retry; check `brain info`. |

Full catalogue: [`../errors.md`](../errors.md).

---

## Gated features

The shell parses these surfaces but the wire / SDK plumbing isn't
complete. Each emits a `tracing` warning at runtime so the operator
sees the gap; no state is corrupted.

| Surface | Blocked on |
|---|---|
| `entity list --type <NotPerson>` | A `SCHEMA_GET`-style wire op for qname → `entity_type_id` resolution. |
| `entity show` statements / mentions / relations sections | Wire ops `statement-list-by-subject`, `mention-list-by-entity`, `relation-list-by-entity` exposed via the SDK. |
| `entity show` by ambiguous name | `EntityResolveResp` carrying the candidate list back to the client. |
| `entity neighbors` traversal | Binding `RelationTraverseReq` through to the typed SDK builder. |

---

## See also

- [`statement.md`](statement.md) — what entities appear in
- [`relation.md`](relation.md) — typed edges between entities
- [`mention.md`](mention.md) — memory ↔ entity provenance
- [`recall.md`](recall.md) — `--include-graph` for hybrid enrichment
- [`../output-formats.md`](../output-formats.md) — table + JSON
- Spec: [`spec/17_knowledge_model/`](../../../../spec/17_knowledge_model/), [`spec/18_entities/`](../../../../spec/18_entities/)
