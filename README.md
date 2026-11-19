# Vidora

![Phase 1 · Local CLI](https://img.shields.io/badge/Phase_1-Local_CLI-2563EB?style=flat-square)
![Rust 2024](https://img.shields.io/badge/Rust-2024-000000?style=flat-square&logo=rust)
![FFmpeg 8](https://img.shields.io/badge/FFmpeg-8-007808?style=flat-square&logo=ffmpeg)
![Tokio](https://img.shields.io/badge/Async-Tokio-334155?style=flat-square)

Vidora prepares short MP4 clips for WhatsApp Status. Run the Rust CLI locally or use the self-hosted bot workflow. The bot accepts video documents from one owner and sends the parts in order. The owner posts them to Status manually. The sidecar has been tested with mocks and needs a live secondary-account trial.

## What Phase 1 does

- Accepts a local video up to 300 MiB and 10 minutes, with one video stream and optional audio. Supported container families include MP4/MOV, Matroska/WebM, AVI, and MPEG-TS. Detected HDR is rejected.
- Uses FFmpeg and the candidate `hd29` profile to fit the video onto a 1080 × 1920 canvas while preserving its aspect ratio.
- Produces ordered H.264 MP4 clips with AAC audio when present. Each clip is validated against a 29-second limit and a 15,800,000-byte cap.
- Removes working media after processing and exports validated clips to a private directory under the destination you choose.

Vidora is a clean-room project. The encoding profile is a candidate for testing, and WhatsApp playback quality still needs validation on real devices. See the [encoding profile and test plan](docs/ENCODING.md).

## Run the local processor

Use a stable Rust toolchain with Cargo and FFmpeg 8 built with `libx264`. Put both `ffmpeg` and `ffprobe` on `PATH`.

```sh
git clone https://github.com/Sienz16/vidora.git
cd vidora
mkdir -p output
cargo run --locked -- ./sample.mp4 ./output
```

Replace `./sample.mp4` with your input video. On success, the CLI prints a redacted JSON job record followed by the path to the exported clips. The source file stays in place. Manage or delete the exported copies after use.

## Stack and roadmap

| Phase | Stack | Scope |
| --- | --- | --- |
| 1 · Implemented | Rust 2024, Tokio, FFmpeg 8 | Local validation, transcoding, segmentation, and cleanup |
| 2 · Implemented | Rust, axum, SQLite | Authenticated internal API, durable jobs, and restart recovery |
| 3 · Implemented | Node.js 24, Baileys | Owner-only WhatsApp access and ordered clip delivery |
| 4 · Planned | Docker Compose | Private networking, resource limits, and container hardening |
| 5–6 · Planned | Dedicated secondary WhatsApp account, VPS | Controlled quality trial followed by deployment |

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
