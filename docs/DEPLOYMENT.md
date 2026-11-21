# VPS deployment and operations

Deployment requires explicit user approval after the implementation and security audits.

For trusted local use, run `./scripts/start-local.sh` from the repository root. The launcher keeps its credentials, WhatsApp session, and API data under `data/local` by default and binds all three services to loopback. Use Docker on the deployment host for network and resource isolation.

## Starting capacity

- Ubuntu LTS with Docker Engine 28+ and Docker Compose 2.33.1+
- 2 vCPU and 4 GiB RAM as a starting single-worker size
- Approximately 40 GB available storage, with usage alerting
- Tailscale for administrative access where practical

Encoding speed depends on the VPS CPU and source media. Keep one encode worker until measurements support a change. VPS rental and bandwidth mean the system is self-hosted, not necessarily zero-cost.

## Services

- `api`: Rust service; no public host port.
- `whatsapp`: Node.js/Baileys sidecar; no public host port.
- `web`: optional loopback admin UI. Only `127.0.0.1:${WEB_PORT:-3000}` is published.
- Internal `processing` network for API requests.
- Internal `control` network for the sidecar listener and the web service, with no API attachment. The Docker host is trusted and can reach bridge container IPs.
- Separate `whatsapp-egress` network attached only to the sidecar.
- Separate `web-ingress` bridge attached only to the UI for loopback port publishing, with IP masquerading disabled.
- Separate persistent volumes for Rust data and Baileys authentication.

No public web server is required for the bot workflow. Do not expose internal artifact URLs or management endpoints to the internet.

## Configuration

Use a protected deployment secret store or a `0600` `.env` file. Copy `.env.example` to `.env` under `umask 077`. Generate each token independently with `openssl rand -hex 32` and set the values with a local editor or secret manager. Enter phone numbers in international format. Keep the live values out of command history and chat. Docker administrators can read container environment variables, so restrict access to the Docker socket.

| Variable | Purpose |
| --- | --- |
| `OWNER_WHATSAPP_NUMBER` | Optional with the UI. Leave empty to set the owner in the dashboard. For fresh headless setup, supply the single owner allowed to submit jobs. |
| `BOT_WHATSAPP_NUMBER` | Optional identity pin with UI control. Leave empty to enter the secondary number in the UI. Required for headless pairing and startup without control. |
| `WEB_ADMIN_TOKEN` | Independent admin secret with at least 32 printable non-space ASCII characters |
| `SIDECAR_CONTROL_TOKEN` | Independent web-to-sidecar bearer secret with at least 32 printable non-space ASCII characters |
| `WEB_PORT` | UI loopback host port, default 3000 |
| `WEB_PUBLIC_ORIGIN` | Exact browser origin, default `http://127.0.0.1:3000`. Set when changing port, hostname, or scheme. |
| `WEB_ALLOWED_HOSTS`, `WEB_COOKIE_SECURE` | Optional host allowlist and Secure cookies for HTTPS. Defaults derive the allowlist from the origin and local port 3000 names. |
| `VIDORA_CONTROL_SUBNET`, `VIDORA_CONTROL_SIDECAR_IP`, `VIDORA_CONTROL_WEB_IP` | Override the control bridge subnet and addresses together if defaults overlap another network. Defaults are `172.30.250.0/24`, `172.30.250.2`, and `172.30.250.3`. |
| `INTERNAL_API_TOKEN` | Random secret of at least 32 bytes shared by the API, sidecar, and optional web service |
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

## Local admin UI

On the approved host, run:

```sh
docker compose -f compose.yaml -f compose.ui.yaml up -d --build
```

Open `http://127.0.0.1:3000`, unlock with `WEB_ADMIN_TOKEN`, then set the owner number if prompted and enter the dedicated secondary bot number if it is unpinned. If removing an existing owner setting from `.env`, enter the same owner in the UI. The UI saves the owner once in the sidecar volume. Complete pairing on that phone only after explicit approval. Run `docker compose -f compose.yaml -f compose.ui.yaml ps` and send `/help` from the owner phone when connected. The sidecar can be unhealthy while unpaired. Start without `up --wait` until pairing is complete.

For remote access, use an SSH local forward such as `ssh -N -L 3000:127.0.0.1:3000 user@host`, or private Tailscale Serve forwarding to loopback. For HTTPS, set the public origin to the HTTPS origin and `WEB_COOKIE_SECURE=1`. Keep the host bind on `127.0.0.1`.

If the UI shows “Sidecar unreachable,” inspect service status and fixed sidecar event names. Check matching control tokens, the control bridge configuration, and whether the session was invalidated. `bot_number_required` and `owner_number_required` identify missing numbers in headless startup.
For a manual session reset, stop the sidecar, unlink the old session on the dedicated phone, and identify the exact Compose session volume. Back it up with encryption if needed, remove only that session volume after approval, then start one sidecar and pair again. Preserve the API media volume.

## Headless pairing fallback

Set `BOT_WHATSAPP_NUMBER` and provide `OWNER_WHATSAPP_NUMBER` if the owner was not saved through the UI. Proceed only after explicit approval:

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

- Stop both services for a filesystem-level backup. Use an approved encrypted backup destination for the complete sidecar session directory (including `owner.key` when present) and the API's `jobs.db` and any matching `jobs.db-wal` and `jobs.db-shm` files. Preserve directory ownership and mode.
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
