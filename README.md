# Vidora

![Verification · Live trial pending](https://img.shields.io/badge/Verification-Live_trial_pending-2563EB?style=flat-square)
![Rust 2024](https://img.shields.io/badge/Rust-2024-000000?style=flat-square&logo=rust)
![FFmpeg 9](https://img.shields.io/badge/FFmpeg-9-007808?style=flat-square&logo=ffmpeg)
![Node.js 24](https://img.shields.io/badge/Node.js-24-5FA04E?style=flat-square&logo=nodedotjs)
![Docker Compose](https://img.shields.io/badge/Docker-Compose-2496ED?style=flat-square&logo=docker)

Vidora prepares short MP4 clips and HD JPEG photos for WhatsApp Status. Run the Rust CLI locally or use the self-hosted bot workflow. The bot accepts video documents and photos (as documents or direct HD images) from one owner and returns prepared MP4 parts in order or a prepared HD JPEG for manual posting or forwarding to Status. The sidecar has been tested with mocks and needs a live secondary-account trial.

## Media support

- **Videos (`hd29`):** Accepts a local video up to 300 MiB and 10 minutes, with one video stream and optional audio across MP4/MOV, Matroska/WebM, AVI, and MPEG-TS. Tone-maps standard BT.2020 HLG, HDR10, and compatible Dolby Vision Profile 8 to SDR BT.709, fits onto a 1080 × 1920 canvas while preserving aspect ratio, and exports ordered H.264/AAC MP4 clips capped at 29 seconds and 15,800,000 bytes.
- **Photos (`photo_hd`):** Accepts a local JPEG, PNG, WebP, BMP, TIFF, or single-frame HEIC/AVIF photo up to 50 MiB (at most 12,288 px per edge and 52 MP). Applies EXIF/display rotation into pixels, flattens alpha channels onto white, tone-maps supported BT.2020 HLG/PQ photos to SDR, downscales only if longer than 3840 px while preserving aspect ratio (never upscales, crops, or pads), strips metadata, and exports a `yuvj420p` JPEG capped at 4,900,000 bytes.
- Removes working media after processing and exports validated outputs to a private directory under the destination you choose.

Vidora is a clean-room project. Both encoding profiles (`hd29` and `photo_hd`) are candidates for testing, and WhatsApp playback/image quality still needs validation on real devices. See the [encoding profile and test plan](docs/ENCODING.md).

## Run the local processor

Use a stable Rust toolchain with Cargo and FFmpeg 9 with `libx264`. The container pins FFmpeg 9.0.2. Put both `ffmpeg` and `ffprobe` on `PATH` for local use.

```sh
git clone https://github.com/Sienz16/vidora.git
cd vidora
mkdir -p output
cargo run --locked -- ./sample.mp4 ./output
cargo run --locked -- ./sample.jpg ./output
```

Replace `./sample.mp4` or `./sample.jpg` with your input video or photo. On success, the CLI prints a redacted JSON job record followed by the path to the exported outputs. The source file stays in place. Manage or delete the exported copies after use.

## Run the bot

Use Docker Engine and Compose on Linux. Configure a private `.env` from `.env.example`, start the API, and pair a dedicated secondary WhatsApp account from a private terminal. After pairing, start the stack and send a video as a document or a photo (as a document or HD image) from the configured owner account. Use `/help` or `/status` in the chat. See the [deployment runbook](docs/DEPLOYMENT.md).

The Compose smoke test uses generated media and runs without a WhatsApp session. Live trial and VPS verification are pending. See the [testing guide](docs/TESTING.md).

## Stack and roadmap

| Phase | Stack | Scope |
| --- | --- | --- |
| 1 · Implemented | Rust 2024, Tokio, FFmpeg | Local validation, transcoding, segmentation, and cleanup |
| 2 · Implemented | Rust, axum, SQLite | Authenticated internal API, durable jobs, and restart recovery |
| 3 · Implemented | Node.js 24, Baileys | Owner-only WhatsApp access and ordered clip delivery |
| 4 · Implemented | Docker Compose | Private networking, resource limits, and container hardening |
| 5–6 · Pending live checks | Dedicated secondary WhatsApp account, VPS | Controlled quality trial followed by deployment |

Rust owns media processing and job state. The Node.js sidecar holds the WhatsApp session and handles messaging. See the [architecture](docs/ARCHITECTURE.md) and the [phase review criteria](docs/IMPLEMENTATION-PLAN.md).

## Verify locally

Run the Rust checks with `ffmpeg` and `ffprobe` available. The tests generate their own video fixtures.

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
npm ci --prefix sidecar --ignore-scripts
npm test --prefix sidecar
```

## Read in this order

1. [AGENTS.md](AGENTS.md) — mandatory instructions for implementing and auditing agents.
2. [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md) — MVP behavior, limits, and acceptance criteria.
3. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — component boundaries and data flow.
4. [docs/ENCODING.md](docs/ENCODING.md) — candidate encoding profiles and quality experiments.
5. [docs/SECURITY.md](docs/SECURITY.md) — threat model and required controls.
6. [docs/IMPLEMENTATION-PLAN.md](docs/IMPLEMENTATION-PLAN.md) — audited implementation phases.
7. [docs/TESTING.md](docs/TESTING.md) — automated and manual verification.
8. [docs/DEPLOYMENT.md](docs/DEPLOYMENT.md) — VPS deployment and operations.
9. [docs/RE-EVIDENCE.md](docs/RE-EVIDENCE.md) — acquired artifact, REA evidence, references, and limitations.
10. [docs/AUDIT.md](docs/AUDIT.md) — review checklist and audit report format.

## Non-negotiable behavior

- Use a dedicated secondary WhatsApp number, never the owner's primary number for the bot session.
- Accept jobs only from an explicit owner allowlist; ignore groups and unknown senders.
- Do not publish Status updates automatically.
- Do not execute or install the downloaded PURESTATUS package.
- Do not store media in third-party object storage or add analytics.
- Do not claim quality improvements without recorded A/B tests.
- Do not commit, push, deploy, install system prerequisites, or link a WhatsApp number without explicit user approval.

## Implementation handoff

Commit each completed implementation phase separately, with test evidence and unresolved issues for review. Live linking and deployment have separate approval gates.
