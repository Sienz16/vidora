# Security and privacy requirements

## Threat model

The system handles private videos, an unofficial WhatsApp session, unauthenticated internet-originated media, and complex native media parsers. Important threats include unauthorized bot use, malicious media, command/path injection, resource exhaustion, session theft, accidental public exposure, duplicate delivery, and residual media after completion.

Self-hosting reduces dependence on third-party apps but does not prove the system is malware-free or protect media from the VPS provider.

## WhatsApp controls

- Use a dedicated secondary bot number. Never use the owner's primary number for the bot session.
- Allow only the configured owner, compared against authenticated sender metadata. Owner setup in the UI requires an authenticated session, CSRF protection, and the sidecar bearer token.
- Reject groups, broadcasts, unknown senders, and unexpected message types.
- Do not auto-post Status updates, auto-forward media, or expose bot functionality to contacts.
- Run exactly one Baileys sidecar using a given session. A second instance can disconnect or corrupt the session.
- Treat Baileys as unofficial and expect breaking changes, logout, or account restriction.
- The sidecar mutes dependency console output and the Baileys logger. The pinned Signal dependency can print session objects directly to the console. Vidora emits its own fixed operational events.

## Input and parser controls

- Enforce declared and actual byte limits while streaming; never trust `Content-Length` alone.
- Bound WhatsApp media download, internal upload, ffprobe, encode, artifact download, and delivery times.
- Validate media using ffprobe and strict stream/container rules before encoding.
- Use random server-generated paths. Reject path separators and never resolve user-controlled paths.
- Start processes with argument arrays and no shell. Environment and working directory must be explicitly controlled.
- Limit source duration, output clips, output bytes, job count, and concurrent encodes.
- Keep FFmpeg and container base images patched.

## Process and container isolation

- Run Rust, Node.js, and FFmpeg as non-root users.
- Use read-only container root filesystems where practical, with only required writable volumes.
- Production images use pinned, shell-free runtime bases. Build tools and npm are confined to build stages.
- Attach the Rust/API container only to an internal processing network with no external route; only the sidecar receives a separate WhatsApp egress network.
- Restrict FFmpeg to local file processing and approved protocols. Do not grant extra capabilities for per-process network-namespace creation.
- The container build compiles FFmpeg without network support and enables only file and pipe protocols. Production processing arguments further restrict media inputs to the file protocol.
- Apply CPU, memory, process-count, file-size, and wall-clock limits. One job must not exhaust the VPS.
- Use separate volumes and permissions for Rust media data and Baileys credentials.
- Do not publish sidecar or internal API ports to the host or internet.

## Secrets and session data

- Supply secrets through a deployment secret mechanism or a protected environment file excluded from source control.
- `INTERNAL_API_TOKEN` must be cryptographically random and at least 32 bytes.
- Baileys session directories require restrictive permissions and encrypted backups if backed up.
- Never print session files, authorization headers, pairing codes, or tokens in application logs.
- Pairing codes are sensitive: use a one-time controlled linking procedure and do not retain terminal logs containing them.

## Media privacy and logging

- Do not use third-party object storage, analytics, crash-reporting media attachments, or external AI processing.
- Delete input and derived media on every terminal path within the required retention windows.
- Logs may contain job IDs, state transitions, durations, byte counts, and redacted error classes.
- Logs must not contain video contents, original filenames, full phone numbers/JIDs, captions, or authentication data.
- Artifact responses require authentication and must not be guessable public URLs.

## Supply chain and RE artifacts

- Build from source with reviewed dependencies and lockfiles.
- Do not install the downloaded PURESTATUS package or treat its mirror hash as proof of authenticity.
- Do not copy proprietary PURESTATUS code or resources into this project.
- Review Baileys release changes and pin an explicit version; do not track an unpinned branch.
- Generate dependency and container vulnerability reports during the security audit, while recognizing that scanners are not proof of safety.

## Local admin security

- Treat DNS rebinding, CSRF, XSS, local multi-user access, and pairing-code or credential disclosure as threats.
- Publish only the UI on host IPv4 loopback. Validate every request Host against the configured allowlist. Use SSH or Tailscale Serve for encrypted remote access.
- Store `WEB_ADMIN_TOKEN` in protected server configuration and memory. The UI sends the entered token only on login. Upstream bearer tokens stay on the server. Use a distinct `SIDECAR_CONTROL_TOKEN` and `INTERNAL_API_TOKEN`.
- Use bounded in-memory sessions, HttpOnly SameSite=Strict cookies with Path=/, and Secure cookies when HTTPS is configured. Require the exact Origin and same-origin fetch metadata for mutations, and a CSRF token after login. Rate-limit login failures and pairing starts.
- Serve local assets under strict CSP without inline scripts, HTML injection, or third-party requests. Return fixed errors without stack traces, upstream bodies, paths, or secrets.
- Bind the control listener to its internal bridge IP or local loopback. Keep the Docker host trusted because it may reach bridge addresses. No sidecar port is published.
- Reject owner-number pairing. A configured bot number pins the expected identity. Headless mode and startup without control require a number and report `bot_number_required` if it is missing.
- Store the UI-configured owner encrypted in the sidecar database. Keep the encryption key in the same private session volume and protect the whole volume and its backups. Return only an owner-configured flag. Reject conflicting environment configuration and prevent overwrite through the UI.
- Keep pairing codes in memory only. Clear them at cancellation, deadline, or completion. Stage pairing credentials until the bot identity is verified, then persist the session without the pairing code. Keep codes, tokens, full numbers, JIDs, cookies, and bodies out of logs.

## Security acceptance gate

Do not deploy until tests demonstrate unauthorized-sender rejection, bounded uploads, no shell invocation, safe path handling, duplicate-message idempotency, process resource limits, no published internal ports, secret redaction, and cleanup after every terminal path. For the optional UI, also verify loopback publication, private control binding, Host and CSRF checks, CSP, route allowlisting, and pairing-code redaction.
