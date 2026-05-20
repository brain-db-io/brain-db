# `brain` shell — reference

In-depth, look-it-up reference for the `brain` interactive shell.
For the overview + quick-start, start at
[`../brain-shell.md`](../brain-shell.md). For task-oriented how-tos,
see [`../../guides/shell/`](../../guides/shell/). For a 20-minute
guided walkthrough, see
[`../../tutorials/03-shell-deep-dive.md`](../../tutorials/03-shell-deep-dive.md).

## Per-verb reference (one file per command)

| Area | Index |
|---|---|
| Server verbs (`encode`, `recall`, `plan`, `reason`, `forget`, `link`, `unlink`, `txn`, `subscribe`, `entity`, `statement`, `relation`, `mention`, `extract`, `config`, `agent`, `info`, `shell`, `generate-completion`) | [`commands.md`](commands.md) → [`commands/`](commands/) |
| REPL meta-commands (`\set`, `\unset`, `\timing`, `\connect`, `\config`, `\agent`, `\info`, `help`) | [`repl-meta.md`](repl-meta.md) → [`meta/`](meta/) |

## Cross-cutting reference

| Page | What's in it |
|---|---|
| [`output-formats.md`](output-formats.md) | Table vs JSON; per-verb JSON schemas; `auto` / `wide` / `ndjson` / `yaml` / `jsonpath=…` variants; memory-id short vs long form; streaming subscribe shape. |
| [`configuration.md`](configuration.md) | `~/.config/brain/config.toml` schema, agent resolution precedence, `brain agent …` / `brain config …` CLI, migration from legacy singleton config. |
| [`errors.md`](errors.md) | Wire error codes → rendered messages → exit codes. Common cases with remedies. |
