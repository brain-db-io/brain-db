# `\config` (REPL meta)

Read, edit, and persist shell preferences from inside the REPL.
Mirrors the one-shot `brain config` family, with one key difference:
`\config set` *also* mutates the live session so you don't need to
quit + restart to see the new setting take effect (the mongosh
"set + persist" pattern).

**REPL only.** For scripts and CI use `brain config …` — see
[`../commands/config.md`](../commands/config.md) for the persistence
schema.

---

## Synopsis

```
\config list
\config get <KEY>
\config set <KEY> <VALUE>
\config path
\config edit
```

Bare `\config` (no subcommand) prints `unknown meta command: \config
requires a subcommand`. Subcommands take exactly the argument shapes
above; extra tokens fall through to `Meta::Unknown` rather than
truncating silently.

---

## Behavior

### `\config list`

Print every effective `[settings]` key with its current value, one
per line. Reads the on-disk config (merged with defaults).

### `\config get <KEY>`

Print a single setting's value. `<KEY>` must be one of `output`,
`timing`, `sticky_context`, `server`. Unknown keys surface the
config layer's "did you mean…" error.

### `\config set <KEY> <VALUE>`

Validate, write `~/.config/brain/config.toml` atomically, and
**also** mirror the change into the live session. The live-session
mirror is selective:

| Key | Persisted? | Live session? | Notes |
|---|---|---|---|
| `output` | yes | yes | `session.output` updated in place. |
| `timing` | yes | yes | Accepts `true` / `on` / `1` as true; everything else is false. |
| `sticky_context` | yes | yes | Parsed as `u64`; non-numeric values are persisted but the live mirror silently no-ops. |
| `server` | yes | **no** | Persisted only — `\connect <host:port>` is the live verb so the existing connection isn't yanked out from under you. |

On success the verb prints `<KEY> = <VALUE>`. A schema rejection
prints `error: <reason>` and leaves both disk and live session
unchanged.

### `\config path`

Print the absolute path of the config file (XDG-aware:
`$XDG_CONFIG_HOME/brain/config.toml` or `~/.config/brain/config.toml`).

### `\config edit`

Launch `$VISUAL`, then `$EDITOR`, then `vi` against the config file.
The file is created (with the current settings + agents) before the
editor opens if it doesn't exist yet — so first-time edits don't see
an empty buffer. After the editor exits, the shell does **not**
re-read the file; quit + restart, or use `\config set` for the
specific keys you changed, to pick up edits in the live session.

---

## Output sample

```
brain> \config list
output          table
timing          false
sticky_context  7
server          127.0.0.1:9090

brain> \config get output
table

brain> \config set output json
output = json
brain> recall "x"                  # already JSON, no restart needed
{"op":"recall","result":[...]}

brain> \config set server 10.0.5.21:9090
server = 10.0.5.21:9090            # persisted; live connection unchanged
brain> \connect 10.0.5.21:9090     # apply now

brain> \config path
/Users/dodo/.config/brain/config.toml

brain> \config edit
# (opens $VISUAL on the file)
```

---

## Examples

```bash
# Pin output to wide for every future shell, and right now
brain> \config set output wide

# Bump the default context for this project
brain> \config set sticky_context 12
brain[ctx=12]>                                # prompt picks it up

# Change servers — persist + apply in two steps
brain> \config set server 10.0.5.21:9090
brain> \connect 10.0.5.21:9090

# Read a single value into your fingers
brain> \config get sticky_context
12

# Hand-edit; restart to pick up the change
brain> \config edit
brain> quit
$ brain shell

# Compare with the session-only verb
brain> \set output json              # this session only, never persisted
brain> \config set output json       # persisted AND applied
```

---

## See also

- [`set.md`](set.md) — session-only counterpart (`\set output …`)
- [`timing.md`](timing.md) — `\timing on|off` is the boolean shortcut for `output=timing`
- [`connect.md`](connect.md) — the live verb for `server`
- [`agent.md`](agent.md) — the agent CRUD surface in `[agents.<name>]`
- [`../configuration.md`](../configuration.md) — full file schema + resolution precedence
- One-shot equivalent: [`../commands/config.md`](../commands/config.md) (`brain config list|get|set|path|edit`)
