# `\set` (REPL meta)

Mutate a single session preference for this shell only. Two keys are
recognised: `output` (renderer choice) and `context` (sticky
`--context` default for `encode` / `recall`). Use `\set` for "try this
for one session"; use [`\config set`](config.md) when you want the
change to outlive the process.

**REPL only.** In one-shot mode use `--output` per call or `brain
config set output …` for persistence.

---

## Synopsis

```
\set output {auto|table|wide|json|ndjson|yaml}
\set context <N>
```

Any other key, or a recognised key with an invalid value, prints
`unknown meta command: <input>` and leaves the session untouched.
There is no `\set timing` — use [`\timing on|off`](timing.md) for the
boolean toggle.

---

## Behavior

### `\set output <FORMAT>`

Replaces `session.output`. Subsequent commands render with the new
default until the session ends; per-line `-o <FORMAT>` still wins
when supplied. Accepted values:

| Value | Renderer |
|---|---|
| `auto` | Table when stdout is a TTY, JSON otherwise. |
| `table` | Default human format. |
| `wide` | Table with extra columns (full ids, timestamps). |
| `json` | Compact JSON envelope. |
| `ndjson` | One JSON object per result, newline-delimited. Use for streams. |
| `yaml` | YAML envelope. |

The renderer is selected against `OutputFormatArg` in
`crates/brain-shell/src/parser/command.rs`. Anything else (including
`csv`, `tsv`, `jsonpath=…`) is rejected — `jsonpath=` is parse-time
only on `--output`, not on `\set`.

### `\set context <N>`

Stashes a `u64` on `session.sticky_context`. The next `encode` or
`recall` that omits `--context` (or `--filter-context`) inherits this
value. The prompt updates to encode the state at a glance:

| Prompt | Meaning |
|---|---|
| `brain> ` | No active txn, no sticky context. |
| `brain*> ` | Active transaction (sticky txn_id). |
| `brain[ctx=7]> ` | Sticky context = 7. |
| `brain*[ctx=7]> ` | Both. |

`N` must parse as `u64`. A bare `\set context` or a non-numeric value
yields `unknown meta command: …` without touching the session.

There is **no** `\unset context` — clear the sticky context by setting
it to `0` if you actually want context 0, or quit and restart the
session if you want "no default". This is the only asymmetry between
`\set` and [`\unset`](unset.md).

---

## Output sample

```
brain> \set output wide
output = wide

brain> \set context 7
sticky context = 7

brain[ctx=7]> \set output json
output = json

brain[ctx=7]> \set output csv
unknown meta command: csv
```

---

## Examples

```bash
# JSON for one debugging session, then back to table
brain> \set output json
brain> recall "deploy" --top-k 3
brain> \set output table

# Pin every encode/recall to a project context
brain> \set context 12
brain[ctx=12]> encode "kickoff"            # → context 12
brain[ctx=12]> recall "kickoff"            # → searches context 12

# Persist instead — survives across `brain shell` invocations
brain> \config set output wide
brain> \config set sticky_context 12

# Per-line override still wins
brain[ctx=12]> recall "kickoff" --filter-context 4   # overrides sticky
brain[ctx=12]> encode "note" -o json                 # overrides session output
```

---

## See also

- [`unset.md`](unset.md) — clears the active txn (note: `\unset context` is not implemented)
- [`config.md`](config.md) — persisted equivalents (`\config set output …` mirrors live + saves to disk)
- [`timing.md`](timing.md) — the boolean cousin
- [`../output-formats.md`](../output-formats.md) — what each renderer produces
- [`../configuration.md`](../configuration.md) — file schema for `[settings]`
- One-shot equivalent: `brain config set output <FORMAT>` and `brain config set sticky_context <N>`.
