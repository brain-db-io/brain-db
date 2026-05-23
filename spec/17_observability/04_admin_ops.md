# 17.04 Admin Operations

The operational commands available to operators.

## 1. The admin API

Admin operations use the same wire protocol as regular operations, with admin opcodes. Authentication required (admin token, distinct from agent tokens).

The `brain-cli` tool provides convenient access:

```bash
brain-cli stats
brain-cli rebuild-ann <shard-id>
brain-cli snapshot create my-snapshot
```

Internally these are admin API calls.

## 2. The categories

- **Status**: stats, health, info.
- **Maintenance**: rebuild-ann, gc, vacuum.
- **Snapshots**: create, list, restore.
- **Workers**: list, stop, start, run-now.
- **Configuration**: reload, get, set.
- **Audit**: query, export.
- **Diagnostics**: profile, debug-snapshot.

## 3. Status operations

### `stats`

```bash
brain-cli stats [--shard SHARD]
```

Returns summary stats:

```json
{
  "shard": "<uuid>",
  "memory_count": 1234567,
  "tombstone_count": 23456,
  "arena_used_gb": 45.6,
  "wal_segments": 12,
  "uptime_seconds": 86400
}
```

### `health`

```bash
brain-cli health
```

Returns overall health:

```json
{
  "status": "healthy",
  "shards": {
    "<uuid_1>": "healthy",
    "<uuid_2>": "healthy"
  },
  "version": "1.0.0"
}
```

### `info`

```bash
brain-cli info
```

Returns version, build info, configuration summary.

## 4. Maintenance operations

### `rebuild-ann`

```bash
brain-cli rebuild-ann <shard-id>
```

Triggers immediate HNSW rebuild on the shard. Async; returns immediately. Status visible via:

```bash
brain-cli rebuild-ann-status <shard-id>
```

### `gc`

```bash
brain-cli gc [--shard SHARD] [--type idempotency|wal|slots]
```

Triggers immediate garbage collection of the specified type:

- `idempotency`: prune expired idempotency entries.
- `wal`: delete eligible WAL segments.
- `slots`: reclaim eligible slots.

### `vacuum`

```bash
brain-cli vacuum [--shard SHARD]
```

Compacts the metadata store (redb). May take minutes for large stores.

## 5. Snapshot operations

### `snapshot create`

```bash
brain-cli snapshot create <name> [--shard SHARD]
```

Creates a consistent snapshot. Brain:

1. Triggers a checkpoint.
2. Copies (or reflinks, if supported) the storage files.
3. Records the snapshot's metadata.

### `snapshot list`

```bash
brain-cli snapshot list
```

Lists snapshots:

```json
[
  {"name": "pre-upgrade-2026-05-07", "shard": "<uuid>", "size_gb": 12.3, "created_at": "..."},
  {"name": "scheduled-daily-001", "shard": "<uuid>", "size_gb": 12.3, "created_at": "..."}
]
```

### `snapshot restore`

```bash
brain-cli snapshot restore <name> [--shard SHARD]
```

Restores from a snapshot. **Destructive** — current data is lost. Requires confirmation:

```
brain-cli snapshot restore pre-upgrade-2026-05-07 --confirm
```

### `snapshot delete`

```bash
brain-cli snapshot delete <name>
```

Removes a snapshot.

## 6. Worker operations

### `worker list`

```bash
brain-cli worker list [--shard SHARD]
```

Returns worker status:

```json
[
  {"shard": "...", "name": "decay", "status": "running", "last_run": "...", "next_run": "..."},
  {"shard": "...", "name": "consolidation", "status": "running", "last_run": "...", "next_run": "..."}
]
```

### `worker stop` / `worker start`

```bash
brain-cli worker stop --name decay --shard <uuid>
brain-cli worker start --name decay --shard <uuid>
```

Pauses or resumes specific workers.

### `worker run-now`

```bash
brain-cli worker run-now --name decay --shard <uuid>
```

Triggers an immediate cycle.

## 7. Configuration operations

### `config reload`

```bash
brain-cli config reload
```

Reloads configuration from disk. Some settings reload live; others require restart.

### `config get`

```bash
brain-cli config get [--key KEY]
```

Returns current configuration. With `--key`, returns just that key:

```bash
brain-cli config get --key workers.decay.interval
# 1h
```

### `config set`

```bash
brain-cli config set --key workers.decay.interval --value 30m
```

Updates a setting. Live reload if supported; otherwise persisted for next restart.

Not all settings are runtime-tunable. Brain logs which take effect immediately vs which need restart.

## 8. Audit operations

### `audit query`

```bash
brain-cli audit query \
  --since "2026-05-01" \
  --until "2026-05-07" \
  --agent agent-001
```

Returns audit log entries matching the filter.

### `audit export`

```bash
brain-cli audit export --output /backup/audit-2026-05.jsonl
```

Exports audit logs to a file. For long-term archival.

## 9. Diagnostic operations

### `profile`

```bash
brain-cli profile --shard <uuid> --duration 30s --output /tmp/profile.pb
```

Captures a CPU profile of the shard's executor. Output is pprof-compatible.

### `debug-snapshot`

```bash
brain-cli debug-snapshot --shard <uuid> --output /tmp/debug.json
```

Captures a detailed runtime snapshot:

- Active tasks.
- Pending requests.
- In-memory state summary.
- Recent errors.
- Worker statuses.

For deep debugging.

## 10. Agent operations

### `agent list`

```bash
brain-cli agent list [--shard SHARD]
```

Lists agents on the shard.

### `agent stats`

```bash
brain-cli agent stats <agent-id>
```

Per-agent stats:

```json
{
  "agent_id": "...",
  "shard": "...",
  "memory_count": 12345,
  "context_count": 5,
  "oldest_memory_age": "30d"
}
```

### `agent delete`

```bash
brain-cli agent delete <agent-id> --confirm
```

Deletes all of an agent's data. Destructive. Confirm required.

## 11. Shard operations

### `shard list`

```bash
brain-cli shard list
```

Lists shards.

### `shard create` (rare)

```bash
brain-cli shard create --logical-id 16
```

Creates a new shard. Used during expansion.

### `shard delete` (rare)

```bash
brain-cli shard delete <shard-id> --confirm
```

Deletes a shard. All its data is gone. Used during decommission.

## 12. Authentication

Admin operations require an admin token:

```bash
export BRAIN_ADMIN_TOKEN="..."
brain-cli stats
```

Or:

```bash
brain-cli --token "..." stats
```

Tokens are configured at deployment. Multiple tokens supported (per-operator).

### 12.1 Scope-bound API keys

For agent-facing (non-admin) traffic, API keys are issued with an explicit scope claim bound to the key at issuance time. The server derives the connection's effective `(org_id, user_id, namespace_id, agent_id, permissions)` from the AUTH header rather than trusting any client-supplied `agent_id` on the wire:

```jsonc
{
  "key_id": "...",
  "org_id": "...",
  "user_id": "...",
  "namespace_id": "...",
  "agent_id": "...",
  "permissions": ["encode", "recall", "subscribe"]
}
```

This closes the agent-impersonation surface: an authenticated client at one `agent_id` cannot construct a request claiming a different `agent_id` and have the server honor it. Every shard-side handler reads the bound `agent_id` from the connection's session state, not from the request payload.

**v1.0 rollout posture: opt-in via env flag.** Existing deployments don't break — the default-permissive code path is preserved behind `BRAIN_AUTH_SCOPE_BINDING_REQUIRED=false`. Operators opt in by flipping the flag once their key-issuance pipeline emits scope claims.

**v1.1: default-deny.** The flag inverts; operators opt out explicitly if they have a reason to. This is the security target.

The key-issuance flow + the `api_keys` redb table live under `crates/brain-server/src/admin/api_key.rs`; operator UX for key rotation rides on the admin frame surface (see §9, Diagnostic operations).

## 13. The dry-run mode

For destructive operations, `--dry-run`:

```bash
brain-cli agent delete <agent-id> --dry-run
# Would delete: 12345 memories, 5 contexts, 67890 edges
```

Shows what would happen without doing it.

## 14. The "scriptable" output

```bash
brain-cli stats --output json
brain-cli stats --output yaml
brain-cli stats --output table
```

JSON for scripts, YAML for humans, table for terminal. Default: table.

## 15. Output piping

Brain-cli works in pipelines:

```bash
brain-cli agent list --output json | jq '.[].agent_id' | xargs -I{} brain-cli agent stats {}
```

Standard Unix tool composition.

## 16. The "background ops" tracking

Async operations (rebuild, restore, etc.) return a job ID:

```bash
$ brain-cli rebuild-ann <shard-id>
Job started: abc-123

$ brain-cli job status abc-123
{"id":"abc-123","status":"in_progress","progress":0.45,"started_at":"..."}
```

Job tracking lets operators check long-running operations.

## 16.5. Identity binding for admin keys

Admin operations authenticate with admin keys, which are separate from client agent keys. Each admin key carries explicit scope claims — which agents it may inspect, which shards it may rebuild, whether it may snapshot, whether it may restore. The server derives the operator's permissions from the key at AUTH time; admin commands do not accept an arbitrary `agent_id` or `shard_id` override that bypasses the claims.

This means an admin key issued for "inspect agent A only" cannot rebuild agent B's HNSW even if the CLI command syntactically allows it; the server rejects the operation at dispatch with an authorization error. Audit logs record the key's claims alongside the operation, so post-incident review can verify that an action was authorized at the moment it ran, not just that the operator had access at some point.

Key rotation, revocation, and claim updates flow through a separate admin operation (`ADMIN_KEY_*`); changes take effect on subsequent AUTH attempts.

## 17. The "audit of admin ops"

All admin operations are audit-logged:

```json
{
  "ts": "...",
  "actor": "operator-name",
  "operation": "snapshot_create",
  "params": {"name": "pre-upgrade-2026-05-07"},
  "result": "success"
}
```

This documents who did what, when. Important for compliance.

---

*Continue to [`05_runbooks.md`](05_runbooks.md) for runbooks.*
