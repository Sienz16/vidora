# Audit guide

Use this guide to review another agent's implementation. Trust the repository and reproduced evidence, not the implementation summary.

## Review procedure

1. Read `AGENTS.md`, the current phase, and the requirements being claimed.
2. Inspect the actual changed files and dependency lockfiles.
3. Compare behavior with `REQUIREMENTS.md`, `ARCHITECTURE.md`, `SECURITY.md`, and `ENCODING.md`.
4. Run the relevant tests independently where the environment permits.
5. Inspect test quality and negative cases, not only test counts.
6. Record deviations, unverified claims, security findings, and remaining unknowns.
7. Approve only the audited phase. Do not authorize deployment or WhatsApp linking on the user's behalf.

## Universal rejection conditions

Do not approve a phase if any of these apply:

- Required tests fail, were not run, or were replaced by a summary.
- FFmpeg is invoked through a shell or with user-controlled command fragments.
- The bot accepts groups or senders outside the owner allowlist.
- Internal APIs or artifact downloads lack authentication or bounds.
- User input controls filesystem paths.
- Secrets, session data, private media, real phone numbers, or pairing codes appear in source or logs.
- The implementation adds public access, automatic Status posting, analytics, third-party media storage, or unrelated features.
- Cleanup is missing on a reachable failure path.
- Documentation claims official provenance, malware absence, compression bypass, or quality parity without valid evidence.
- The Rust/Node boundary was replaced without approval.

## Phase 1 audit: media core

- Inspect every process invocation and confirm argument-array execution.
- Verify ffprobe stream rules, duration/size bounds, and error handling.
- Compare output validation with every `hd29` constraint.
- Inspect segmentation and bounded retry logic for infinite loops or silent quality degradation.
- Test malicious filenames, corrupt input, long input, hanging processes, and oversized output.
- Verify media cleanup on success, failure, timeout, and interruption.
- Confirm test fixtures are generated/authorized and no private media is committed.

## Phase 2 audit: API and jobs

- Exercise every route without credentials and with invalid credentials.
- Verify streaming limits are enforced on actual bytes, not just declared metadata.
- Inspect job/artifact path resolution and traversal handling.
- Test duplicate message IDs, concurrent duplicate requests, restart reconciliation, and idempotent delivery acknowledgement.
- Inspect SQLite transactions and state transitions for jobs stuck in ambiguous states.
- Verify artifact responses cannot become unauthenticated public URLs.

## Phase 3 audit: WhatsApp sidecar

- Verify sender authorization uses authenticated protocol metadata.
- Test groups, broadcasts, unknown senders, unsupported messages, and spoofed captions.
- Inspect media download bounds and behavior during disconnects.
- Verify one message cannot create duplicate jobs or duplicate completed deliveries.
- Inspect Baileys version pinning, session-path permissions, pairing-code handling, and single-instance assumptions.
- Confirm no live account was linked without explicit approval.

## Phase 4 audit: deployment hardening

- Inspect effective Compose ports, networks, users, mounts, capabilities, and resource limits.
- Verify FFmpeg has no network route and cannot exhaust the host through one job.
- Check dependency/container reports and investigate relevant findings.
- Test graceful shutdown, restart reconciliation, disk pressure, and retention sweeps.
- Inspect build context and images for secrets, session files, and media.

## Phase 5 audit: secondary-account trial

- Confirm explicit approval and use of a dedicated secondary number.
- Reproduce unauthorized-sender and group rejection on the live boundary.
- Review manual test records and actual output metadata.
- Check reconnect/restart behavior for duplicate delivery.
- Compare quality claims with the recorded A/B/C matrix and app/device versions.
- Inspect logs and retention after real media processing.

## Phase 6 audit: VPS

- Record approved host, source revision, lockfiles, and deployed configuration.
- Verify no public sidecar/API exposure and no unexpected services.
- Verify encrypted session backup handling and that only one sidecar uses the session.
- Run smoke, authorization, cleanup, restart, and resource tests.
- Confirm rollback cannot run two sessions simultaneously.

## Audit report template

```text
Phase:
Verdict: approved / changes required / blocked
Files inspected:
Commands independently run and results:
Requirements satisfied:
Findings (severity, file:line, evidence, required fix):
Tests missing or inadequate:
Documentation/claim corrections:
Security and privacy notes:
Unresolved questions:
Scope explicitly not audited:
```

Link every finding to a concrete file and line when code exists. Mark a phase approved only when no required fix remains and the evidence supports the claimed scope.
