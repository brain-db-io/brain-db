# `help` (REPL meta)

In-shell quick reference. Lists the cognitive verbs, knowledge-browsing
verbs, meta commands, and agent commands the REPL recognises — or, with
an argument, prints flag-level help for one verb.

**Unified across three entry points.** The same card output is produced
by all of:

| Form | Where |
|---|---|
| `help <verb>` (or `?` / `\?` / `\help`) | inside the REPL |
| `<verb> --help` / `<verb> -h` | inside the REPL |
| `brain <verb> --help` / `brain <verb> -h` | from your OS shell |

Same renderer, same content, same wrap — pick whichever feels natural.
The clap-generated help that older `<verb> --help` invocations produced
is gone; one source of truth lives in `repl::help::lookup`.

---

## Synopsis

```
help [VERB]
?    [VERB]
\?   [VERB]
\help [VERB]
```

All four spellings are equivalent. `help` (no backslash) is recognised
because users coming from psql / mongosh type it by reflex; `?` and
`\?` mirror psql; `\help` matches the rest of the backslash family.

---

## Behavior

- **Bare form** (`help`, `?`, `\?`, `\help`, `brain --help`) — prints
  the top-level cheat sheet: cognitive verbs, knowledge-browsing
  verbs, meta commands, persisted-config commands, agent commands,
  and a note on first-run agent auto-mint.
- **With argument** (`help encode`, `? recall`, `encode --help`,
  `brain encode -h`) — prints the verb-specific card with Flags,
  Sources, Notes, Example, and Reference sections. Case-insensitive;
  unknown verbs fall back to `no help for '<arg>'. Try \`help\` for
  the list.`.
- The text is built by `repl::help::lookup` and shipped inline in the
  binary — no network round-trip, no clap auto-help invocation.
  `<verb> --help` short-circuits before clap parsing so missing
  positional arguments (e.g. `recall --help`) don't block the card.
- `brain encode -- --help` opts out of the interception: the `--`
  escape tells the parser "everything after this is positional," so
  the literal text `--help` is encoded as the memory body.

Recognised argument values:

| Argument | Shows |
|---|---|
| (none), `help` | Top-level cheat sheet. |
| `encode`, `recall`, `plan`, `reason`, `forget`, `link`, `unlink`, `txn`, `subscribe` | One-paragraph blurb + flag synopsis for that verb. |
| `meta`, `\` | The full meta-command catalogue. |
| Anything else | `no help for '<arg>'…` |

---

## Output sample

```
brain> help
Cognitive verbs:
  encode <TEXT> [--context N] [--kind ...] [--salience F] [--allow-duplicate]
         [--edge KIND:ID]... [--request-id UUID] [--from-file PATH]
         [--from-stdin] [--vector CSV] [--wait-for-extraction]
  recall <QUERY> [--top-k N] [--confidence F] [--filter-context N]...
         [--filter-kind K]... [--include-text] [--include-graph]
  ...

Meta (session-only by default — `\config set` persists):
  quit | exit | \q                 exit the shell
  help [verb] | ? [verb] | \?       show help
  \set output auto|table|wide|json|ndjson|yaml
  \set context <N>                 session-only sticky --context
  \unset txn                       drop the active transaction
  \timing on|off                   per-op wall time
  \connect <host:port>             reconnect to a different server
  \info                            server / agent / connection / session diagnostic

Persisted (writes ~/.config/brain/config.toml):
  \config list                     show effective settings
  ...

Agents (named identities, see `brain agent --help`):
  \agent                           current binding (id + source)
  ...
```

```
brain> help encode
encode <TEXT> [--context N] [--kind episodic|semantic|consolidated]
              [--salience F] [--allow-duplicate] [--txn HEX]

Store text as a memory. Inherits the session's sticky --context and
active transaction when those flags are omitted. ENCODE happens against
the current agent (use `\agent` to see the binding).
...
```

---

## Examples

```bash
brain> help                   # the cheat sheet
brain> ?                      # same
brain> \?                     # same
brain> help recall            # flag summary for recall
brain> ? meta                 # the full meta catalogue
brain> help txn               # commit / abort id syntax
brain> help nonsense          # → no help for 'nonsense'. Try `help` for the list.
```

---

## History

The REPL persists line history alongside the agent config, so re-running
something you typed last session is one up-arrow away. Path resolution
follows the XDG spec:

```
$XDG_DATA_HOME/brain/history          # honoured first
~/.local/share/brain/history          # XDG default
~/.brain_history                      # fallback when neither exists
```

Loaded on REPL start, appended after each accepted line.

---

## See also

- [`set.md`](set.md), [`unset.md`](unset.md), [`timing.md`](timing.md),
  [`connect.md`](connect.md), [`info.md`](info.md) — the per-meta references
- [`config.md`](config.md), [`agent.md`](agent.md) — persistent settings + identities
- [`../commands.md`](../commands.md) — server-side verb index
- [`../repl-meta.md`](../repl-meta.md) — meta-command overview
- One-shot equivalent: `brain --help` and `brain <verb> --help` from your OS shell.
