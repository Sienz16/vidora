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

## Local admin checks

Run `npm test --prefix sidecar`, `npm test --prefix web`, and `npm run build --prefix web` under Node.js 24. Use fake socket/control/API implementations and generated media. Automated tests must never link a real WhatsApp account or contain real numbers.

Verify the private control listener, owner-number rejection, pinned and unpinned UI pairing, and redaction across success, expiry, cancellation, and wrong identity. Check that headless pairing and startup without control report `bot_number_required` for an empty number, and that configured base startup still reports `sidecar_pairing_required` if unpaired.

With the UI overlay and fake upstreams, verify loopback publication and that the API and sidecar have no host ports. In the browser check unauthenticated rejection, failed-login throttling, the HttpOnly cookie, CSRF enforcement, and no third-party requests. Issue, cancel, and regenerate a fake pairing code, observe its countdown, and verify the code is absent from logs. Exercise job transitions and download the current artifact without acknowledging it. Confirm delivery and terminal routes are unavailable. Check keyboard navigation, mobile width, and that polling pauses while the tab is hidden.

## Deployment checks

```bash
docker compose config --quiet
docker build --target test .
docker build --target test ./sidecar
bash scripts/check-compose.sh
```

Run the smoke test on the Linux Docker host with cgroup v2. It builds both images in a disposable Compose project, verifies effective limits and isolation, processes generated media through the real Node HTTP client and Rust API, then checks delivery acknowledgement, deduplication, and disk cleanup. It probes the public WhatsApp web endpoint for network reachability without linking either fictional number.

Record `cargo audit`, `npm audit --prefix sidecar --omit=dev`, and vulnerability scan results for both runtime images. Tool-only test stages are separate from the production images.

## Generated fixture matrix

Include at least:

| Fixture | Purpose |
| --- | --- |
| 1080×1920 SDR, 30 fps, with audio | Golden path through both `hd29` and `status720`. Check the selected canvas, H.264 High / 4.0, `yuv420p`, AAC, duration, and byte cap. |
| 3840×2160 landscape, 60 fps | Scaling, padding, and frame-rate cap with both video profiles |
| Low-resolution portrait and 720×1280 portrait | Scale/pad and profile-specific enlargement reporting |
| Rotated-phone video | Rotation metadata handling |
| Variable-frame-rate source | Timestamp and sync handling |
| Silent video | Optional audio mapping |
| BT.2020 HLG and PQ sources | SDR BT.709 tone mapping, including segmentation past 29 seconds |
| HDR with incomplete color tags or incompatible Dolby Vision metadata | Rejection before encoding |
| Video longer than 29 seconds | Correct segmentation and ordering |
| Source longer than 10 minutes | Rejection before encode |
| Corrupt/truncated file | Safe probe failure |
| Additional subtitle/data streams | MVP stream-rule enforcement |
| Filename containing spaces and shell metacharacters | Path/command-injection resistance |

### Photo fixtures (`photo_hd`)

| Fixture | Purpose |
| --- | --- |
| 1080×1920 JPEG | Golden path (no upscaling, metadata stripped, 32 px JPEG thumbnail) |
| 4200×2800 PNG | Downscale-only to 3840×2560 preserving aspect ratio |
| RGBA PNG with alpha channel | Solid white background flattening to `yuvj420p` |
| Lossless WebP still (`webp_pipe`) | Extensionless `webp_pipe` decoding and JPEG conversion |
| EXIF Orientation=6 JPEG | Pixel rotation baked into output (`0` residual rotation) |
| HDR BT.2020 HLG/PQ vs SDR HEIC (`Content light level metadata`) | HDR tone-mapping vs SDR HEIC acceptance |
| Detailed 1600×1200 PNG with lowered output cap | Quality ladder (`q=2..10`) and `0.85×` scale-step retries, plus `ClipSize` rejection when cap is unreachable |
| Corrupt JPEG, 1×1 PNG, 12300×16 PNG, 7300×7300 PNG (>52 MP), 2-frame TIFF, >50 MiB BMP | Safe pre-encode rejection and cleanup |
| Photo filename with spaces and shell metacharacters | Path/command-injection resistance |

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
2. Send each representative video fixture as a document, and each photo fixture both as a document and as a direct HD image.
3. Record acknowledgment, processing time, returned parts/image dimensions and byte size, and failures.
4. Verify output metadata independently with ffprobe.
5. Run A–E posting paths from `ENCODING.md` using the same source, account, and WhatsApp settings. Compare both video profiles. Include WhatsApp Desktop/Web Status at default quality and with HD selected where available. Record unavailable HD options.
6. Record device model, OS, mobile WhatsApp version, desktop app version or Web client version, media upload quality, date, source and output ffprobe metadata, and observed text-edge and motion quality.
7. Test WhatsApp reconnection and VPS restart without duplicate delivery.
8. Verify input/output deletion and inspect logs for personal data or secrets.

## MVP sign-off

Sign-off requires all automated checks passing, all critical failure tests passing, no unresolved high-severity security findings, verified cleanup, and a documented manual quality result. If visual quality does not improve, report that honestly rather than weakening the test or claiming success.
