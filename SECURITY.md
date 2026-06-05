# Security policy

## Supported versions

| Version | Supported |
|---|---|
| 1.0.x | ✓ |
| < 1.0 | ✗ (pre-release; upgrade) |

Security fixes target the latest `1.x` minor release. We
backport to the previous minor release for 90 days after a
new minor lands.

## Reporting a vulnerability

**Do not** open a public GitHub issue for security reports.

Email security disclosures to the maintainer at the address
listed on the GitHub profile, or open a private security
advisory via GitHub's
[Security Advisories](https://github.com/nirajgeorgian/brain/security/advisories)
tab. Include:

- A description of the issue + impact.
- A reproducer (proof of concept, exploit script, or minimal
  test case).
- Affected version(s).
- Any suggested mitigation.

We aim to acknowledge within **3 business days** and ship a
fix within **30 days** for confirmed issues. Coordinated
disclosure is appreciated; we'll work with you on an embargo
window.

## Threat model

Brain runs **untrusted wire input** through:

- The TCP / WebSocket / SSE transport layer.
- The CBOR-decoded request bodies.
- The schema DSL parser (when a schema is uploaded).
- The LLM extractor (when configured) — prompts are derived
  from memory text, which is itself untrusted.

The substrate assumes:
- **Trusted disk** (no malicious filesystem behind the data
  directory).
- **Trusted operator** (the admin endpoint has authority over
  the deployment).
- **TLS in transit** when deployed outside a trust boundary.

Threats we model:

- Malformed wire frames (CRC + CBOR decode reject).
- WAL corruption (CRC32C detects; recovery refuses corrupt
  records).
- Idempotency-key collisions (`RequestId` UUID v7; spec §02/03).
- LLM prompt injection (extractor sandboxing per
  [`spec/11_extractors/`](spec/11_extractors/00_purpose.md) + cost budget
  caps).
- Out-of-bounds resource use (per-shard quotas, group commit
  caps, bounded queues with overflow metrics).

Threats out of scope:

- A malicious operator with shell access to the data
  directory.
- Side-channel attacks against the host (CPU, RAM, disk).
- Adversarial training data poisoning of the embedding model
  (we ship a pinned model fingerprint; rotation is
  operator-driven).

## Public CVE history

None yet. v1.0.0 is the first release.
