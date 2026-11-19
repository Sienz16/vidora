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

The API uses `axum`, `tokio`, `rusqlite` with bundled SQLite, `uuid`, and `sha2`. The short database operations run on one worker and large bodies stream through Tokio.

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

The sidecar checks the direct chat's authenticated phone JID or its Baileys-provided LID mapping. It accepts video documents with a declared length and plaintext SHA-256. It downloads from verified WhatsApp media hosts over TLS, checks the media MAC, and streams plaintext to the API, which checks the length and hash before processing. The sidecar prepares each document before reserving its send, waits for the WhatsApp server acknowledgement, and retries the internal acknowledgement independently. A missing server acknowledgement leaves the send uncertain and prevents a blind replay. A server acknowledgement does not establish playback or a read receipt.

The sidecar keeps authentication and Signal keys in its own SQLite database. An OS-backed SQLite lock prevents a second process from opening the same session, including during pairing. Session data and media remain on separate storage.

## Internal API

Exact field schemas may be implemented in code, but preserve these semantics:

| Endpoint | Purpose |
| --- | --- |
| `GET /health` | Minimal health without personal or session information |
| `POST /internal/v1/jobs` | Stream one media body with `Content-Length`, `X-Message-Id`, `X-Content-Sha256` (lowercase hex), and optional `X-Display-Name` |
| `GET /internal/v1/jobs` | Read the active or latest job, or look up a specific message using `X-Message-Id` |
| `GET /internal/v1/jobs/:id` | Return state, progress, validated artifact metadata, or a safe error |
| `GET /internal/v1/jobs/:id/artifacts/:index` | Stream one validated clip to the authenticated sidecar |
| `POST /internal/v1/jobs/:id/delivery` | Begin a send or record its acknowledgement with `{"action":"begin","index":0}` or `{"action":"ack","index":0}` |
| `POST /internal/v1/jobs/:id/terminal` | Record a terminal sidecar failure when delivery cannot continue |

Requirements:

- Every `/internal` route requires `Authorization: Bearer <INTERNAL_API_TOKEN>`.
- The Rust service binds to the private Compose network only; published host ports are not required for MVP.
- Artifact indices and job IDs are validated server-side. Paths are resolved from server-owned records, never raw request strings.
- Request bodies and downloads are streamed and bounded; buffering an entire 300 MiB video in memory is not acceptable.
- Job creation returns 202 for a new upload, 200 for a duplicate, and 409 when another job is active. The display name is validated and discarded. SQLite retains a SHA-256 digest of the message ID for idempotency.
- Artifact indices start at zero. The API serves only the next unacknowledged artifact. A durable `begin` prevents a second send attempt after an ambiguous crash or disconnect. Transient preparation failures can be retried before `begin`, and `ack` can be retried afterwards.

## Job state

Use durable transitions equivalent to:

```text
RECEIVING → PROBING → ENCODING → READY → DELIVERING → DELIVERED → CLEANED
     │          │          │         │          │
     └──────────┴──────────┴─────────┴──────────┴→ FAILED → CLEANED
```

The API uses one `processing` state for probing and encoding. A restart discards partial uploads and processing directories. An interrupted encode fails, and a ready or partially delivered job resumes if its validated files are still present. An in-flight send with an unknown outcome fails with `delivery_uncertain`. Graceful API shutdown cancels active work and removes its media. Completed and failed jobs retain minimal deduplication metadata. Uncollected ready clips expire after 20 minutes, with a sweep every 30 seconds while the service runs.

## Storage

- `data/jobs.db`: redacted durable job and idempotency metadata.
- `data/work/`: short-lived uploads and validated processing workspace.
- `data/media/<random-uuid>/`: short-lived validated outputs. Detailed redacted encoding metadata stays in SQLite until terminal cleanup.
- `baileys-auth/`: persistent WhatsApp session, accessible only to the sidecar.

Do not place Baileys authentication inside the Rust media volume or container image.

## Deployment boundary

Docker Compose contains two services and two networks:

- `api`: Rust service, attached only to the `processing` network, with no public port.
- `whatsapp`: Node.js sidecar, attached to `processing` and `whatsapp-egress`, with no public port.
- `processing`: an internal Compose network with no external network route.
- `whatsapp-egress`: attached only to the sidecar so Baileys can reach WhatsApp.

FFmpeg therefore runs without an external network route while the sidecar retains the connectivity it needs. An optional Tailscale-only administration endpoint can be added later. Public inbound access is unnecessary for the WhatsApp bot workflow.
