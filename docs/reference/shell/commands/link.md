# `brain link`

Create a typed, weighted edge from one memory to another. LINK is
**idempotent on the (source, kind, target) triple** — re-linking with
a new weight overwrites the old one and surfaces
`already_existed=true` so callers can distinguish "fresh edge" from
"weight update".

```
brain link <SRC> <KIND> <TGT>
        [--weight <FLOAT>]
        [--txn <HEX>]
```

Inherits the session's **active txn** (`txn begin`) unless `--txn`
overrides. When an edge is added inside a txn, the new weight only
becomes visible to other readers after `txn commit`.

---

## Positional arguments

| Position | Meaning |
|---|---|
| `<SRC>` | Source memory id — any of the three [`MemoryId` input forms](../output-formats.md#memory-ids). |
| `<KIND>` | Edge kind (see [Edge kinds](#edge-kinds)). |
| `<TGT>` | Target memory id — same input forms as `<SRC>`. |

```bash
brain link s2/m1/v1 caused s2/m2/v1
```

### Edge kinds

`clap` accepts kebab-case (`followed-by`) at the CLI; the wire enum
uses CamelCase. Both forms render as CamelCase in output.

| Flag value | Wire enum | Typical use |
|---|---|---|
| `caused` | `Caused` | A directly produced B. |
| `followed-by` | `FollowedBy` | Temporal sequence; A then B. |
| `derived-from` | `DerivedFrom` | B is a refinement / summary of A. |
| `similar-to` | `SimilarTo` | Semantic neighborhood; symmetric-ish. |
| `contradicts` | `Contradicts` | A and B can't both be true. |
| `supports` | `Supports` | A is evidence for B. |
| `references` | `References` | A textually cites B. |
| `part-of` | `PartOf` | A is a sub-component of B. |

Edge direction is meaningful for every kind except `similar-to` —
the planner and graph workers read it asymmetrically.

---

## Flags

### `--weight <FLOAT>`

Edge weight. Defaults to `1.0`. Clamped server-side to `[0.0, 1.0]`.

Use sub-`1.0` weights for soft / probabilistic edges that the
planner should de-prioritise. The decay worker does **not** touch
edge weights — they are write-time hints and stay until rewritten or
removed.

```bash
# Strong causal link
brain link s2/m1/v1 caused s2/m2/v1 --weight 1.0

# Weak supporting evidence
brain link s2/m1/v1 supports s2/m3/v1 --weight 0.3
```

### `--txn <HEX>`

Attach to a server-side transaction. 32-hex-char id (`0x` prefix
optional). In the REPL, an active `txn begin` auto-attaches without
needing this flag — see [`txn.md`](txn.md).

---

## Output

### Table

```
ok  s2/m1/v1 --[Caused]--> s2/m2/v1  weight=0.7000  already_existed=false
```

| Field | Meaning |
|---|---|
| `s2/m1/v1` | Short source `MemoryId`. |
| `--[Caused]-->` | Edge kind in CamelCase, with arrow indicating direction. |
| `s2/m2/v1` | Short target `MemoryId`. |
| `weight=0.7000` | Final stored weight (post-clamp). Four decimal places, always. |
| `already_existed=…` | `false` for a fresh edge; `true` if LINK overwrote an existing weight. |

### JSON

```json
{ "op": "link",
  "result": {
    "source": "0x00020000000000010000000100000000",
    "target": "0x00020000000000020000000100000000",
    "kind": "Caused",
    "weight": 0.7,
    "created_at_unix_nanos": 1779153941479431250,
    "already_existed": false
  } }
```

| Field | Notes |
|---|---|
| `source` / `target` | Canonical 32-hex `MemoryId`s. |
| `kind` | CamelCase wire enum name. |
| `weight` | Server-clamped final value. |
| `created_at_unix_nanos` | Wall-clock at edge insertion. For an overwrite, the **original** create time is preserved — only the weight moved. |
| `already_existed` | Use this to distinguish "create" vs "update" without an extra round-trip. |

See [`../output-formats.md`](../output-formats.md) for `wide` / `yaml`
/ `jsonpath=` variants.

---

## Examples

```bash
# Inline after a recall — chain the top hit to a known anchor
ID=$(brain recall "deploy" --top-k 1 -o json | jq -r '.result[0].memory_id')
brain link "$ID" derived-from s2/m1/v1

# Re-weight an existing edge (the response carries already_existed=true)
brain link s2/m1/v1 supports s2/m3/v1 --weight 0.8 -o json \
  | jq '{updated: .result.already_existed, weight: .result.weight}'

# Multi-edge atomic insert via a transaction
brain> txn begin
brain*> link s2/m1/v1 caused s2/m2/v1
brain*> link s2/m2/v1 followed-by s2/m3/v1
brain*> link s2/m3/v1 supports s2/m1/v1
brain*> txn commit <id>

# Bulk import from a TSV (src \t kind \t tgt \t weight)
while IFS=$'\t' read -r s k t w; do
  brain link "$s" "$k" "$t" --weight "$w"
done < edges.tsv
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `InvalidArgument` | `--weight` outside `[0.0, 1.0]`. Unknown edge kind. | Re-issue with valid input. |
| `NotFound` | Source or target memory id doesn't exist (or has been hard-forgotten and reclaimed). | Verify the id with `recall` or `forget` first. |
| `TxnNotFound` | `--txn` (or session txn) references a txn the server doesn't know. | Reopen with `txn begin`; the shell auto-clears the stale session txn. |
| `TransactionTimeout` | The bound txn hit its server-side timeout before LINK landed. | Reopen with `txn begin`; the shell auto-clears the stale session txn. |
| `ShardUnavailable` | Source shard down. | Wait + retry; check `brain info`. |
| `Overloaded` | WAL group-commit queue full. | Back off. |

Full catalogue: [`../errors.md`](../errors.md).

---

## See also

- [`unlink.md`](unlink.md) — the inverse
- [`encode.md`](encode.md) — `--edge` attaches edges at create time
- [`txn.md`](txn.md) — group multiple LINKs into one atomic apply
- [`recall.md`](recall.md) — produces ids to link against
- [`../output-formats.md`](../output-formats.md) — table + JSON + ndjson + yaml + jsonpath
- [`../errors.md`](../errors.md) — error codes
- Spec: [`spec/05_operations/02_write_pipeline.md`](../../../../spec/05_operations/02_write_pipeline.md)
