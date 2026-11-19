# VPS deployment and operations

Deployment requires explicit user approval after the implementation and security audits.

## Starting capacity

- Ubuntu LTS with Docker and Compose
- 2 vCPU and 4 GiB RAM as a starting single-worker size
- Approximately 40 GB available storage, with usage alerting
- Tailscale for administrative access where practical

Encoding speed depends on the VPS CPU and source media. Keep one encode worker until measurements support a change. VPS rental and bandwidth mean the system is self-hosted, not necessarily zero-cost.

## Services

- `api`: Rust service; no public host port.
- `whatsapp`: Node.js/Baileys sidecar; no public host port.
- Internal `processing` network for authenticated requests, attached to both services.
- Separate `whatsapp-egress` network attached only to the sidecar.
- Separate persistent volumes for Rust data and Baileys authentication.

No public web server is required for the bot workflow. Do not expose internal artifact URLs or management endpoints to the internet.

## Configuration

Use a protected deployment secret store or an excluded environment file with restrictive permissions. Required names are planned here; implementation must validate them at startup.

| Variable | Purpose |
| --- | --- |
| `OWNER_WHATSAPP_NUMBER` | Single owner allowed to submit jobs; store only in protected deployment configuration |
| `BOT_WHATSAPP_NUMBER` | Dedicated secondary number used for controlled pairing |
| `INTERNAL_API_TOKEN` | Random secret of at least 32 bytes shared only by the two services |
| `RUST_API_URL` | Sidecar-only internal Rust base URL |
| `DATA_DIR` | Rust job database and temporary media root |
| `BAILEYS_AUTH_DIR` | Persistent sidecar session directory |
| `MAX_INPUT_BYTES` | Default `314572800` |
| `JOB_TIMEOUT_SECONDS` | Bounded total processing time; choose from measured tests |
| `MEDIA_RETENTION_MINUTES` | Default `15`, with failure sweep bounded to 30 minutes |

Never commit real values. Never reuse the same Baileys session concurrently on development and production.

## Pre-deployment checklist

- All phase audits and automated tests pass.
- Effective Compose configuration has no published internal ports.
- Containers run as non-root with resource limits and minimal writable mounts.
- Rust/API has no external network route; only the sidecar is attached to WhatsApp egress, and FFmpeg is restricted to local file processing.
- Secret and media exclusions are verified.
- Dependency and container findings have been reviewed.
- Media cleanup, restart reconciliation, log redaction, and disk alerts are tested.
- A rollback procedure exists that does not start two sidecars with the same session.

## Controlled linking

Only after explicit approval:

1. Deploy without connecting the owner's primary account.
2. Start one sidecar and follow its one-time pairing procedure for the dedicated bot number.
3. Protect pairing output and do not retain it in persistent logs.
4. Verify health without exposing phone numbers or session details.
5. Run the secondary-account smoke and authorization tests.

Do not run a second sidecar during linking, restart, rollback, or restore.

## Backup and restore

- Back up the SQLite metadata and Baileys authentication only if operationally necessary, using encrypted backups.
- Do not back up temporary input/output media.
- Stop the sidecar before restoring its session.
- Restore into one instance only, then verify connection and message idempotency.
- Treat Baileys session backups as credentials with equivalent sensitivity.

## Operations

- Monitor disk usage, job failures, encode duration, memory, container restarts, and WhatsApp disconnect state without logging personal data.
- Apply reviewed dependency, FFmpeg, and base-image updates through a tested deployment process.
- Keep the exact source revision and lockfiles used for each deployment.
- On suspected session compromise, stop the sidecar, unlink the bot device from WhatsApp, rotate internal secrets, preserve relevant redacted logs, and relink only after review.
