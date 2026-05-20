# `brain config`

Inspect or mutate the persisted shell settings at
`~/.config/brain/config.toml`. Reach for this when you want a setting
to outlive the current invocation — `--output`, `BRAIN_SERVER`, and
the like override per-invocation, but `config set` is how you make a
preference stick across sessions and across shells.

```
brain config list
brain config get  <KEY>
brain config set  <KEY> <VALUE>
brain config path
brain config edit
```

The config file is XDG-honouring (`$XDG_CONFIG_HOME/brain/config.toml`
when set, otherwise `~/.config/brain/config.toml` on Linux,
`~/Library/Application Support/brain/config.toml` on macOS). It is
created on first run with permissions `0o600` and every write is
atomic — a sibling temp file is `chmod 0o600`'d, the new contents
serialised, then `rename(2)`'d over the live file. A `kill -9` mid
write leaves the previous file intact.

---

## Known keys

The schema is **closed by design**: unknown keys are rejected with a
Levenshtein "did you mean" hint rather than silently no-op'd. Adding
a key is a code change.

| Key | Type | Default | Notes |
|---|---|---|---|
| `output` | `"table"` \| `"json"` | unset (auto by TTY) | Default `--output`. See the gotcha below. |
| `timing` | bool | `false` | Show per-op wall time. Overridden by `\timing on/off`. |
| `sticky_context` | `u64` | unset | Default `--context` for `encode` / `recall`. Overridden by `--context`. |
| `server` | `host:port` | `127.0.0.1:9090` | Endpoint. Overridden by `--server`, `BRAIN_SERVER`. |

### Output gotcha: persisted ≠ runtime

The CLI's `--output` accepts seven values
(`auto` / `table` / `wide` / `json` / `ndjson` / `yaml` /
`jsonpath=<expr>`) — but the config file only persists two
(`table` / `json`). The remaining five must be set per invocation via
`-o`:

```bash
brain config set output yaml
# error: invalid value for output: 'yaml' is not one of: table, json

brain recall "auth" -o yaml       # works — per-invocation only
```

The reasoning: `auto` would be ambiguous (which surface is it autoing
for?), `jsonpath=` carries an expression that has no business being
persisted, and `wide` / `ndjson` are output knobs that change between
runs of the same command. The two persisted values are the ones users
toggle once-per-machine.

---

## `config list`

```
brain config list
```

Prints every known setting and its current value, one per line, with
the column-aligned key on the left. Missing values render as
`(unset)` — meaning "the in-process default applies." Stable
ordering: the same as the `KNOWN_KEYS` table above.

```
$ brain config list
output          (unset)
timing          false
sticky_context  7
server          127.0.0.1:9090
```

Pipe-friendly when you want a snapshot of what `brain` will use for
the next invocation. For machine consumption, run `brain config get`
in a loop — `list` is for humans.

---

## `config get <KEY>`

```
brain config get <KEY>
```

Print a single value, undecorated, to stdout. Script-friendly:
suitable for `$(brain config get server)`. Unknown key prints the
"did you mean" hint to stderr and exits 2; unset key prints
`(unset)` to stdout and exits 0.

```bash
SERVER=$(brain config get server)
if [ "$SERVER" = "(unset)" ]; then SERVER=127.0.0.1:9090; fi
```

---

## `config set <KEY> <VALUE>`

```
brain config set <KEY> <VALUE>
```

Validate, write the file, echo the new pair. Validation is per-key:

| Key | Accepted values |
|---|---|
| `output` | `table`, `json` |
| `timing` | `true` / `on` / `1`, `false` / `off` / `0` |
| `sticky_context` | any `u64` |
| `server` | any string containing `:` (light `host:port` shape check; full validation runs at connect) |

A rejected value exits 2 with a structured error:

```bash
$ brain config set output yaml
error: invalid value for output: 'yaml' is not one of: table, json

$ brain config set otput json
error: unknown setting key: otput. did you mean 'output'?
```

### Inside the REPL — `\config set` is dual-write

From the prompt, `\config set <KEY> <VALUE>` writes the file **and**
mutates the live session. The next command in the REPL sees the new
value without a reconnect. This is the mongosh model — there's no
useful distinction between "stash for next time" and "want it now."

```
brain> \config set timing on
timing = true                      # persisted + live
brain> encode "hello"
ok  s2/m12/v1  lsn=9
(3 ms)                             # next command shows timing
```

One-shot `brain config set` does **not** affect any running REPL;
the REPL holds its own session-state copy in memory.

---

## `config path`

```
brain config path
```

Print the absolute path the shell would read/write. Useful in
scripts (`vim "$(brain config path)"`) and when diagnosing
multi-user / XDG-override setups.

```bash
$ brain config path
/Users/alice/.config/brain/config.toml
```

The path is **always printed**, even when the file doesn't exist yet.
Combine with `[ -f "$(brain config path)" ]` for a presence probe.

---

## `config edit`

```
brain config edit
```

Open the config file in `$VISUAL`, falling back to `$EDITOR`,
falling back to `vi`. The exit code is the editor's exit code, so
quitting `vim` with `:cq` propagates as failure.

If the file doesn't exist yet, an empty (but valid) one is written
first — so the editor never opens on a phantom path. This mirrors
`git config --edit`'s first-run behaviour.

```bash
EDITOR=nano brain config edit
VISUAL=code brain config edit            # opens in VS Code
```

After the editor exits, the next `brain` invocation re-parses the
file from scratch; there's no daemon to reload. A malformed file
refuses to load on the next invocation with a `Parse` error
pointing at `brain config path`.

---

## Examples

```bash
# One-time setup on a fresh machine
brain config set server     prod.brain.internal:9090
brain config set output     json
brain config set timing     on

# Drop the sticky context that a previous session set
brain config set sticky_context 0

# Move to a new server for a single command without touching the file
brain --server staging.brain.internal:9090 recall "auth"

# Read a setting into a shell variable
TIMING=$(brain config get timing)

# Audit the effective config across a fleet
ssh node-{1..16} 'brain config list' | column -t
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `UnknownKey` | Key not in the closed schema. | Use one of `output`, `timing`, `sticky_context`, `server`. The error includes a "did you mean" suggestion. |
| `InvalidValue` | Value fails the per-key validator (see table above). | Re-issue with a valid value; the error names the accepted set. |
| `Parse` | The TOML file is malformed (likely hand-edited). | `brain config path` to find it; fix or delete to reset. |
| `Write` | Filesystem write failed (permissions, full disk). | Check `ls -la "$(brain config path)"`; ensure `0o600` and writable. |
| `NoConfigDir` | No `$XDG_CONFIG_HOME` and no `$HOME`. | Set one; the shell can't persist without a config dir. |

Exit code is `2` for validation / unknown-key errors, `1` for I/O
errors.

---

## See also

- [`agent.md`](agent.md) — agent CRUD (uses the same file).
- [`info.md`](info.md) — see all settings the live session ended up using.
- [`../configuration.md`](../configuration.md) — file schema reference.
- [`../repl-meta.md`](../repl-meta.md) — `\config` meta-commands (the dual-write surface).
- Spec: [`spec/13_sdk_design/01_principles.md`](../../../../spec/13_sdk_design/01_principles.md) — SDK ergonomic principles the CLI inherits.
