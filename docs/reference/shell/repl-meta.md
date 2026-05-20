# `brain` shell — REPL meta-command index

Backslash-prefixed commands (and a few un-prefixed aliases) intercepted
**before** `clap` parses, so they never round-trip to the server. Each
entry below links to a stand-alone reference page. For server-verbs, see
[`commands.md`](commands.md); for the overview, see
[`../brain-shell.md`](../brain-shell.md).

All meta-commands are REPL-only — they don't work in one-shot mode.
For their one-shot equivalents (where one exists), see
[`commands/config.md`](commands/config.md) and
[`commands/agent.md`](commands/agent.md).

---

## Session control

| Command | What it does | Reference |
|---|---|---|
| `quit`, `exit`, `\q`, Ctrl-D | Exit the shell cleanly. | — |
| `help [VERB]`, `? [VERB]`, `\?`, `\help` | In-REPL help (psql aliases). | [`meta/help.md`](meta/help.md) |
| `\connect <host:port>` | Reconnect to a different server. | [`meta/connect.md`](meta/connect.md) |

## Live-session settings

These mutate the **live session only** — they don't touch
`~/.config/brain/config.toml`. For persistence, see
[`meta/config.md`](meta/config.md).

| Command | What it does | Reference |
|---|---|---|
| `\set output {auto\|json\|table\|wide\|ndjson\|yaml}` | Output format for this session. | [`meta/set.md`](meta/set.md) |
| `\set context <N>` | Sticky `--context` default; prompt updates to `brain[ctx=N]> `. | [`meta/set.md`](meta/set.md) |
| `\unset txn` | Drop the sticky txn locally (does **not** abort it on the server). | [`meta/unset.md`](meta/unset.md) |
| `\timing on\|off` | Show per-op wall time. | [`meta/timing.md`](meta/timing.md) |

## Persistent settings (also mutate live session)

| Command | What it does | Reference |
|---|---|---|
| `\config list\|get\|set\|path\|edit` | mongosh-style "set + persist" — writes the file AND updates the live session. | [`meta/config.md`](meta/config.md) |

## Named agents

| Command | What it does | Reference |
|---|---|---|
| `\agent` (bare) | Current binding — agent id + resolution source. | [`meta/agent.md`](meta/agent.md) |
| `\agent list\|show\|use\|create\|set-default` | Named-agent surface available in the REPL. `rename`/`delete`/`import` are one-shot only. | [`meta/agent.md`](meta/agent.md) |

## Diagnostics

| Command | What it does | Reference |
|---|---|---|
| `\info`, `info` | Stacked diagnostic card (server + agent + connection + session). | [`meta/info.md`](meta/info.md) |

---

## Prompt encoding

The REPL prompt reflects session state at a glance:

| Prompt | Meaning |
|---|---|
| `brain> ` | No active txn, no sticky context. |
| `brain*> ` | Active transaction. |
| `brain[ctx=7]> ` | Sticky context = 7. |
| `brain*[ctx=7]> ` | Both. |

The connection banner at REPL entry also shows the bound agent + its
resolution source — see [`configuration.md`](configuration.md).

---

## Completion (REPL)

Tab cycles through:

1. **Subcommands** at the start of a line — `enc<TAB>` → `encode`.
2. **Flag names** after a subcommand — `encode "x" --c<TAB>` → `--context`.
3. **Enum values** after an enum-flag — `encode "x" --kind <TAB>` →
   `episodic | semantic | consolidated`.

For tab completion in **non-REPL** shells (bash/zsh/fish), see
[`commands/generate-completion.md`](commands/generate-completion.md).

## History

```
$XDG_DATA_HOME/brain/history          # primary
~/.local/share/brain/history          # XDG default
~/.brain_history                      # fallback
```

Loaded on REPL start; appended after each entered line.

---

## See also

- [`commands.md`](commands.md) — server verbs
- [`configuration.md`](configuration.md) — config file + agent resolution
- [`output-formats.md`](output-formats.md) — table + JSON shapes
- [`errors.md`](errors.md) — error codes + exit codes
- [`../../guides/shell/named-agents.md`](../../guides/shell/named-agents.md) — task-oriented walkthrough
