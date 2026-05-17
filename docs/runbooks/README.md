# Brain runbooks

Step-by-step procedures for common operational situations.
Authoritative shape comes from spec §14/07; these files are the
operator-edit-friendly version that lives outside the spec.

| ID | Runbook | Linked alert |
|---|---|---|
| RB-1  | [Substrate doesn't start](substrate-down.md) | `BrainSubstrateDown` |
| RB-2  | [High latency on a shard](high-latency.md) | `BrainHighLatency` |
| RB-3  | [Memory pressure / OOM](memory-pressure.md) | `BrainHighMemoryPressure` |
| RB-4  | [Disk filling](disk-filling.md) | `BrainDiskFilling` (deferred) |
| RB-5  | [Worker stuck](worker-stuck.md) | `BrainWorkerStuck` |
| RB-6  | [HNSW recall degraded](recall-degraded.md) | `BrainRecallQualityDegraded` |
| RB-7  | [Recovery from corruption](corruption-recovery.md) | (chaos-detected) |
| RB-8  | [Substrate becoming unresponsive](unresponsive.md) | (composite) |
| RB-9  | [Mass FORGET aftermath](mass-forget.md) | `BrainHighTombstoneRatio` |
| RB-10 | [Network partition (v2)](network-partition.md) | (v2 only) |
| RB-11 | [Schema toggle (declare / migrate / revert)](schema-toggle.md) | (operator-triggered) |

## When to use a runbook

When an alert fires, the alert's `runbook` annotation links here.
Follow the steps in order. If the issue isn't resolved by the listed
steps, escalate per the runbook's "Escalate if" line.

## Validating a runbook

Per spec §14/07 §1, each runbook should be **executed at least once
against a Phase 13 chaos scenario** before release. The validation
matrix:

| Runbook | Chaos scenario |
|---|---|
| RB-1 substrate-down | `brain-storage::random_kill` (truncated WAL → recovery fails) |
| RB-3 memory-pressure | Phase 13 soak rig with sustained heavy encode |
| RB-5 worker-stuck | Manual: stop a worker via admin API, observe alert fire |
| RB-7 corruption-recovery | `brain-storage::bit_flip` |
| RB-9 mass-forget | Manual: encode 10K, FORGET --hard 7K, verify rebuild restores recall |

Operators validating a runbook should add a `Last validated:` line
at the top of the file when they exercise it.

## Editing

These files are not derived from the spec. Operators edit them as
procedures change. Spec changes go through the user; runbook
changes don't.
