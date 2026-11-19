# Implementation plan

Keep a separate commit for each completed implementation phase so its code and test evidence can be reviewed. Live linking and deployment still require approval. Do not commit, push, deploy, link WhatsApp, or install host-level prerequisites without explicit user approval.

## Phase 1 — Rust media-processing core

Build the smallest testable Rust library and CLI/worker entry point that:

- Generates server-owned job directories and accepts a bounded local input stream.
- Probes media with ffprobe and enforces the MVP input rules.
- Encodes the `hd29` profile without a shell.
- Segments, validates outputs, and applies the candidate clip cap with bounded retries.
- Cleans media after success and injected failures.
- Emits redacted structured job records.

Tests must include generated SDR fixtures, corrupt input, rotation, landscape/portrait, silent video, long input, excessive duration, invalid streams, timeout, and cleanup.

**Review evidence:** provide files changed, exact test output, sample redacted metadata, and known encoding limitations.

## Phase 2 — Rust internal API and durable jobs

Add:

- SQLite job state and idempotency by WhatsApp message ID.
- Authenticated streaming job creation, status, artifact, delivery, and terminal endpoints.
- One-worker scheduling and restart reconciliation.
- Bounded request/download bodies and authenticated artifact streaming.
- API integration tests for authorization, malformed input, duplicate jobs, path traversal, delivery retries, and cleanup.

**Review evidence:** inspect route authorization, stream bounds, path resolution, restart behavior, and test results.

## Phase 3 — Node.js WhatsApp sidecar

Add:

- Pinned Baileys dependency and Node.js 24 lockfile.
- Owner-only authorization, group/unknown-sender rejection, and message deduplication.
- Bounded media download and streaming to Rust.
- Status polling, ordered artifact delivery, per-artifact acknowledgement, and bounded retries.
- `/help` and `/status` responses without personal data in logs.
- A mock Rust service and mocked protocol boundary for automated tests. No live WhatsApp linking in this phase.

**Review evidence:** prove unauthorized and duplicate-message behavior with tests. Do not use a real WhatsApp account without separate approval.

## Phase 4 — Containers and security hardening

Add:

- Reproducible Dockerfiles and Compose configuration for `api` and `whatsapp`.
- Non-root users, private internal networking, no published internal ports, minimal writable volumes, and resource limits.
- FFmpeg network isolation and process limits.
- Environment validation, secret exclusion, health checks, graceful shutdown, and retention sweeps.
- Security tests and dependency/container audit reports.

**Review evidence:** run the complete automated suite and inspect the effective Compose configuration. Fix all high-severity findings or document an accepted, justified exception with the user.

## Phase 5 — Controlled secondary-account trial

Only after explicit approval to link a dedicated secondary number:

- Link one bot number using a controlled one-time pairing flow.
- Verify unknown senders and groups cannot use the bot.
- Run the manual media matrix and A/B quality experiments in `TESTING.md`.
- Record WhatsApp versions, device details, exact encoding metadata, results, and failures.
- Tune profiles only on recorded evidence; do not claim a bypass.

**Stop for audit:** review actual outputs, cleanup, logs, reconnection behavior, and quality evidence before VPS deployment.

## Phase 6 — VPS deployment

Only after explicit deployment approval:

- Provision or use the approved VPS and follow `DEPLOYMENT.md`.
- Confirm no public sidecar/API exposure and no real secrets in shell history or deployment logs.
- Perform backup/restore and single-instance checks for the Baileys session.
- Run smoke tests, verify cleanup and resource usage, and record the deployed source revision and dependency lockfiles.

## Phase handoff format

The implementing agent must return:

1. Phase number and requirement IDs addressed.
2. Actual files changed.
3. Commands run and exact outcomes.
4. Tests and fixtures added.
5. Deviations, unresolved issues, and security implications.
6. A statement of what was not tested.

A completed phase requires verified tests and recorded evidence. Deployment requires security review and the live trial results.
