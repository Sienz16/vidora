# VPS deployment and operations

Deployment requires explicit user approval after the implementation and security audits.

## Starting capacity

- Ubuntu LTS with Docker Engine 28+ and Docker Compose 2.33.1+
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

Use a protected deployment secret store or a `0600` `.env` file. Copy `.env.example` to `.env` under `umask 077`. Generate the token with `openssl rand -hex 32` and set the three values with a local editor or secret manager. Enter phone numbers in international format. Keep the live values out of command history and chat. Docker administrators can read container environment variables, so restrict access to the Docker socket.

| Variable | Purpose |
| --- | --- |
| `OWNER_WHATSAPP_NUMBER` | Single owner allowed to submit jobs; store only in protected deployment configuration |
| `BOT_WHATSAPP_NUMBER` | Dedicated secondary number used for controlled pairing |
| `INTERNAL_API_TOKEN` | Random secret of at least 32 bytes shared only by the two services |
| `RUST_API_URL` | Set by Compose to `http://api:8080` |
| `DATA_DIR` | API volume at `/var/lib/vidora` |
| `BAILEYS_AUTH_DIR` | Sidecar-only session volume at `/var/lib/vidora-auth` |

The API enforces a 300 MiB upload cap, a 90-second upload deadline, a 20-minute processing deadline, and a 20-minute expiry for uncollected output. Delivery or failure triggers cleanup immediately. Startup reconciles leftovers from an unclean shutdown.

## Pre-deployment checklist

- All phase audits and automated tests pass.
- `docker compose config --quiet` validates the configuration. Avoid printing the effective environment because it includes the token and account numbers.
- `bash scripts/check-compose.sh` passes in an isolated Compose project using generated media.
- Containers run as non-root with resource limits and minimal writable mounts.
- Rust/API has no external network route; only the sidecar is attached to WhatsApp egress, and FFmpeg is restricted to local file processing.
- Secret and media exclusions are verified.
- Dependency and container findings have been reviewed.
- Media cleanup, restart reconciliation, log redaction, and disk alerts are tested.
- A rollback procedure exists that does not start two sidecars with the same session.

## Controlled linking

Only after explicit approval:

On the approved trial host, run these commands from the repository root using a private interactive terminal with recording disabled.

```sh
docker compose up -d --build --wait api
docker compose build whatsapp
docker compose stop whatsapp
docker compose -f compose.yaml -f compose.pairing.yaml run --rm --no-deps whatsapp
docker compose up -d --wait --wait-timeout 120
docker compose ps
```

Use **Linked devices → Link a device → Link with phone number instead** on the dedicated secondary phone and enter the code shown by the one-off container. The pairing override disables container logging. Once both services are healthy, message the bot with `/help` from the owner account, then run the [secondary-account verification matrix](TESTING.md).

Do not run a second sidecar during linking, restart, rollback, or restore.

## Backup and restore

- Stop both services for a filesystem-level backup. Use an approved encrypted backup destination for the sidecar session files and the API's `jobs.db` and any matching `jobs.db-wal` and `jobs.db-shm` files. Preserve directory ownership and mode.
- Do not back up temporary input/output media.
- Stop the sidecar before restoring its session.
- Restore into one instance only, then verify connection and message idempotency.
- Treat Baileys session backups as credentials with equivalent sensitivity.
- Verify a restore on the approved host with the original sidecar stopped. Confirm the restored sidecar is healthy and the API still recognizes previously submitted test message IDs.

## Operations

- Monitor disk usage, job failures, encode duration, memory, container restarts, and WhatsApp disconnect state without logging personal data.
- Apply reviewed dependency, FFmpeg, and base-image updates through a tested deployment process.
- Keep the exact source revision and lockfiles used for each deployment.
- For rollback, stop the sidecar, check out the last verified revision, and rebuild the stack. Keep the current session volume and restore its matching backup only if the prior revision cannot use it.
- On suspected session compromise, stop the sidecar, unlink the bot device from WhatsApp, rotate internal secrets, preserve relevant redacted logs, and relink only after review.
