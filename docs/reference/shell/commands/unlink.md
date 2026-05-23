# `brain unlink`

Remove a typed edge from one memory to another. UNLINK is
**idempotent**: removing a non-existent edge returns
`removed=false` and exits `0` — callers can issue UNLINK blindly
without a precondition check.

```
brain unlink <SRC> <KIND> <TGT>
        [--txn <HEX>]
```

Inherits the session's **active txn** (`txn begin`) unless `--txn`
overrides. Inside a txn the edge stays visible to other readers
until `txn commit`.

---

## Positional arguments

| Position | Meaning |
|---|---|
| `<SRC>` | Source memory id — any of the three [`MemoryId` input forms](../output-formats.md#memory-ids). |
| `<KIND>` | Edge kind. Same enum as [`link.md`](link.md#edge-kinds). |
| `<TGT>` | Target memory id — same input forms as `<SRC>`. |

The `(SRC, KIND, TGT)` triple must match exactly. Removing a
`caused` edge does not affect a parallel `supports` edge between the
same pair.

```bash
brain unlink s2/m1/v1 caused s2/m2/v1
```

---

## Flags

### `--txn <HEX>`

Attach to a server-side transaction. 32-hex-char id (`0x` prefix
optional). In the REPL, an active `txn begin` auto-attaches — see
[`txn.md`](txn.md).

---

## Output

### Table

```
ok  s2/m1/v1 --[Caused]--> s2/m2/v1  removed=true
```

| Field | Meaning |
|---|---|
| `s2/m1/v1` | Short source `MemoryId`. |
| `--[Caused]-->` | Edge kind in CamelCase. |
| `s2/m2/v1` | Short target `MemoryId`. |
| `removed=true` | The edge existed and was removed. |
| `removed=false` | No edge with this `(src, kind, tgt)` triple. Treated as success — UNLINK is idempotent. |

### JSON

```json
{ "op": "unlink",
  "result": {
    "source": "0x00020000000000010000000100000000",
    "target": "0x00020000000000020000000100000000",
    "kind": "Caused",
    "removed": true
  } }
```

| Field | Notes |
|---|---|
| `source` / `target` | Canonical 32-hex `MemoryId`s. |
| `kind` | CamelCase wire enum name. |
| `removed` | `true` if an edge was deleted; `false` if it didn't exist. Both outcomes exit `0`. |

See [`../output-formats.md`](../output-formats.md) for `wide` / `yaml`
/ `jsonpath=` variants.

---

## Examples

```bash
# Plain removal
brain unlink s2/m1/v1 caused s2/m2/v1

# Idempotent — second call exits 0, removed=false
brain unlink s2/m1/v1 caused s2/m2/v1

# Was this a no-op? Pluck the flag with jq
brain unlink s2/m1/v1 supports s2/m3/v1 -o json \
  | jq -r '.result.removed'

# Drop every outgoing edge of a known kind (recall outgoing edges via
# the renderer, then unlink each)
brain recall "auth" --top-k 1 -o json \
  | jq -r '.result[0].memory_id' \
  | xargs -I{} brain unlink {} similar-to s2/m17/v1

# Atomic re-wire: drop the old edge and add a new one inside one txn
brain> txn begin
brain*> unlink s2/m1/v1 supports s2/m3/v1
brain*> link s2/m1/v1 contradicts s2/m3/v1
brain*> txn commit <id>
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `InvalidArgument` | Unknown edge kind. Malformed memory id. | Re-issue with valid input. |
| `NotFound` | Source or target memory id doesn't exist (the **memory**, not the edge — missing edges are silent). | Verify the id with `recall`. |
| `TxnNotFound` | `--txn` (or session txn) references a txn the server doesn't know. | Reopen with `txn begin`; the shell auto-clears the stale session txn. |
| `TransactionTimeout` | The bound txn hit its server-side timeout before UNLINK landed. | Reopen with `txn begin`; the shell auto-clears the stale session txn. |
| `ShardUnavailable` | Source shard down. | Wait + retry; check `brain info`. |
| `Overloaded` | WAL group-commit queue full. | Back off. |

Full catalogue: [`../errors.md`](../errors.md).

---

## See also

- [`link.md`](link.md) — the inverse
- [`forget.md`](forget.md) — `forget --mode hard` removes a memory and **all** its edges in one operation
- [`txn.md`](txn.md) — group multiple UNLINKs into one atomic apply
- [`../output-formats.md`](../output-formats.md) — table + JSON + ndjson + yaml + jsonpath
- [`../errors.md`](../errors.md) — error codes
- Spec: [`spec/05_operations/02_write_pipeline.md`](../../../../spec/05_operations/02_write_pipeline.md)
