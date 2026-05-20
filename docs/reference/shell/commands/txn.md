# `brain txn`

Multi-op atomicity. A transaction buffers `encode` / `forget` /
`link` / `unlink` operations on the server; on commit they apply as
**one redb write transaction inside one WAL bracket** — all-or-nothing
durability, no partial visibility.

```
brain txn begin
brain txn commit <ID>
brain txn abort  <ID>
```

`<ID>` is a 32-hex-character transaction id. The `0x` prefix is
optional — `0xdead…beef` and `dead…beef` parse identically.

---

## Subcommands

### `brain txn begin`

Open a new transaction. The server replies with a fresh 16-byte id,
the server-side timeout (default 30s — abort happens automatically if
no commit by then), and the wall-clock start time.

```bash
brain txn begin
# ok  txn_id=0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5  timeout_seconds=30
```

**In the REPL**, the returned id is stored on the session as the
"sticky" active txn. Subsequent `encode` / `forget` / `link` /
`unlink` auto-attach without needing `--txn`. The prompt switches
from `brain> ` to `brain*> ` so you can see at a glance you're inside
one.

**In one-shot mode**, the id is printed and that's it. The caller
must thread it through subsequent invocations with `--txn <ID>`.

### `brain txn commit <ID>`

Apply every buffered op atomically. Either every op lands (one WAL
bracket, one redb write) or none do. The response carries
`operations_applied=N` so you can sanity-check that the server saw
the same op count you intended.

In the REPL, a matching commit clears the session's sticky txn and
the prompt returns to `brain> `.

```bash
brain txn commit 0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5
# ok  txn_id=0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5  operations_applied=3
```

### `brain txn abort <ID>`

Discard every buffered op. No WAL bracket is emitted. The response
carries `operations_discarded=N` for observability.

In the REPL, a matching abort clears the session's sticky txn.

```bash
brain txn abort 0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5
# ok  txn_id=0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5  operations_discarded=3
```

---

## REPL sticky behaviour

The REPL treats `txn begin` as a session-wide flag rather than a
per-command argument. Once open:

- Every supporting verb auto-attaches: `encode`, `forget`, `link`,
  `unlink`, `recall` (recall sees the txn's pending writes in its
  read view).
- The prompt shows `brain*> ` while a txn is sticky; `brain> ` when
  not.
- `txn commit <id>` / `txn abort <id>` clear the sticky txn when the
  id matches.
- `\unset txn` clears the sticky txn **locally** — no server call.
  Useful when the txn id is stale and you don't want to round-trip
  to abort.
- Server-side termination is reflected client-side: if any op returns
  `TxnNotFound` or `TransactionTimeout` (the two terminal txn errors
  — see [`commands/mod.rs::is_txn_terminal`](../../../../crates/brain-shell/src/commands/mod.rs)),
  the session's sticky txn is dropped. The next prompt is `brain> `
  again — no need to remember to `\unset` it.

The flag `--txn <HEX>` on individual verbs always wins over the
sticky id, so you can manually thread a different transaction
through a single op without disturbing the session state.

---

## Wire-level semantics

The server holds buffered ops in a per-shard txn table. Commit:

1. Allocates a **single** WAL bracket (one `TXN_BEGIN` record, the
   buffered op records, one `TXN_COMMIT` record).
2. Opens **one** redb write transaction.
3. Applies every op against redb and the in-memory state.
4. fsyncs the WAL bracket.
5. Acks the client.

If step 3 fails (e.g. one of the buffered ops references a memory
that's since been hard-forgotten and reclaimed), the entire bracket
aborts — no partial state lands. The error code surfaces the offending
op.

Abort skips steps 1–4 entirely; the WAL has no record the txn
existed.

---

## Output

### Table

```
# txn begin
ok  txn_id=0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5  timeout_seconds=30

# txn commit
ok  txn_id=0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5  operations_applied=3

# txn abort
ok  txn_id=0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5  operations_discarded=3
```

### JSON

```json
// txn begin
{ "op": "txn_begin",
  "result": {
    "txn_id": "0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5",
    "timeout_seconds": 30,
    "started_at_unix_nanos": 1779153941479431250
  } }

// txn commit
{ "op": "txn_commit",
  "result": {
    "txn_id": "0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5",
    "operations_applied": 3,
    "committed_at_unix_nanos": 1779153945123456789
  } }

// txn abort
{ "op": "txn_abort",
  "result": {
    "txn_id": "0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5",
    "operations_discarded": 3
  } }
```

See [`../output-formats.md`](../output-formats.md) for `wide` / `yaml`
/ `jsonpath=` variants.

---

## Examples

```bash
# REPL: atomic graph rewrite
brain> txn begin
ok  txn_id=0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5  timeout_seconds=30
brain*> encode "deploy step 3 fixed the bug" --context 4
brain*> link s2/m3/v1 caused s2/m2/v1
brain*> unlink s2/m1/v1 contradicts s2/m2/v1
brain*> txn commit 0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5
ok  txn_id=0x019e3ac4b1c67f73a7d2c4e7f1a9b3c5  operations_applied=3
brain> _

# One-shot: capture the id, thread it through, commit
TXN=$(brain txn begin -o json | jq -r '.result.txn_id')
brain encode "the build broke" --context 4 --txn "$TXN"
brain encode "rollback triggered" --context 4 --txn "$TXN"
brain link s2/m17/v1 caused s2/m18/v1 --txn "$TXN"
brain txn commit "$TXN"

# Bail out: drop the session txn locally without round-tripping
brain> txn begin
brain*> encode "this is a mistake"
brain*> \unset txn
brain> # back to normal — server-side txn is still open and will
brain> # time out after 30s; abort explicitly if you care.

# Discover that the txn timed out
brain> link s2/m1/v1 caused s2/m2/v1 --txn 0xdead...beef
error: TransactionTimeout: txn 0xdead...beef has expired
brain> # session sticky txn auto-cleared by is_txn_terminal
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `InvalidArgument` | Bad id (wrong length, non-hex). | Use the id from `txn begin`'s response verbatim. |
| `TxnNotFound` | Commit/abort against a non-existent id, or a verb's `--txn` references an id the server doesn't know. **Terminal** — the shell clears the session sticky txn. | Reopen with `txn begin`. |
| `TransactionTimeout` | The txn exceeded its server-side timeout (default 30s) before commit. **Terminal** — the shell clears the session sticky txn. | Reopen with `txn begin`; keep transactions short. |
| `Conflict` | Idempotency-key reuse for `txn begin` with mismatched params (rare). | Generate a fresh request_id. |
| `Overloaded` | Server out of transaction slots. | Back off; commit/abort outstanding transactions first. |
| `ShardUnavailable` | Coordinator shard down. | Wait + retry. |

Full catalogue: [`../errors.md`](../errors.md).

---

## See also

- [`encode.md`](encode.md) — `--txn <HEX>` attaches an encode
- [`forget.md`](forget.md) — `--txn <HEX>` attaches a forget
- [`link.md`](link.md) / [`unlink.md`](unlink.md) — `--txn <HEX>` attaches an edge mutation
- [`recall.md`](recall.md) — `--txn <HEX>` reads the txn's pending write view
- [`../repl-meta.md`](../repl-meta.md) — `\unset txn` and other session meta-commands
- [`../output-formats.md`](../output-formats.md) — table + JSON + ndjson + yaml + jsonpath
- [`../errors.md`](../errors.md) — error codes
- Spec: [`spec/09_cognitive_operations/08_transactions.md`](../../../../spec/09_cognitive_operations/08_transactions.md)
