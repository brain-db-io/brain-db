# `\timing` (REPL meta)

Toggle a per-command wall-clock footer. When on, each command prints
`(N ms)` to stdout after its result. Use it for quick latency spot-
checks without leaving the shell — for serious measurement reach for
`criterion` (`just bench <crate>`).

**REPL only.** In one-shot mode, time the call from your OS shell
(`time brain recall …`) — there is no equivalent flag on the binary.

---

## Synopsis

```
\timing on
\timing off
```

Any other argument (including `\timing true`, `\timing 1`) prints
`unknown meta command: <arg>` and leaves the setting unchanged. Only
the literal `on` / `off` tokens are accepted by `parse_meta`.

---

## Behavior

`\timing on` sets `session.timing = true`. `\timing off` clears it.
The footer is printed by the REPL loop after dispatch returns:

- Elapsed is measured around the whole verb dispatch, so it includes
  argument parsing, wire round-trip, server compute, response
  rendering — not just network.
- Only printed for **human formats**: `auto`, `table`, `wide`.
  Suppressed for `json`, `ndjson`, `yaml` — a stray `(12 ms)` line
  would corrupt structured output and break downstream `jq` / `yq`
  pipelines.
- Resolution is milliseconds (`Duration::as_millis`). Sub-millisecond
  ops render as `(0 ms)`.

To persist across sessions, use [`\config set timing true`](config.md)
— that writes `timing = true` into `[settings]` and also flips the
live session in one go.

---

## Output sample

```
brain> \timing on
timing = true

brain> recall "auth rewrite" --top-k 3 --include-text
#1  s2/m1/v1  episodic  ctx=7  sal=0.700  score=0.0164
    Alice merged the auth-rewrite branch
...
3 results
(12 ms)

brain> recall "auth" -o json
{"op":"recall","result":[...]}

brain> \timing off
timing = false
```

The second `recall` skipped the footer because `-o json` is a
structured format.

---

## Examples

```bash
# Spot-check encode latency for an afternoon
brain> \timing on
brain> encode "note 1"
brain> encode "note 2"

# Persist the toggle — survives `quit` + `brain shell` later
brain> \config set timing true

# Switch to JSON for a pipeline; the timing footer auto-suppresses
brain> \set output ndjson
brain> subscribe --collect 100

# Quick before/after on `--include-text`
brain> \timing on
brain> recall "outage"                       # ids only — cheap
brain> recall "outage" --include-text        # also fetches bodies
# Compare the two (N ms) tails.
```

---

## See also

- [`set.md`](set.md) — sibling toggles that don't have a dedicated verb
- [`config.md`](config.md) — `\config set timing true` to persist
- [`info.md`](info.md) — shows current `timing` state under the Session section
- [`../output-formats.md`](../output-formats.md) — which formats suppress the footer
- One-shot equivalent: `time brain <verb> …` from your OS shell.
