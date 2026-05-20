# `\unset` (REPL meta)

Drop session-local state. Today the only supported key is `txn`,
which detaches the shell from an active transaction without telling
the server. Use it when a long-running txn is in your way and you'd
rather let it age out than fish out the txn id for `txn abort`.

**REPL only.** One-shot invocations don't carry transactions across
calls, so there's nothing to unset.

---

## Synopsis

```
\unset txn
```

That is the entire surface. `\unset context`, `\unset output`,
`\unset timing` — none of these are recognised by `parse_meta` in
`crates/brain-shell/src/repl/loop.rs`. The earlier `repl-meta.md`
documented `\unset context`; that claim is stale. Clear a sticky
context by overwriting it with `\set context 0` (if you actually want
context 0) or by restarting the shell.

---

## Behavior

`\unset txn` sets `session.active_txn = None` and prints `active txn
cleared`. The effects, in order:

1. **Local detach only.** The shell no longer auto-attaches `--txn`
   to subsequent `encode` / `recall` / `link` / `unlink` calls.
2. **Server keeps the txn open.** No `TXN_ABORT` frame is sent. The
   server-side transaction sits in its registry until its idle
   timeout fires (per spec §07/05), at which point the server aborts
   it and reclaims the slot. Any writes you'd queued in the txn
   never land.
3. **Prompt drops the `*` marker.** `brain*> ` → `brain> `;
   `brain*[ctx=7]> ` → `brain[ctx=7]> `.
4. **`txn commit <id>` on the same id later may still succeed** if
   the server hasn't timed it out yet — `\unset txn` doesn't lose
   the id, the user does (it's no longer in the session, only the
   server knows it).

When you actually want to abort, use the verb:

```
brain*> txn abort <hex-id>
```

That sends `TXN_ABORT` to the server, frees resources immediately,
and is the right call for hot paths.

The `\unset txn` path also fires automatically (without user input)
when a write inside the active txn returns `TxnNotFound` /
`TransactionTimeout` — the REPL stamps a `note: server reported the
active transaction is no longer usable; session no longer attached
to txn …` line and clears the stale id. See
`crates/brain-shell/src/repl/loop.rs` around line 248.

---

## Output sample

```
brain> txn begin
txn 0x4f3a…91c2 begun
brain*> encode "kickoff"
ok  s2/m9/v1  lsn=42
brain*> \unset txn
active txn cleared
brain>
```

---

## Examples

```bash
# Drop a stuck txn and carry on (server times it out on its own)
brain*> \unset txn
brain>

# Better — explicit abort talks to the server
brain*> txn abort 4f3a000000000000000000000000a91c
txn aborted

# The cousin command: clears sticky output, NOT implemented — use \set instead
brain> \unset output             # → unknown meta command: \unset output
brain> \set output table         # use this to "reset" to the default
```

---

## See also

- [`set.md`](set.md) — the verb that sets these in the first place
- [`info.md`](info.md) — shows `active_txn` so you can confirm the local detach
- [`../commands/txn.md`](../commands/txn.md) — `txn abort`, the server-aware way to release a transaction
- Spec: [`spec/07_metadata_graph/05_transactions.md`](../../../../spec/07_metadata_graph/05_transactions.md) — server-side txn lifetime
- One-shot equivalent: none. Transactions don't span one-shot invocations.
