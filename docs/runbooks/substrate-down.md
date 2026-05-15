# RB-1: Substrate doesn't start

**Linked alert:** `BrainSubstrateDown`

## Symptoms

Process exits at startup; the metrics port doesn't answer; logs
show errors.

## Common causes

- Configuration error.
- Missing or corrupted data files.
- Port in use.
- Insufficient permissions on the data directory.

## Steps

1. **Check logs.** The substrate prints the first failure reason on
   stdout/stderr before exiting:
   ```bash
   journalctl -u brain-server --since "10 min ago"
   ```
   Or, if you redirected to a file:
   ```bash
   tail -200 /var/log/brain/brain.log
   ```

2. **Config error.** Symptom: `config error: ...` on stderr.
   Validate the TOML:
   ```bash
   brain-server --config /etc/brain/config.toml --help
   ```
   The substrate parses the config before printing help. Syntax
   issues, missing required fields, or unrecognised keys all surface
   here.

3. **Address already in use.** Symptom: `bind: Address already in
   use` on stderr.
   ```bash
   ss -tlnp 'sport = :9090'
   ```
   Either kill the conflicting process or change
   `[server] listen_addr` / `[server] metrics_addr` in the config.

4. **Data directory missing or wrong permissions.** Symptom:
   `ArenaOpenError` or `Permission denied`.
   ```bash
   ls -ld /var/lib/brain/data
   ```
   Verify owner / mode and that `[storage] data_dir` matches.

5. **WAL gap or recovery error.** Symptom: log shows
   `recover: ...` errors.
   **STOP.** Don't force-restart. Either:
   - Restore from snapshot: `brain-cli snapshot restore <last-known-good>`.
   - Capture state and escalate (don't overwrite the corrupted data
     directory).

## Escalate if

Issue isn't resolved by step 5. Capture:
- Full startup logs.
- `ls -la` of the data directory.
- Output of `brain-cli health --addr <metrics-addr>` (if reachable).
