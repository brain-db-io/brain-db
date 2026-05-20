# `brain plan`

Stepwise causal/temporal path between two states. The substrate searches
the memory graph (vector similarity + causal edges) for a chain of
memories that walks from the `FROM` description toward the `TO`
description, returning the steps and a status describing how the search
terminated.

```
brain plan <FROM> <TO>
        [--max-steps <N>]
        [--max-wall-time-ms <N>]
```

---

## Positional arguments

| Arg | Meaning |
|---|---|
| `<FROM>` | Free-text description of the starting state. Embedded server-side. |
| `<TO>` | Free-text description of the goal state. Embedded server-side. |

Both arguments are `ByText` inputs; the wire op also supports
`ByMemoryId` and `ByVector`, but the shell exposes only the text form
(the others compose better from the SDK).

---

## Flags

### `--max-steps <N>`

Hard cap on plan length. Default `10`. The search aborts with
`BudgetExhausted` once N steps are queued.

### `--max-wall-time-ms <N>`

Wall-clock budget for the search. Default `5000` (5 s). Aborts with
`BudgetExhausted` on timeout.

The shell pins the third internal budget — `max_branches_explored` —
at `256`. Override that, if you need to, via the Rust SDK directly.

---

## Output

### Table

```
#1  s2/m1/v1  Causal       conf=0.8  est_to_goal=0.42
    deploy started

#2  s2/m2/v1  Similarity   conf=0.6  est_to_goal=0.15
    auth tokens migrated to BLAKE3

3 steps  ·  status=GoalReached
```

| Column | Meaning |
|---|---|
| `#N` | Step number. |
| `s2/m1/v1` | Short `MemoryId` of the memory selected for this step. |
| `Causal`/`Similarity`/`Temporal` | Edge kind that led from the previous step. |
| `conf=…` | Per-step confidence the planner assigned. |
| `est_to_goal=…` | Remaining distance estimate (lower = closer to goal). |

The `status=` footer surfaces the terminating condition **loudly** so a
partial plan isn't mistaken for a complete one.

| Status | Meaning |
|---|---|
| `GoalReached` | The goal was reached — the plan is complete. |
| `BudgetExhausted` | Hit `--max-steps` or `--max-wall-time-ms`. The returned steps are valid but partial. |
| `NoPathFound` | Search terminated without finding a path. |
| `Timeout` | Server-side timeout (per spec §09/04 §3). |

### JSON

```json
{ "op": "plan",
  "result": {
    "steps": [
      { "memory_id": "0x...", "edge_kind": "Causal",
        "confidence": 0.8, "estimated_distance_to_goal": 0.42 }
    ],
    "status": "GoalReached"
  } }
```

---

## Examples

```bash
# Default budgets
brain plan "deploy started" "deploy succeeded"

# Deeper search
brain plan "outage detected" "service restored" \
  --max-steps 50 --max-wall-time-ms 30000

# Run, then fail fast on non-GoalReached
brain plan "$FROM" "$TO" -o json \
  | jq -e '.result.status == "GoalReached"' >/dev/null \
  || { echo "partial plan"; exit 1; }
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `InvalidArgument` | Empty `<FROM>` or `<TO>`; budget <= 0. | Re-issue with valid input. |
| `ShardUnavailable` | Target shard down. | Wait + retry. |

Non-`GoalReached` outcomes are **not** errors — they're surfaced in the
`status` field. The command exits `0` either way; check `status` in
scripts.

---

## See also

- [`reason.md`](reason.md) — inference chain (no goal state, just
  forward inference)
- [`recall.md`](recall.md) — direct lookup, no path-finding
- [`link.md`](link.md) — adding the edges the planner walks
- Spec: [`spec/09_cognitive_operations/04_plan.md`](../../../../spec/09_cognitive_operations/04_plan.md)
