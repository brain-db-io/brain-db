# `brain` shell — per-verb command index

Every verb the `brain` binary exposes, in depth. Each entry below
links to a stand-alone reference page. For the overview + quick
start, see [`../brain-shell.md`](../brain-shell.md); for backslash
meta-commands, see [`repl-meta.md`](repl-meta.md).

Every verb works in both one-shot and REPL mode unless its page
notes otherwise.

---

## Cognitive primitives

| Verb | What it does | Reference |
|---|---|---|
| `encode` | Write a memory; return id + WAL `lsn`. | [`commands/encode.md`](commands/encode.md) |
| `recall` | Vector-similarity search; ranked `MemoryResult`s. | [`commands/recall.md`](commands/recall.md) |
| `plan` | Stepwise causal/temporal path from one state to another. | [`commands/plan.md`](commands/plan.md) |
| `reason` | Inference chain from an observation. | [`commands/reason.md`](commands/reason.md) |
| `forget` | Tombstone a memory (soft or hard). | [`commands/forget.md`](commands/forget.md) |
| `link` | Add a typed edge between two memories. | [`commands/link.md`](commands/link.md) |
| `unlink` | Remove an edge. Idempotent. | [`commands/unlink.md`](commands/unlink.md) |

## Transactions + streams

| Verb | What it does | Reference |
|---|---|---|
| `txn begin\|commit\|abort` | Multi-op atomic batch with REPL sticky behavior. | [`commands/txn.md`](commands/txn.md) |
| `subscribe` | Live + replay change-feed stream. `--start-lsn`, `--collect`. | [`commands/subscribe.md`](commands/subscribe.md) |

## Knowledge layer (active when a schema is declared)

| Verb | What it does | Reference |
|---|---|---|
| `entity list\|show\|neighbors` | Browse entities written by the extractor pipeline. | [`commands/entity.md`](commands/entity.md) |
| `statement list\|show` | Browse Fact / Preference / Event statements. | [`commands/statement.md`](commands/statement.md) |
| `relation list` | Browse typed relations. | [`commands/relation.md`](commands/relation.md) |
| `mention list` | Inspect Mentions edges (memory ↔ entity provenance). | [`commands/mention.md`](commands/mention.md) |
| `extract status\|backfill` | Extraction audit + admin backfill. | [`commands/extract.md`](commands/extract.md) |

## Session + admin

| Verb | What it does | Reference |
|---|---|---|
| `config list\|get\|set\|path\|edit` | Persistent shell settings. | [`commands/config.md`](commands/config.md) |
| `agent list\|show\|create\|rename\|delete\|import\|set-default` | Named-agent CRUD. | [`commands/agent.md`](commands/agent.md) |
| `info` | One-shot diagnostic card (server + agent + connection + session). | [`commands/info.md`](commands/info.md) |
| `shell` | Explicit REPL entry. Same as bare `brain`. | [`commands/shell.md`](commands/shell.md) |
| `generate-completion <SHELL>` | Emit a bash/zsh/fish/powershell/elvish completion script. | [`commands/generate-completion.md`](commands/generate-completion.md) |

---

## Global options (apply to every verb)

| Option | Default | Notes |
|---|---|---|
| `--server <host:port>` | `127.0.0.1:9090` | Also reads `BRAIN_SERVER`. |
| `--agent <name>` | — | Named agent from `~/.config/brain/config.toml`. Reads `BRAIN_AGENT`. |
| `--agent-id <UUID>` | random UUIDv7 (ephemeral) | Raw id; skips config lookup. Reads `BRAIN_AGENT_ID`. |
| `--output / -o <FORMAT>` | `auto` (table on TTY, ndjson piped) | `auto\|table\|wide\|json\|ndjson\|yaml\|jsonpath=<EXPR>`. |
| `--color {auto\|always\|never}` | `auto` | Honours `NO_COLOR` / `CLICOLOR` / isatty. |
| `--hyperlinks {auto\|always\|never}` | `auto` | OSC 8 hyperlink policy. |
| `--timeout <SECS>` | `30` | Per-op wall-clock budget. |
| `--token <VALUE>` | — | Reserved for v2 auth (parsed and ignored in v1). |

**Conflicts:** `--agent` + `--agent-id` together → error; `BRAIN_AGENT` + `BRAIN_AGENT_ID` together → error.

---

## See also

- [`../brain-shell.md`](../brain-shell.md) — overview
- [`repl-meta.md`](repl-meta.md) — backslash meta-commands
- [`output-formats.md`](output-formats.md) — table + JSON + ndjson + yaml + jsonpath shapes
- [`configuration.md`](configuration.md) — `~/.config/brain/config.toml` + agent resolution
- [`errors.md`](errors.md) — error codes + exit codes
- [`../../guides/shell/`](../../guides/shell/) — task-oriented playbooks
- [`../../tutorials/03-shell-deep-dive.md`](../../tutorials/03-shell-deep-dive.md) — 20-minute guided tour
