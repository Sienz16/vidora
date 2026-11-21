# Agent instructions

These are live project instructions. Keep them current when an approved decision changes the implementation.

## Required reading

Before changing files, read:

1. `README.md`
2. `docs/REQUIREMENTS.md`
3. `docs/ARCHITECTURE.md`
4. `docs/SECURITY.md`
5. The current phase in `docs/IMPLEMENTATION-PLAN.md`
6. `docs/RE-EVIDENCE.md` before changing encoding behavior or making claims about PURESTATUS

## Scope and architecture

- Build the clean-room bot described in the documents, not a PURESTATUS clone with copied code, branding, or assets.
- Rust owns jobs, validation, FFmpeg, output metadata, and cleanup.
- Node.js 24 owns only the Baileys WhatsApp connection and delivery.
- Keep the bot single-owner and single-instance. A loopback-only admin UI is allowed for pairing, connection status, and read-only job monitoring. Bind its host port to `127.0.0.1` and use an encrypted SSH tunnel or Tailscale Serve for remote access. Do not add public deployment, multi-tenancy, AI features, image generation, automatic Status posting, or unrelated features.
- Do not replace the Rust/Node boundary with Node-only or another stack without explicit user approval.

## Safety and privacy

- Never install or execute the acquired PURESTATUS XAPK/APKs.
- Never use the owner's primary WhatsApp number for the bot.
- Never commit credentials, pairing codes, session files, phone numbers, media, or real user data.
- Never invoke FFmpeg through a shell or interpolate media filenames into a command string.
- Do not expose the Node.js sidecar or internal Rust API on a public interface.
- Do not download replacement APKs from mirrors or install Java/JADX without explicit user approval.
- Static RE observations are not runtime evidence. Preserve all stated provenance and coverage limitations.

## Engineering rules

- Prefer the smallest implementation that satisfies the current phase.
- Validate all external input at the WhatsApp and HTTP boundaries. Trust neither metadata nor filenames.
- Use bounded concurrency, bounded input size, timeouts, deterministic cleanup, and idempotent message handling.
- Pin dependencies and use lockfiles. Do not add analytics, remote media storage, or unnecessary services.
- Keep logs operational and minimal. Do not log media contents, authorization headers, session material, or full user identifiers.
- Update the relevant document when approved behavior changes; do not rewrite evidence to match an implementation.

## Verification and completion

- Run the checks required for the current phase and `docs/TESTING.md`.
- Never mark work complete with failing, unrun, or unverified tests.
- Distinguish observations, inferences, and unknowns. Do not claim a compression bypass, official WhatsApp support, or measured similarity without evidence.
- Keep each completed implementation phase in a separate commit for review when continuing across phases. Do not commit, push, tag, release, deploy, or link WhatsApp without explicit user approval.

## Handoff format

Every implementation-phase response must include:

- Phase attempted and actual files changed
- Commands run and their exact outcomes
- Tests added and evidence produced
- Deviations from the documents and why
- Security/privacy implications
- Unresolved questions and the recommended next step
