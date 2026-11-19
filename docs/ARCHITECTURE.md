# Architecture

## Overview

```text
Owner WhatsApp account
        │
        ▼
Dedicated bot number
        │
        ▼
Node.js 24 + Baileys sidecar
  sender allowlist
  message deduplication
  bounded media download
  delivery and retries
        │ authenticated internal HTTP over private Docker network
        ▼
Rust service
  axum API → SQLite jobs → one Tokio worker → ffprobe/FFmpeg
                                      │
                                      ▼
                           validated MP4 output clips
                                      │
                                      ▼
                           automatic media cleanup
```

## Component responsibilities

### Rust service

Owns all media and job state. It must not contain WhatsApp protocol credentials.

Responsibilities:

- Authenticate every internal request with a shared bearer token.
- Enforce request-size, job-count, duration, and concurrency limits again even if the sidecar already checked them.
- Stream input into a random job directory; do not trust the supplied filename.
- Store durable job metadata in SQLite.
- Run ffprobe and FFmpeg with `tokio::process::Command` or an equivalent argument-array API.
- Validate outputs and serve them only through authenticated internal artifact endpoints.
- Track delivery acknowledgement and clean media on every terminal path.

Recommended baseline: stable Rust, `axum`, `tokio`, `sqlx` with SQLite, `tracing`, `uuid`, and `sha2`. Avoid additional frameworks unless a requirement cannot be met simply.

### Node.js sidecar

The only process with Baileys session material. It must not execute FFmpeg or expose a public HTTP server.

Responsibilities:

- Connect one dedicated WhatsApp number through Baileys.
- Derive authorization from authenticated sender metadata, not message text.
- Allow only the owner, reject groups and unknown senders, and deduplicate message IDs.
- Download media with byte and time limits and stream it to the Rust service.
- Poll job state, retrieve validated artifacts, deliver them in order, and acknowledge delivery.
- Keep Baileys authentication in a persistent directory with restrictive permissions.
- Treat reconnects and restarts as normal; never create a second job for an existing WhatsApp message ID.

Use Node.js 24, Baileys, and native `fetch`/streams where practical. Do not add a second web framework solely for outbound HTTP requests.

## Internal API

Exact field schemas may be implemented in code, but preserve these semantics:

| Endpoint | Purpose |
| --- | --- |
| `GET /health` | Minimal health without personal or session information |
| `POST /internal/v1/jobs` | Stream one media body with message ID, display name, and SHA-256 metadata |
| `GET /internal/v1/jobs/:id` | Return state, progress, validated artifact metadata, or a safe error |
| `GET /internal/v1/jobs/:id/artifacts/:index` | Stream one validated clip to the authenticated sidecar |
| `POST /internal/v1/jobs/:id/delivery` | Record per-artifact delivery acknowledgement idempotently |
| `POST /internal/v1/jobs/:id/terminal` | Record a terminal sidecar failure when delivery cannot continue |

Requirements:

- Every `/internal` route requires `Authorization: Bearer <INTERNAL_API_TOKEN>`.
- The Rust service binds to the private Compose network only; published host ports are not required for MVP.
- Artifact indices and job IDs are validated server-side. Paths are resolved from server-owned records, never raw request strings.
- Request bodies and downloads are streamed and bounded; buffering an entire 300 MiB video in memory is not acceptable.

## Job state

Use durable transitions equivalent to:

```text
RECEIVING → PROBING → ENCODING → READY → DELIVERING → DELIVERED → CLEANED
     │          │          │         │          │
     └──────────┴──────────┴─────────┴──────────┴→ FAILED → CLEANED
```

A restart reconciles interrupted jobs against actual files and delivery acknowledgements. Unknown or partial media is cleaned rather than assumed valid.

## Storage

- `data/jobs.db`: redacted durable job and idempotency metadata.
- `data/jobs/<random-uuid>/`: short-lived input, outputs, and processing metadata.
- `baileys-auth/`: persistent WhatsApp session, accessible only to the sidecar.

Do not place Baileys authentication inside the Rust media volume or container image.

## Deployment boundary

Docker Compose contains two services and two networks:

- `api`: Rust service, attached only to the `processing` network, with no public port.
- `whatsapp`: Node.js sidecar, attached to `processing` and `whatsapp-egress`, with no public port.
- `processing`: an internal Compose network with no external network route.
- `whatsapp-egress`: attached only to the sidecar so Baileys can reach WhatsApp.

FFmpeg therefore runs without an external network route while the sidecar retains the connectivity it needs. An optional Tailscale-only administration endpoint can be added later. Public inbound access is unnecessary for the WhatsApp bot workflow.
