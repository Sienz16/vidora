# Requirements

## Goal

Provide a private bot workflow equivalent to the useful behavior visible in the supplied screenshot:

1. The owner sends a video as a WhatsApp document or a photo (as a document or HD image) to a dedicated bot number.
2. The bot acknowledges the job and reports bounded progress states.
3. The service validates and processes the media with FFmpeg (`hd29` MP4 clips for video, `photo_hd` JPEG for photos).
4. The bot returns Status-ready MP4 clips in order or the prepared HD JPEG image.
5. The owner manually posts the clips or forwards the HD image to WhatsApp Status.

The project does not guarantee that WhatsApp will skip recompression.

## Actors and trust boundaries

- **Owner:** the single explicitly allowlisted WhatsApp account permitted to use the bot.
- **Bot number:** a dedicated secondary WhatsApp number linked through Baileys.
- **WhatsApp sidecar:** untrusted-input boundary; handles protocol messages and media download.
- **Rust service:** validates every sidecar request and owns all processing.
- **VPS:** trusted to process media temporarily. The VPS provider can theoretically access host storage; this is not an on-device privacy solution.

## MVP functional requirements

### Input and authorization

- Accept one video document (up to 300 MiB) or one photo (as an image document or direct WhatsApp `imageMessage`, up to 50 MiB) per job.
- Accept jobs only when the authenticated WhatsApp sender matches `OWNER_WHATSAPP_NUMBER`.
- Reject group messages, broadcasts, unknown senders, forwarded commands, and unsolicited media from other accounts.
- Deduplicate WhatsApp message IDs so retries do not create duplicate jobs or deliveries.
- Allow one active job for the owner. Additional jobs receive a concise busy response.
- Support `/help` and `/status`; no other commands are required for MVP.

### Local admin UI

- Provide an optional Compose overlay with a host port bound only to `127.0.0.1`.
- Authenticate with `WEB_ADMIN_TOKEN`, an HttpOnly SameSite=Strict session cookie, and CSRF protection on authenticated mutations.
- Pair through a private token-authenticated sidecar endpoint. Keep pairing codes transient and out of logs and storage.
- Display connection status and the active or latest job. Proxy only the currently available artifact without acknowledging delivery.
- Keep browser uploads, job submission, history, automatic Status posting, and multi-user access out of scope.

### Validation

- Enforce a maximum declared and received input size of 300 MiB for videos and 50 MiB for photos.
- For videos: enforce a maximum source duration of 10 minutes, dimensions `2..=4096`, and a maximum of 32 output clips. Require exactly one video stream and at most one audio stream (ignoring `mebx` camera metadata tracks in MOV/MP4 inputs and rejecting other extra tracks).
- For photos: support JPEG, PNG, WebP, BMP, TIFF, and single-frame HEIC/AVIF in MOV containers with dimensions `2..=12,288` px, at most `52,000,000` pixels, and a single decoded frame (reject animated/multi-frame images and unexpected extra tracks).
- Inspect the downloaded file with ffprobe before encoding.
- Reject corrupt, encrypted, unsupported, truncated, or timed-out inputs without starting an encode.
- Treat original filenames as display metadata only. Never use them as filesystem paths or command fragments.

### Processing

- Implement `hd29` for videos and `photo_hd` for photos (both documented in `ENCODING.md`) as the default MVP profiles.
- For videos: preserve aspect ratio using scale-and-pad; do not stretch or upscale beyond `1080x1920`.
- For photos: bake EXIF/display rotation into pixels, flatten alpha channels onto white, downscale only if larger than `3840` px on the long edge (never upscale or crop/pad), strip metadata, and enforce a `4,900,000`-byte JPEG output cap via bounded quality and `0.85x` resolution retries.
- Convert supported BT.2020 HLG and HDR10 camera sources to SDR BT.709. Use the compatible base layer for Dolby Vision Profile 8. Reject HDR sources whose color information cannot be interpreted safely.
- Use FFmpeg through an argument-array process API with no shell.
- Run one encode at a time with CPU, memory, output-size, and wall-clock limits.
- Validate every output with ffprobe and the profile constraints before delivery.
- Record the exact FFmpeg version, arguments, input metadata, output metadata, durations, sizes, and errors in a redacted job record.

### Delivery and cleanup

- Return video clips in sequence with a concise part number, for example `Part 1/3`, and return prepared photos as an `imageMessage` ready to forward to Status.
- Retry bounded transient delivery failures without re-encoding or duplicating successful sends.
- Report terminal failure clearly; never report success before every output is validated and delivered.
- Delete input, intermediate, and output media within 15 minutes after successful delivery and within 30 minutes after failure or abandonment.
- Keep only minimal redacted job metadata after media deletion.

## Non-functional requirements

- One Rust worker and one WhatsApp sidecar instance.
- No public API, public WhatsApp sidecar port, ads, analytics, tracking, or third-party media storage.
- No WhatsApp session material in source control, build logs, container images, or unencrypted backups.
- Reproducible dependency installation from lockfiles.
- Actionable structured logs with secrets and personal identifiers redacted.
- Graceful shutdown must stop accepting new work and clean temporary media without corrupting persistent session state.

## Explicitly out of scope for MVP

- Automatic posting to WhatsApp Status
- Public or multi-user access
- Photo-to-video (`photoAsVideo`), music, editing, stickers, or AI features
- Copying PURESTATUS branding, proprietary code, or assets
- Forced 60 fps output
- Guaranteed quality parity, a compression bypass, or official WhatsApp support
- The `long59` and `hd90` profiles until `hd29` passes manual quality tests

## MVP acceptance criteria

MVP is accepted only when all of these are demonstrated:

1. An unauthorized sender and a group message cannot start a job.
2. The owner can send a supported document and receive all valid clips in order.
3. Duplicate message delivery does not duplicate processing or completed output delivery.
4. Corrupt, oversized, over-duration, and maliciously named inputs are rejected safely.
5. Outputs satisfy the selected codec, pixel format, resolution, frame-rate, duration, and size constraints.
6. FFmpeg has no external network route, is restricted to local file processing, and runs as an unprivileged, resource-bounded process.
7. Media cleanup is verified after success, failure, timeout, cancellation, and restart.
8. Automated tests and the manual quality matrix in `TESTING.md` pass, with device and WhatsApp versions recorded.
9. No credentials, session data, personal media, or real phone numbers appear in the repository or logs.
10. Documentation reports actual results without unsupported quality claims.
11. The optional admin UI rejects unauthenticated requests, invalid CSRF, and unapproved hosts.
12. Only the admin UI has a host port, bound to loopback. Pairing codes are never logged or persisted.
13. The UI exposes no job submission, delivery acknowledgement, or terminal mutation.
