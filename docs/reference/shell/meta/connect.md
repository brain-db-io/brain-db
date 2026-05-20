# `\connect` (REPL meta)

Reconnect the live shell to a different server endpoint without
quitting. Use it to point an already-loaded REPL at a staging vs prod
instance, or to recover from a server restart while keeping your
sticky context / output preferences / agent binding intact.

**REPL only.** Outside the shell pass `--server <host:port>` or set
`BRAIN_SERVER`.

---

## Synopsis

```
\connect <host:port>
```

`<host:port>` must parse as a `SocketAddr` — bare hostnames are
rejected. IPv4 (`127.0.0.1:9090`), IPv6 (`[::1]:9090`), and any
address `parse_server` accepts (see
`crates/brain-shell/src/parser/command.rs`) are valid.

---

## Behavior

1. **Validate the address.** A parse failure prints
   `invalid --server '<arg>': …` and leaves the existing connection
   untouched.
2. **Pick the agent.** `session.sticky_agent` (set via
   [`\agent use <NAME>`](agent.md)) wins; otherwise the
   process-bound `agent_id` resolved at REPL start is reused. This is
   what lets `\agent use` → `\connect` actually change the wire
   identity.
3. **Open a fresh connection.** 30-second handshake budget. On
   success replaces the existing `Client`, replaces
   `session.server`, prints `connected to <addr> as <agent_id>`.
4. **On failure**, prints `connect failed: <error>` and leaves the
   existing `Client` and `session.server` alone — you're still
   talking to whatever you were talking to before.

What it does **not** carry across:

- **Open transactions.** The server-side txn is bound to the prior
  connection; `\connect` doesn't try to migrate it. Commit or abort
  first, or be prepared for the next `encode --txn …` to fail with
  `TxnNotFound`. The shell's `session.active_txn` is not cleared
  automatically — that's a known sharp edge; clear it with
  [`\unset txn`](unset.md) if you reconnected mid-transaction.
- **Subscribe streams.** A `subscribe` that was running before
  reconnect is on the old connection; restart it after `\connect`.
- **Idempotency cache.** Cached against `(agent_id, request_id)` on
  the server — survives reconnects to the same server, doesn't
  follow you across servers.

---

## Output sample

```
brain> \connect 10.0.5.21:9090
connected to 10.0.5.21:9090 as 019e3d1f-bd66-7890-a4bc-947ab6ca9c3e

brain> \connect localhost:9090
invalid --server 'localhost:9090': invalid socket address syntax

brain> \connect 127.0.0.1:9999
connect failed: connection refused
brain> \info | head -3
Server
  address       127.0.0.1:9090            # unchanged — old session still live
```

---

## Examples

```bash
# Bounce between local and staging in one session
brain> \connect 127.0.0.1:9090
brain> encode "local test"
brain> \connect 10.0.5.21:9090
brain> recall "local test"                 # different server — empty results

# Rebind agent and reconnect to apply
brain> \agent use prod
brain> \connect 10.0.5.21:9090             # now bound as 'prod' on the wire

# Recover from a server restart without losing sticky state
brain[ctx=7]> \connect 127.0.0.1:9090
brain[ctx=7]>                              # ctx=7 preserved

# Always commit/abort before changing servers
brain*> txn commit 4f3a…91c2
brain> \connect 10.0.5.21:9090
```

---

## See also

- [`agent.md`](agent.md) — `\agent use <NAME>` then `\connect` to rebind identity
- [`unset.md`](unset.md) — `\unset txn` if you reconnected with a stale txn id stuck on the session
- [`info.md`](info.md) — confirms `address` + `agent_id` post-reconnect
- [`../configuration.md`](../configuration.md) — `server` setting and resolution precedence
- One-shot equivalent: `brain --server <host:port> …` or `BRAIN_SERVER=<host:port> brain …` per call.
