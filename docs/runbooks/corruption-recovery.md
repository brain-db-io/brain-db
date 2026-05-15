# RB-7: Recovery from corruption

**Linked alert:** (chaos-detected — the substrate refuses to start)

## Symptoms

- Substrate refuses to start: `recover: CrcMismatch` or
  `WAL gap detected` in logs.
- Recovery starts but fails partway through.

## STOP — preserve evidence

The instinct to force-restart with `--ignore-errors` flags is
**wrong**. Spec §15/07 §11 mandates fail-stop on corruption; the
substrate is correctly refusing to corrupt downstream state.

## Steps

1. **Backup the corrupted state.** Don't overwrite — engineering
   will want this:
   ```bash
   tar czf /tmp/corrupt-state-$(date +%s).tar.gz \
     /var/lib/brain/data
   ```

2. **Identify the last known good snapshot.**
   ```bash
   brain-cli snapshot list --addr <admin-addr>
   ```
   The output lists snapshot IDs + timestamps + sizes. Pick the
   most recent one whose timestamp predates the suspected
   corruption.

3. **Restore.**
   ```bash
   systemctl stop brain-server
   brain-cli snapshot restore --id <id> --confirm
   ```
   The restore replaces `data_dir` contents atomically (rename
   over the corrupted state); the backup from step 1 is your
   undo path.

4. **Restart and verify.**
   ```bash
   systemctl start brain-server
   curl -s http://<metrics-addr>/healthz   # → "ok\n"
   curl -s http://<metrics-addr>/metrics | grep brain_up
   ```
   Smoke a known cue through RECALL to confirm the substrate is
   serving.

5. **Investigate the root cause** (after recovery — never block
   recovery on root-cause analysis):
   - Hardware: `dmesg | grep -i "ecc\|memory\|disk"`. Bad RAM or
     dying disk surfaces here.
   - Substrate bug: file an issue with the `corrupt-state.tar.gz`
     attached. The chaos tests (Phase 13.3 — `bit_flip`,
     `random_kill`, `io_fault`) cover the known failure modes; an
     uncovered mode is a substrate bug.
   - Operational: a partial copy of `data_dir` from another host?
     A force-power-off without `sync`? Document and fix the
     procedure.

## Escalate if

No good snapshot exists. Engineering may be able to recover from
WAL fragments — keep the corrupted state intact for them.
