# `brain mention`

Browse Mentions edges — the provenance link that says "this memory
references this entity". A Mentions edge is written by the extractor
pipeline every time it finds a span of a memory that resolves to an
entity. The verb covers `list`.

**Knowledge layer prerequisite.** Mentions edges exist only when a
schema is declared via `SCHEMA_UPLOAD` and the extractor pipeline has
run. On substrate-only deployments these subcommands return empty
rows.

**Status.** The shell parses the command surface today; the wire op
isn't exposed yet, so every invocation returns an empty table after a
`tracing` warning. The shape below is the **target** — scripts can be
written against it now. See [Gated features](#gated-features).

---

## `brain mention list`

Filtered table of Mentions edges. Exactly one anchor must be supplied
— `clap`'s argument-group enforcement makes `--memory` and `--entity`
mutually exclusive at parse time.

```
brain mention list
        (--memory <MEMORY_ID> | --entity <ENTITY_UUID>)
        [--limit <N>]
```

### Flags

#### `--memory <MEMORY_ID>`

"Which entities does this memory mention?" Accepts any of the three
[`MemoryId` input forms](../output-formats.md#memory-ids): short
(`s2/m17/v1`), long hex (`0x…`), decimal `u128`.

```bash
brain mention list --memory s2/m17/v1
```

#### `--entity <ENTITY_UUID>`

"Which memories mention this entity?" Accepts an `EntityId` UUID,
dashed or undashed.

```bash
brain mention list --entity 0190f8c0-1c1d-7c41-9b88-0acdb5e6f0e1
```

#### `--limit <N>`

Maximum rows. Default `50`.

### Output

#### Table

```
+-------------+--------------------------------------+----------+---------------------+
| memory      | entity                               | kind     | created_at          |
+-------------+--------------------------------------+----------+---------------------+
| s2/m17/v1   | 0190f8c0-1c1d-7c41-9b88-0acdb5e6f0e1 | Mentions | 1779153941479431250 |
| s2/m17/v1   | 0190f8e1-1c1d-7c42-9b88-12cb8e2a4d0c | Mentions | 1779153941479431250 |
+-------------+--------------------------------------+----------+---------------------+
2 rows
```

| Column | Meaning |
|---|---|
| `memory` | Short `MemoryId` (the originating memory). |
| `entity` | `EntityId` UUID of the entity mentioned. |
| `kind` | Edge kind — always `Mentions` for this command. The column exists for forward compatibility with future provenance edge types (e.g. `MentionedBy`, `EvidenceFor`). |
| `created_at` | Unix-nanos timestamp when the extractor wrote the edge. |

Empty result → `(no rows)`. **Always empty today** until the wire op
ships.

#### JSON

```json
[
  { "memory": "s2/m17/v1",
    "entity": "0190f8c0-…",
    "kind": "Mentions",
    "created_at": "1779153941479431250" }
]
```

All cells are strings (shared `AdHocTable` shape).

### Examples

```bash
# Provenance for a single memory (target shape)
brain mention list --memory s2/m17/v1

# All memories about a person, with full text
brain mention list --entity 0190f8c0-… -o json \
  | jq -r '.[].memory' \
  | while read ID; do brain recall "" --include-text | grep "$ID"; done

# JSON drill: count of distinct memories mentioning an entity
brain mention list --entity 0190f8c0-… -o json | jq 'map(.memory) | unique | length'
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `InvalidArgument` | Both `--memory` and `--entity` passed. | Clap rejects this at parse time. Drop one. |
| `Internal: requires --memory or --entity` | Both omitted. | Pass exactly one anchor. |
| `InvalidArgument` | Bad `MemoryId` or `EntityId` form. | Re-issue with a parseable id. |
| `ShardUnavailable` | Target shard down. | Wait + retry. |

Full catalogue: [`../errors.md`](../errors.md).

---

## Gated features

The shell surface is complete; the data path is not. Every invocation
emits the same warning today:

```
WARN brain_shell: mention list: wire op MentionListReq / EdgeList(Mentions) not yet
                  exposed via the SDK. Returning an empty table; a follow-up needs
                  to add the wire op + SDK builder.
```

| Surface | Blocked on |
|---|---|
| `mention list --memory <ID>` | Wire op `MentionListReq` (or generalised `EdgeList(Mentions)`) + SDK list-builder. The underlying edge data is already in the per-shard unified edge index. |
| `mention list --entity <UUID>` | Same wire op, queried on the inbound side. |
| Span / offset columns | Once the extractor records the `(byte_start, byte_end)` span of the mention in the source memory, surface it as a `wide`-only column. |

---

## See also

- [`entity.md`](entity.md) — destination of every Mentions edge
- [`statement.md`](statement.md) — statements derived from the mentioned memory
- [`extract.md`](extract.md) — the pipeline that writes Mentions edges
- [`recall.md`](recall.md) — fetch the memory body referenced by a mention row
- [`../output-formats.md`](../output-formats.md) — table + JSON
- Spec: [`spec/02_data_model/02_storage.md`](../../../../spec/02_data_model/02_storage.md), [`spec/11_extractors/00_purpose.md`](../../../../spec/11_extractors/00_purpose.md)
