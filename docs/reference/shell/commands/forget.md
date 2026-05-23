# `brain forget`

Tombstone a memory. Idempotent; safe to retry.

```
brain forget <ID> [--mode soft|hard]
```

Inherits the session's active txn automatically — `forget` inside a
`txn begin` is part of the same WAL bracket as the surrounding
`encode`/`link` calls.

---

## Positional argument

`<ID>` accepts any of the three [`MemoryId` input forms](../output-formats.md#memory-ids):

| Form | Example |
|---|---|
| Short | `s2/m1/v1` |
| Long hex | `0x00020000000000010000000100000000` |
| Decimal `u128` | `42` |

Paste any id you see in `recall` table output, JSON output, or another
script — all three work.

---

## `--mode soft|hard`

Default `soft`.

### `--mode soft` (default)

Marks the memory `HARD_FORGOTTEN`, evicts the FINGERPRINTS entry (so
subsequent dedup-on `encode`s of the same text get a `miss`), and emits
a `Forgotten` event on the change feed.

The arena slot is reclaimed by the background worker after the
tombstone grace period (default 7 days — see
`spec/19_benchmarks/01_correctness_and_durability.md`).

A subsequent `recall` will not surface the row; a direct `link` /
`unlink` targeting it returns `NotFound`.

### `--mode hard`

Everything a soft forget does, **plus** zeroes the vector in the arena
immediately. Use when the memory contains sensitive content you can't
let sit in the WAL/arena for the grace period.

The WAL record itself is preserved (its presence is the audit trail);
only the in-place arena slot is zeroed.

---

## Output

### Table

```
ok  s2/m1/v1  outcome=Tombstoned  edges_removed=0
```

| `outcome=` | Meaning |
|---|---|
| `Tombstoned` | Memory was Active; tombstoned. |
| `AlreadyTombstoned` | Idempotent no-op — `forget` had already been called for this id. |
| `MemoryNotFound` | No such memory. **Returns success**, per spec §02/06 §10 (idempotent semantics). |

`edges_removed=N` reports edges removed in the same write transaction
(both in-edges and out-edges).

### JSON

```json
{ "op": "forget",
  "result": {
    "memory_id": "0x00020000000000010000000100000000",
    "outcome": "Tombstoned",
    "was_already_forgotten": false,
    "edges_removed": 0
  } }
```

---

## Examples

```bash
# Soft forget (default)
brain forget s2/m1/v1

# Hard forget for PII
brain forget 0x00020000000000010000000100000000 --mode hard

# Pipe recall ids into forget
brain recall "test fixture" --top-k 50 -o json \
  | jq -r '.result[].memory_id' \
  | xargs -n1 brain forget --mode soft

# Inside a txn — the forget is committed atomically with surrounding ops
brain> txn begin
brain*> forget s2/m1/v1
brain*> encode "replacement memory" --context 4
brain*> txn commit <id>
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `InvalidArgument` | Malformed id. | Re-paste the id; check it parses (`s<shard>/m<slot>/v<version>` for short form). |
| `Conflict` | (Should not fire — `forget` is idempotent server-side.) | File a bug. |

`MemoryNotFound` is **not** an error — it's surfaced in `outcome=`.

Full catalogue: [`../errors.md`](../errors.md).

---

## See also

- [`encode.md`](encode.md) — the write side
- [`link.md`](link.md) / [`unlink.md`](unlink.md) — edges are auto-removed on forget
- [`subscribe.md`](subscribe.md) — observe `Forgotten` events
- Spec: [`spec/05_operations/02_write_pipeline.md`](../../../../spec/05_operations/02_write_pipeline.md)
