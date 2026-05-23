# `brain info`

One-shot diagnostic card: who you connected to, as whom, with what
capabilities, and what session preferences are live. Mirrors the
REPL's `\info` meta-command — same data, same renderer, runs once
and exits. Reach for it when CI needs to assert a deployment posture
(`brain info -o json | jq …`), when an operator pings you with
"what's the wire version?", or when you want to confirm a flag took
effect before encoding anything.

```
brain info
brain info -o json
```

Unlike most verbs, `info` runs the **full** startup sequence — agent
resolution, settings load, TCP connect, handshake — and only *then*
renders. That's the point: the card is the diagnosis of that
sequence. A connect failure in `info` is fail-stop (exit 1); a
*post-connect* handshake hiccup degrades gracefully so the card
still renders with `(not connected)` in the server block.

---

## What it collects

Four blocks, populated independently:

| Block | Source |
|---|---|
| **Server** | The SDK's session handshake (`Welcome` + `AuthOk` frames). Missing → `(not connected)`. |
| **Agent** | The resolved agent id + source label, plus the matching `AgentEntry` from the config file (if any). |
| **Connection** | Whether the SDK considers the connection authenticated. |
| **Session** | This invocation's output format, sticky context, active txn, timing flag — i.e. what `\set` would mutate inside the REPL. |

The collection function (`commands/info.rs`) is split from the
renderer (in `brain-explore`) so the shell decides *what* to read
without the renderer pulling in `brain-sdk-rust`. Both surfaces
(`brain info` and `\info`) call the same collector.

---

## Output

### Table

```
Server
  address       127.0.0.1:9090
  server_id     brain-dev-01
  wire_version  1
  server_time   1779153941479431250 (2026-05-15T14:05:41Z, 4s ago) [server clock]
  bound_shard   2
  capabilities  streaming, push

Agent
  name          work
  id            019e3d1f-bd66-7890-a4bc-947ab6ca9c3e
  source        --agent work
  default       yes
  note          prod work notebook
  created_at    2026-05-19T10:00:00Z (1 day ago)

Connection
  state         authenticated

Session
  output        table
  sticky_context  (none)
  active_txn    none
  timing        off
```

### When the connection misbehaves

The Server block degrades to `(not connected)` and the Connection
block likewise — the rest of the card still renders, so `info` is
useful even mid-outage.

```
Server
  address       127.0.0.1:9090
  (not connected)

Agent
  name          work
  …

Connection
  state         (not connected)

Session
  …
```

### Source labels

The `source` row mirrors the connect-banner suffix and tells you
which precedence rung the resolver fired on:

| Label | Meaning |
|---|---|
| `--agent <NAME>` | `--agent` flag won. |
| `--agent-id` | `--agent-id` flag won; no name. |
| `BRAIN_AGENT=<NAME>` | Env var named-agent. |
| `BRAIN_AGENT_ID` | Env var raw id; no name. |
| `config: active = <NAME>` | File's `active` flag (set by `\agent use`). |
| `config: default = <NAME>` | File's `default` flag (no `active` present). |
| `auto-minted as <NAME>` | First-run path — the resolver minted + persisted. |
| `ephemeral (no config file)` | No XDG / HOME → in-memory only, nothing persisted. |

See [`agent.md#resolution-precedence`](agent.md#resolution-precedence)
for the full cascade.

### JSON

```json
{
  "server": {
    "address": "127.0.0.1:9090",
    "welcome": {
      "server_id": "brain-dev-01",
      "wire_version": 1,
      "server_time_unix_nanos": 1779153941479431250,
      "bound_shard": 2,
      "capabilities": {
        "streaming": true,
        "compression_zstd": false,
        "server_push": true
      }
    }
  },
  "agent": {
    "name": "work",
    "id": "019e3d1f-bd66-7890-a4bc-947ab6ca9c3e",
    "source": "--agent work",
    "default": true,
    "note": "prod work notebook",
    "created_at": "2026-05-19T10:00:00Z"
  },
  "connection": {
    "authenticated": true,
    "connected_at_unix_nanos": null
  },
  "session": {
    "output": "table",
    "sticky_context": null,
    "active_txn": null,
    "timing": false
  }
}
```

`server.welcome` is `null` when not connected. `agent.name` is
`null` for raw-id and ephemeral flows. `connected_at_unix_nanos` is
always `null` in v1 — the SDK doesn't yet expose a connect
timestamp, and fabricating one was the wrong fix.

---

## Examples

```bash
# Glance at who you're connected as
brain info

# CI assertion: confirm the bound shard
brain info -o json | jq -e '.server.welcome.bound_shard == 2'

# Confirm a flag took effect before doing damage
brain --agent prod info | grep '^  source'
#   source        --agent prod

# Diagnose "why isn't recall finding my memory" — start here
brain info | grep -E '(agent|sticky_context)'

# Stash the wire version for compat gating in scripts
WIRE=$(brain info -o json | jq -r '.server.welcome.wire_version')

# Assert deployment posture across a fleet
ssh node-{1..16} 'brain info -o json | jq -c "{shard:.server.welcome.bound_shard, server:.server.welcome.server_id}"'
```

---

## Errors

| Exit | Trigger | Notes |
|---|---|---|
| `0` | Card rendered (connected or `(not connected)`). | The degraded case is **not** an error — the whole point is to render something. |
| `1` | Connect failed (couldn't open TCP, couldn't complete handshake at all). | Check `brain config get server`, the network, the server process. |
| `1` | Rendering / IO error writing to stdout. | Check stdout isn't a closed pipe; otherwise file a bug. |
| `2` | Argv parse error (bad `-o`, bad `--server`, etc.). | Re-issue. |

`info` does not return wire errors — it never sends a request.
Everything it shows comes from the session-state the SDK already
has, the resolver's output, and the config file.

---

## See also

- [`agent.md`](agent.md) — the resolution rules that drive the Agent block.
- [`config.md`](config.md) — the persisted settings that drive the Session block.
- [`../repl-meta.md`](../repl-meta.md) — the REPL's `\info` (same card).
- Spec: [`spec/06_sdk/03_connection.md`](../../../../spec/06_sdk/03_connection.md) — the handshake that fills the Server block.
- Spec: [`spec/17_observability/04_admin_ops.md`](../../../../spec/17_observability/04_admin_ops.md) — admin-side diagnostics this complements client-side.
