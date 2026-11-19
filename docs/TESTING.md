# Testing and verification

## Evidence rules

- Record commands and actual results; never replace failures with assumed success.
- Use generated or explicitly authorized test media. Do not commit private videos.
- Record tool versions and exact encoding arguments for quality experiments.
- Automated tests verify implementation correctness, not WhatsApp visual quality.
- A local similarity metric does not establish the quality received by Status viewers.

## Rust checks

Once source exists, run at minimum:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Add integration coverage for stream bounds, ffprobe validation, argument construction, segmentation, output conformance, size-limit retries, timeouts, restart reconciliation, idempotency, authenticated routes, and cleanup.

## Node.js checks

Use the lockfile and project scripts once created:

```bash
npm ci --prefix sidecar --ignore-scripts
npm test --prefix sidecar
```

Test owner allowlisting, group rejection, unknown senders, unsupported messages, message deduplication, download limits, reconnect behavior, ordered delivery, per-artifact acknowledgement, and bounded retries without connecting a real account.

## Deployment checks

```bash
docker compose config
```

Also build both images, run the integration suite inside the Compose environment, inspect published ports, network attachments, and writable mounts, and verify the effective process user and resource limits. Confirm the Rust/API container has no external network route while the sidecar can reach WhatsApp. Save dependency and container scan results as audit evidence without treating a clean scan as a guarantee.

## Generated fixture matrix

Include at least:

| Fixture | Purpose |
| --- | --- |
| 1080×1920 SDR, 30 fps, stereo | Golden path |
| 3840×2160 landscape, 60 fps | Scaling, padding, frame-rate cap |
| Low-resolution portrait | Scale/pad and enlargement reporting |
| Rotated-phone video | Rotation metadata handling |
| Variable-frame-rate source | Timestamp and sync handling |
| Silent video | Optional audio mapping |
| HDR/HLG source | Explicit unsupported or tested tone-map behavior |
| Video longer than 29 seconds | Correct segmentation and ordering |
| Source longer than 10 minutes | Rejection before encode |
| Corrupt/truncated file | Safe probe failure |
| Additional subtitle/data streams | MVP stream-rule enforcement |
| Filename containing spaces and shell metacharacters | Path/command-injection resistance |

Do not commit large generated fixtures; generate them deterministically in test temp directories.

## Required failure tests

- Input exceeds declared and actual byte limits.
- ffprobe hangs or returns malformed metadata.
- FFmpeg hangs, crashes, exits nonzero, or produces an empty/oversized file.
- Output disappears before artifact download.
- Sidecar disconnects during upload, processing, and each artifact delivery.
- Duplicate WhatsApp message arrives after reconnect and after restart.
- Host/container restarts during every job state.
- Retention sweep races with active delivery.
- Unauthorized internal requests and artifact path traversal attempts.

Verify cleanup and redacted logs for each case.

## Manual secondary-account matrix

Only with explicit approval and a dedicated secondary number:

1. Confirm another account and a group cannot start a job.
2. Send each representative fixture as a document.
3. Record acknowledgment, processing time, returned parts, sizes, and failures.
4. Verify output metadata independently with ffprobe.
5. Run A/B/C posting paths from `ENCODING.md` using the same source and WhatsApp settings.
6. Record device model, OS, WhatsApp version, media upload quality, date, and observed result.
7. Test WhatsApp reconnection and VPS restart without duplicate delivery.
8. Verify input/output deletion and inspect logs for personal data or secrets.

## MVP sign-off

Sign-off requires all automated checks passing, all critical failure tests passing, no unresolved high-severity security findings, verified cleanup, and a documented manual quality result. If visual quality does not improve, report that honestly rather than weakening the test or claiming success.
