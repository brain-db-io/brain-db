# `brain reason`

Inference chain from an observation. Given a piece of evidence, expand
outward through the memory graph and return a ranked list of
`InferenceStep`s — what the substrate believes follows.

```
brain reason <OBSERVATION>
        [--depth <N>]
        [--confidence <FLOAT>]
        [--max-inferences <N>]
```

---

## Positional argument

| Arg | Meaning |
|---|---|
| `<OBSERVATION>` | Free-text evidence statement. Embedded server-side and fed to the inference engine. |

---

## Flags

### `--depth <N>`

Maximum reasoning depth (hops through the graph). Default `3`. Each hop
expands the frontier with the highest-scoring edges from the current
set of inferences.

### `--confidence <FLOAT>`

Lower bound on per-step confidence. Default `0.0` (no filter). Inferences
below this threshold are pruned before the next hop, so a higher
threshold both shrinks the result list AND tightens the search.

### `--max-inferences <N>`

Cap on returned inferences. Default `16`. The search returns once it
either hits `--depth` or has produced this many inferences.

---

## Output

### Table

```
#1  s2/m2/v1  Deduction      conf=0.85
    auth tokens use BLAKE3

#2  s2/m5/v1  Analogy        conf=0.62
    sessions invalidate on token rotation

2 inferences  ·  status=Complete
```

| Column | Meaning |
|---|---|
| `#N` | Inference rank. |
| `s2/m2/v1` | The memory id the inference rests on. |
| `Deduction`/`Analogy`/`Induction` | Inference kind. |
| `conf=…` | Per-inference confidence. |

The `status=` footer mirrors `plan`'s status discipline:

| Status | Meaning |
|---|---|
| `Complete` | The depth/inference budget wasn't reached; this is everything the reasoner found. |
| `BudgetExhausted` | Hit `--max-inferences`. |
| `DepthLimitReached` | Hit `--depth`. |
| `Cancelled` | Server-side cancellation. |

### JSON

```json
{ "op": "reason",
  "result": [
    { "memory_id": "0x...", "kind": "Deduction", "confidence": 0.85 },
    { "memory_id": "0x...", "kind": "Analogy",   "confidence": 0.62 }
  ] }
```

---

## Examples

```bash
# Default budgets
brain reason "the build is red"

# Deeper, tighter
brain reason "the build is red" --depth 5 --confidence 0.7 --max-inferences 32

# Pipe into recall to fetch the memory bodies for each inferred id
brain reason "deploy failed" -o json \
  | jq -r '.result[].memory_id' \
  | xargs -I{} brain recall "{}" --top-k 1 --include-text -o json
```

---

## Errors

| Code | Trigger | Fix |
|---|---|---|
| `InvalidArgument` | Empty observation; budget <= 0. | Re-issue with valid input. |
| `ShardUnavailable` | Target shard down. | Wait + retry. |

A `status` other than `Complete` is **not** an error — the command
exits `0` regardless.

---

## See also

- [`plan.md`](plan.md) — when you have a goal state as well as a start
- [`recall.md`](recall.md) — direct lookup
- Spec: [`spec/05_operations/03_read_pipeline.md`](../../../../spec/05_operations/03_read_pipeline.md)
