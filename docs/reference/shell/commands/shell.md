# `brain shell`

Drop into the interactive REPL. Exactly equivalent to running
`brain` with no subcommand — there's no difference at runtime. The
explicit verb exists so shell scripts can be unambiguous about
intent ("this line is supposed to open a REPL, not run a default
verb"), and so completion / docs have a name to attach to.

```
brain shell
```

Reach for the explicit `shell` when:

- A script conditionally drops into an interactive prompt and you
  want the line to read `brain shell` instead of `brain`.
- You're documenting an example and the bare `brain` looks like a
  placeholder.
- You want a fail-loud invocation: a typo in a subcommand position
  (`brain shel`) errors at parse time rather than silently entering
  the REPL.

---

## Global flags seed the session

Every global flag still applies — `shell` doesn't strip them. They
become the **session defaults**:

```bash
brain shell \
  --server prod.brain.internal:9090 \
  --agent  prod \
  --output json \
  --color  always \
  --hyperlinks never \
  --timeout 60
```

The session starts with that server, that agent, JSON output, ANSI
always on, no OSC 8 hyperlinks, 60s timeout. Inside the REPL,
`\set output table` / `\timing on` / `\agent use <other>` mutate
the defaults further — see [`../repl-meta.md`](../repl-meta.md).

| Global | Effect inside the REPL |
|---|---|
| `--server` | Endpoint for the initial connect. `\connect <host:port>` re-targets later. |
| `--agent` / `--agent-id` | Bound agent for the session. `\agent use <NAME>` rebinds. |
| `--output` / `-o` | Default output format. `\set output …` mutates. |
| `--color` | ANSI policy. No in-REPL toggle (per-session only). |
| `--hyperlinks` | OSC 8 policy. No in-REPL toggle. |
| `--timeout` | Per-op timeout. No in-REPL toggle. |
| `--token` | Reserved for v2 auth. Ignored in v1. |

---

## Examples

```bash
# Identical to bare `brain`
brain shell

# Open the REPL against a remote server as a named agent
brain shell --server prod.brain.internal:9090 --agent prod

# Script that drops into the REPL when stdin is a TTY
if [ -t 0 ]; then
  brain shell --agent demo
else
  brain encode --from-stdin --agent demo
fi

# CI smoke that asserts the binary at least connects and exits
echo "quit" | brain shell --server "$BRAIN_TEST_ADDR"
```

---

## See also

- [`../repl-meta.md`](../repl-meta.md) — backslash meta-commands once you're inside.
- [`config.md`](config.md) / [`agent.md`](agent.md) — per-session vs persistent settings.
- [`info.md`](info.md) — diagnostic card for the current session.
- [`../configuration.md`](../configuration.md) — config file + history file paths.
