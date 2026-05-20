# `\info` (REPL meta)

Diagnostic card stacking the four pieces of state that matter when
something looks wrong: the server you're talking to, the agent
you're talking as, whether the connection is authenticated, and your
session preferences. Run it first when troubleshooting "is the right
agent bound?" or "what handshake did we negotiate?".

**REPL only.** A one-shot equivalent exists at
[`../commands/info.md`](../commands/info.md) (`brain info`) — same
card, same renderer, no shell state.

---

## Synopsis

```
\info
info
```

Both spellings work in the REPL — `info` (no backslash) is routed
through the same code path as `\info` so users who type the bare
verb don't hit a clap parse error. See
`crates/brain-shell/src/repl/loop.rs` lines 89–107.

The card is built by `commands::info::collect` and rendered by the
shared `brain-explore` `InfoCard` renderer, so the table layout
matches what one-shot `brain info` prints.

---

## Behavior

`\info` is read-only — it never mutates session state, never sends a
mutating wire op. It does call `Client::session()` once to snapshot
the cached welcome handshake; on a healthy connected client that's a
pool lookup with no network. If the call fails (server gone, pool
closed) the Server section reports `(not connected)` and the
Connection section reports `state (not connected)` — the renderer's
job is to render *something* useful even when the connection has
misbehaved.

Four stacked sections:

### Server

| Row | Source |
|---|---|
| `address` | `session.server` (the `SocketAddr` the REPL is talking to). |
| `server_id` | Welcome frame — opaque server identity. |
| `wire_version` | Negotiated protocol version. |
| `server_time` | Server wall clock at handshake (raw nanos + RFC3339 + relative age + `server clock` note — useful for diagnosing client/server skew). |
| `bound_shard` | Which shard the connection is pinned to (set during `AuthOk`). |
| `capabilities` | Comma-joined: `streaming`, `zstd`, `push`. `(none)` if all off. |

On a disconnected client only `address` is printed; the rest collapses to `(not connected)`.

### Agent

| Row | Source |
|---|---|
| `name` | Resolved agent name (omitted on raw-id / ephemeral). |
| `id` | Full UUID — the wire `agent_id`. |
| `source` | Provenance label, same wording as the connect banner. |
| `default` | `yes` / `no` — whether this agent is marked default in config. |
| `note` | From the `[agents.<name>]` entry (omitted when empty). |
| `created_at` | RFC3339 + relative age (omitted when no config entry to read from). |

### Connection

| Row | Source |
|---|---|
| `state` | `authenticated` when the welcome snapshot succeeded; `(not connected)` otherwise. |
| `connected_at` | Omitted in v1 — the SDK doesn't yet expose a connect timestamp. |

### Session

| Row | Source |
|---|---|
| `output` | Current renderer choice (`auto`/`table`/`wide`/`json`/`ndjson`/`yaml`). |
| `sticky_context` | `\set context <N>` value, or `(none)`. |
| `active_txn` | Formatted txn id, or `none`. |
| `timing` | `on` / `off`. |

---

## Output sample

```
brain> \info
Server
  address       127.0.0.1:9090
  server_id     brain-server-01HMK3
  wire_version  1
  server_time   1779153941479431250  [2026-05-20T10:05:41Z, 12s ago, server clock]
  bound_shard   2
  capabilities  streaming, zstd

Agent
  name        work
  id          019e3d1f-bd66-7890-a4bc-947ab6ca9c3e
  source      config active = work (/Users/dodo/.config/brain/config.toml)
  default     yes
  note        prod work notebook
  created_at  2026-05-19T10:00:00Z [1 day ago]

Connection
  state         authenticated

Session
  output          table
  sticky_context  7
  active_txn      none
  timing          off
```

JSON / YAML / NDJSON output produces the same four blocks in a
structured envelope; see the `InfoCard::render_json` implementation
in `crates/brain-explore/src/render/info.rs`.

---

## Examples

```bash
# First thing to run when something looks off
brain> \info

# Confirm an agent rebind landed on the wire
brain> \agent use demo
brain> \connect 127.0.0.1:9090
brain> \info | grep '^  id\|^  source'

# JSON for a quick scripted health check
brain> \set output json
brain> \info
{"server":{...}, "agent":{...}, "connection":{...}, "session":{...}}

# Disconnected-state probe
brain> \connect 127.0.0.1:9999          # server down → connect failed
brain> \info                            # Server: address only; Connection: (not connected)

# Diagnose client/server clock skew
brain> \info | grep server_time         # the `server clock` note is the hint
```

---

## See also

- [`agent.md`](agent.md) — the verb that controls the Agent section
- [`connect.md`](connect.md) — the verb that controls the Server section
- [`set.md`](set.md), [`timing.md`](timing.md) — the verbs that control the Session section
- [`../output-formats.md`](../output-formats.md) — JSON envelope shape
- One-shot equivalent: [`../commands/info.md`](../commands/info.md) (`brain info` — same card, no shell state)
