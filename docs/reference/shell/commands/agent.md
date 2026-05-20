# `brain agent`

Manage the named agents persisted in `~/.config/brain/config.toml`.
An *agent* is a stable UUIDv7 the substrate uses to scope every
encode, recall, edge, and dedup fingerprint — so "who you are this
session" determines what you can recall. Reach for this verb when
you need a stable identity across sessions, when you want multiple
identities on one machine, or when a teammate shares an id with you.

```
brain agent list
brain agent show         [<NAME>]
brain agent create       <NAME> [--note <TEXT>]
brain agent rename       <OLD>  <NEW>
brain agent delete       <NAME>
brain agent import       <NAME> <UUID> [--note <TEXT>]
brain agent set-default  <NAME>
```

Every write is atomic (`tempfile + rename`, preserving `0o600`).
First-run UX: if a bare `brain` is invoked with no flag, no env, and
no agents in the file, the resolver auto-mints one
(`agent-<8 hex chars of the UUID>`) and persists it as both
`default = true` and `active = true`. The `note:` line on stderr
tells you what happened.

---

## Resolution precedence

Each invocation picks exactly one agent. The cascade, highest to
lowest:

| # | Source | Wins when… |
|---|---|---|
| 1 | `--agent <NAME>` | The flag is set. Missing name → error. |
| 2 | `--agent-id <UUID>` | The flag is set. Raw id; no file lookup. |
| 3 | `BRAIN_AGENT=<NAME>` | The env var is non-empty. Missing name → error. |
| 4 | `BRAIN_AGENT_ID=<UUID>` | The env var is non-empty. Raw id. |
| 5 | `[agents.<x>] active = true` | The file has an `active` entry (set by `\agent use`). |
| 6 | `[agents.<x>] default = true` | The file has a `default` but no `active`. |
| 7 | Auto-mint + persist | No flag, no env, no agents in the file. First-run path. |
| 8 | Ephemeral (in-memory) | No config dir available (no `$HOME` / no `$XDG_CONFIG_HOME`). The only surviving ephemeral case. |

**Conflicts are hard errors**, not silent precedence:

- `--agent` AND `--agent-id` → `error: --agent and --agent-id are both set; pass only one`
- `BRAIN_AGENT` AND `BRAIN_AGENT_ID` → `error: BRAIN_AGENT and BRAIN_AGENT_ID are both set; unset one`

(Flag-vs-env is *not* a conflict — flags simply win.)

The `\info` meta-command and `brain info` both surface the resolved
source as a human-readable label (`--agent work`,
`config: active = work`, `auto-minted as agent-…`,
`ephemeral (no config file)`, …) so you can always tell which rung
of the cascade fired.

---

## `agent list`

```
brain agent list
```

Tabular dump of every entry in the file. The `*` column marks the
agent that the **current invocation** would bind to — i.e. the entry
the resolver picks given the active flags / env / config state. On a
fresh file the marker also indicates the auto-minted default.

### Output

```
   NAME             ID                                   CREATED              NOTE
*  work             019e3d1f-bd66-7890-a4bc-947ab6ca9c3e 2026-05-19T10:00:00Z prod work notebook
   demo             019e3d20-1234-7890-a4bc-947ab6ca9c3e 2026-05-19T11:30:00Z
   agent-019e3d1f   019e3d1f-... -                       2026-05-18T22:01:11Z auto-minted on first run
```

| Column | Meaning |
|---|---|
| `*` | This invocation would bind to this entry (`--agent` / env / `active` / `default`, in that order). |
| `NAME` | TOML key from `[agents.<name>]`. |
| `ID` | Full UUIDv7. |
| `CREATED` | RFC3339 UTC. |
| `NOTE` | Free-form, optional. |

Empty file prints `(no named agents — `brain agent create <name>` to add one)` and exits 0.

---

## `agent show [<NAME>]`

```
brain agent show <NAME>
brain agent show                  # → the entry the next connect would bind to
```

Print the full record for one agent. Omit `<NAME>` to see what the
resolver picks right now — useful for "wait, who am I about to
connect as?" before running a destructive command.

### Output

```
name       = work
id         = 019e3d1f-bd66-7890-a4bc-947ab6ca9c3e
created_at = 2026-05-19T10:00:00Z
note       = prod work notebook
```

When the resolved source is raw-id (`--agent-id` / `BRAIN_AGENT_ID`)
or ephemeral (no config dir), there's no `[agents.<name>]` entry to
show — output reads
`(no named agent — raw-id or ephemeral session)`. On a first-run bare
`brain` without flags the resolver auto-mints and **persists** an
`agent-<8hex>` entry instead of staying ephemeral, so this branch
only fires for explicit raw-id flags / env, or when no `$HOME` /
`$XDG_CONFIG_HOME` is available.

---

## `agent create <NAME> [--note <TEXT>]`

```
brain agent create <NAME>
brain agent create <NAME> --note "for shared infra notebook"
```

Mint a fresh UUIDv7 and write `[agents.<NAME>]` to the file. The new
entry's `created_at` is stamped to RFC3339 UTC. The new id is echoed
to stdout — chain into `clipboard` or `tee` if you want to share it.

```
$ brain agent create infra --note "shared infra notebook"
created agent 'infra' (019e3d20-9999-7890-a4bc-947ab6ca9c3e)
```

### First-create promotion

When the file is empty (or the resolver finds no other agents) the
very first create is auto-promoted to **both** `default = true` and
`active = true`. This satisfies the file invariant that
"non-empty implies a default" — saving an agent-less file with
neither flag set is a hard error (`MissingDefault`).

Subsequent creates leave existing `default` / `active` alone — pick
one explicitly via `\agent use` (live session) or
`brain agent set-default` (file-only).

### Errors

- Name already exists → `error: agent 'work' already exists`.
- Bad name (empty, whitespace, leading dot) → `error: agent name '<name>' is invalid: <reason>`.

---

## `agent rename <OLD> <NEW>`

```
brain agent rename <OLD> <NEW>
```

Atomic rename in one write transaction. The `id` and `created_at`
fields are preserved; only the TOML key changes. Refuses if `<NEW>`
already exists — to overwrite, delete then rename.

```
$ brain agent rename work prod
renamed 'work' → 'prod'
```

If the renamed agent was `active = true`, the active flag follows
the new name automatically; the connect banner reads
`via config: active = prod` after the rename.

---

## `agent delete <NAME>`

```
brain agent delete <NAME>
```

Remove the entry from the file. The on-substrate data (memories,
edges, statements) is unaffected — `delete` only forgets the local
*name* mapping, not the agent itself. To recover, `agent import
<NAME> <THE_OLD_UUID>` brings the binding back.

### Refuses to delete the live binding

If the current invocation is bound to `<NAME>` — i.e. `*` would
point at it in `agent list` — the delete is refused:

```
$ brain --agent work agent delete work
error: refusing to delete 'work' — the current invocation is bound to it
```

The protection avoids the foot-gun where deleting your own agent
mid-session leaves the next operation orphaned at the wire layer.
Drop the binding first (`unset BRAIN_AGENT`, omit `--agent`, or
`\agent use other` in the REPL), then delete.

---

## `agent import <NAME> <UUID> [--note <TEXT>]`

```
brain agent import <NAME> <UUID>
brain agent import <NAME> <UUID> --note "shared with bob"
```

Adopt an externally-supplied UUID under a local name. Used when a
teammate sends you an id and you want to bind to the same agent —
`brain` validates the UUID, stamps a fresh `created_at`, writes the
entry. The `<NAME>` is purely local; teammate's name for the same id
can differ.

```
$ brain agent import bob 019d2a44-aaaa-7890-a4bc-947ab6ca9c3e
imported agent 'bob' (019d2a44-aaaa-7890-a4bc-947ab6ca9c3e)
```

### First-import promotion

Same as `agent create`: importing into an empty file auto-promotes
to `default = true` + `active = true`, so the resolver has a target.
Subsequent imports don't touch existing flags.

### Errors

- UUID fails to parse → `error: agent id is not a valid uuid: <detail>`.
- Name collision → `error: agent '<name>' already exists`.

---

## `agent set-default <NAME>`

```
brain agent set-default <NAME>
```

Mark `<NAME>` as the file's `default = true`, clearing the flag from
whichever entry held it. The `default` is the **factory fallback**
the resolver picks when no `active` is set — useful when you keep a
long-lived "main" agent but session-hop with `\agent use` in the
REPL.

Crucially: this does **not** touch `active`. If `[agents.demo]
active = true` is present, `agent set-default work` leaves `demo`
as the next-connect target — `default` only fires at precedence
rung 6 (see the precedence table above).

```
$ brain agent set-default prod
default agent → prod
```

To change both flags at once, use the REPL's `\agent use <NAME>`,
which sets `active = true` (and load-time promotion ensures
`default` follows when there's nothing else there).

---

## Examples

```bash
# Multi-identity setup on a dev box
brain agent create work --note "real work"
brain agent create demo --note "screencasts"
brain agent create scratch
brain agent set-default work

# Share an agent with a colleague
brain agent show work | grep '^id' | awk '{print $3}' \
  | tee /dev/tty | pbcopy
# (paste in Slack — colleague runs `brain agent import work <uuid>`)

# Throwaway session against a known id
brain --agent-id 019e3d1f-bd66-7890-a4bc-947ab6ca9c3e recall "auth"

# Sticky cross-shell selection without touching the file
export BRAIN_AGENT=demo
brain                                # banner reads `via BRAIN_AGENT=demo`

# Audit who you'd connect as right now
brain agent show

# Reset the file (preserves on-substrate data, just drops the names)
rm "$(brain config path)"
brain                                # auto-mints a fresh agent
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `AgentExists` | `create` / `import` / `rename` with a name already in the file. | Pick a different name, or `agent delete <NAME>` first. |
| `AgentUnknown` | `show` / `rename` / `delete` / `set-default` against a missing name. The error includes a "did you mean" suggestion. | Use `brain agent list` to spell-check. |
| `AgentBadName` | Empty, whitespace, or shape-invalid name. | Pick `[a-zA-Z0-9_-]+` style. |
| `AgentBadId` | `import` with a non-UUID string. | Re-check the id; must be a UUID (any version; v7 is canonical). |
| `MissingDefault` | Save would leave a non-empty file with no `default`. (Tooling-side bug; the CLI promotes automatically on create/import.) | Hand-edit the file to add `default = true` to one entry. |
| `MultipleDefaults` / `MultipleActives` | Hand-edited file violates the at-most-one invariant. | Fix the file (`config edit`) so only one entry has each flag. |
| `Write` | Filesystem write failed. | Check `ls -la "$(brain config path)"`. |

Exit code is `2` for validation errors, `1` for I/O errors.

---

## See also

- [`config.md`](config.md) — settings stored in the same file.
- [`info.md`](info.md) — shows the resolved agent + source for the live session.
- [`../configuration.md`](../configuration.md) — file schema, migration of legacy `agent_id = "<ulid>"`.
- [`../repl-meta.md`](../repl-meta.md) — `\agent use` for live-session rebinding.
- [`../../../guides/shell/named-agents.md`](../../../guides/shell/named-agents.md) — task-oriented walkthrough.
- Spec: [`spec/13_sdk_design/03_connection.md`](../../../../spec/13_sdk_design/03_connection.md) — connection / auth model the agent id rides on.
