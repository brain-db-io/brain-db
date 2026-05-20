# `\agent` (REPL meta)

Inspect the current agent binding and manage named identities from
inside the REPL. The bare form is read-only — it answers "which
agent_id is this session sending on the wire, and where did it come
from?" The subcommands cover the everyday CRUD an active shell user
needs (list, show, use, create, set-default); the heavier
operations live on the one-shot CLI.

**REPL only.** For `rename`, `delete`, `import` — and for scripting —
use `brain agent …` in another terminal. See
[`../commands/agent.md`](../commands/agent.md).

---

## Synopsis

```
\agent                                # current binding
\agent list                           # named agents in config
\agent show [<NAME>]                  # full record for one agent
\agent use <NAME>                     # switch + persist as active
\agent create <NAME> [--note <TEXT>]  # mint a fresh agent
\agent set-default <NAME>             # mark as default (sticky fallback)
```

Subcommands not implemented in the meta: `rename`, `delete`, `import`.
The parser falls through to `Meta::Unknown` for these. They exist on
the one-shot binary (`brain agent rename / delete / import`).

---

## Behavior

### `\agent` (bare)

Prints the live binding — `agent_id`, `source` label (where the
binding came from), and `name` (when the source has one). The label
is the same wording the connect banner uses, so the two surfaces
agree.

| Source | Label |
|---|---|
| `--agent <name>` | `flag --agent <name> (<config path>)` |
| `--agent-id <uuid>` | `flag --agent-id` |
| `BRAIN_AGENT=<name>` | `env BRAIN_AGENT=<name> (<config path>)` |
| `BRAIN_AGENT_ID=<uuid>` | `env BRAIN_AGENT_ID` |
| `[settings].active = <name>` | `config active = <name> (<config path>)` |
| `[settings].default = <name>` | `config default = <name> (<config path>)` |
| Auto-minted on first run | `auto-minted <name> (<config path>)` |
| Ephemeral (no config file) | `ephemeral (no config file path available)` |

In `auto` / `table` / `wide` output the card is three lines; in
`json` / `ndjson` / `yaml` the renderer emits a structured envelope
with `agent_id`, `source` (kebab-case kind tag), and `name`.

### `\agent list`

Tabular roster from the on-disk config — one row per `[agents.<name>]`
block. The agent the live session is currently bound to is marked
with a leading `*`. Empty config prints
`(no named agents — \`\agent create <name>\` to add one)`.

### `\agent show [<NAME>]`

Full record (`name`, `id`, `created_at`, `note`). With `<NAME>`
omitted, falls back to the session's current binding's name (if any);
on a raw-id or no-config-dir session with no name to fall back to,
prints `(no named agent — raw-id or ephemeral session)`. The
first-run bare-`brain` path doesn't hit this branch: the resolver
auto-mints `agent-<8hex>` and persists it, so it has a name to show.

### `\agent use <NAME>`

Live rebind. Semantics:

1. **Refuses on active txn.** If `session.active_txn.is_some()`,
   prints `error: active transaction prevents agent rebind — commit
   or abort first` and aborts. The txn id is tied to the current
   agent's connection; switching mid-txn would orphan it.
2. **Looks up the name** in the config file. Missing name → error.
   Malformed UUID in the file → error.
3. **Stashes the new id** on `session.sticky_agent`. The wire
   identity does **not** change yet — the current `Client` is still
   holding the old agent's connection.
4. **Persists the switch** by setting the config's `active` flag.
   This is best-effort: if the file write fails, the live session
   still uses the new id, but the user gets a `note: could not
   persist active flag to config: …` warning so they know
   session-vs-disk has diverged.
5. **Apply on the wire** via [`\connect <host:port>`](connect.md)
   — that's when `Client::connect` re-handshakes with the new
   `agent_id`. A bare process restart will also pick it up, since
   the config's `active` flag wins on next launch.

### `\agent create <NAME> [--note <TEXT>]`

Mint a fresh UUIDv7, write `[agents.<name>]` (with `created_at` now,
plus the optional note), and persist. Name collision → error.

**Empty-config promote rule.** If this is the very first agent in
the file, it is automatically promoted to both `default` and `active`
so the config file is never left in a "has agents but none chosen"
state. Subsequent creates do not auto-promote — use `set-default`
and `use` for that.

### `\agent set-default <NAME>`

Mark `<NAME>` as the default agent in the config file. The default
is the factory fallback used when no `--agent` / `BRAIN_AGENT` /
`active` is set. Does not touch the live session — it influences the
*next* `brain` invocation, not this one.

---

## Output sample

```
brain> \agent
agent_id = 019e3d1f-bd66-7890-a4bc-947ab6ca9c3e
source   = config active = work (/Users/dodo/.config/brain/config.toml)
name     = work

brain> \agent list
   NAME             ID                                   CREATED              NOTE
*  work             019e3d1f-bd66-7890-a4bc-947ab6ca9c3e  2026-05-19T10:00:00Z  prod work notebook
   demo             019e3d1f-bd66-7891-bf07-1245aab10001  2026-05-19T11:30:00Z

brain> \agent show demo
name       = demo
id         = 019e3d1f-bd66-7891-bf07-1245aab10001
created_at = 2026-05-19T11:30:00Z

brain> \agent create scratch --note "throwaway"
created agent 'scratch' (019e3d1f-bd66-7892-c012-aa5500004422)

brain> \agent use demo
sticky agent set to 'demo' (019e3d1f-bd66-7891-bf07-1245aab10001); reconnect via `\connect <host:port>` to bind the new id on the wire.

brain*> \agent use demo
error: active transaction prevents agent rebind — commit or abort first

brain> \agent set-default work
default agent → work
```

---

## Examples

```bash
# Who am I right now?
brain> \agent

# Add a per-project agent then switch into it for this session
brain> \agent create acme --note "Acme Corp engagement"
brain> \agent use acme
brain> \connect 127.0.0.1:9090

# Mark the project agent as the factory fallback too
brain> \agent set-default acme

# List + pick from a multi-agent setup
brain> \agent list
brain> \agent use demo
brain> \connect 127.0.0.1:9090

# Anything that's not in the meta — drop to a second terminal
$ brain agent rename acme acme-eng
$ brain agent delete scratch
$ brain agent import shared 01HMK_TEAM_AGENT_ULID
```

---

## See also

- [`info.md`](info.md) — full agent record + handshake + session diagnostic in one card
- [`connect.md`](connect.md) — applies the new `sticky_agent` on the wire
- [`config.md`](config.md) — file-level settings (`active`, `default`)
- [`../configuration.md`](../configuration.md) — resolution precedence, file schema, sharing IDs
- [`../../guides/shell/named-agents.md`](../../../guides/shell/named-agents.md) — workflow walkthrough
- One-shot equivalent: [`../commands/agent.md`](../commands/agent.md) (`brain agent list|show|create|use|set-default|rename|delete|import`)
