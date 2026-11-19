# Reverse-engineering evidence and references

Recorded on 2026-11-18. This file distinguishes REA evidence, ordinary shell observations, public vendor claims, and unresolved questions. It does not establish that the acquired package is official, safe, or behaviorally equivalent to this project.

## User-supplied screenshots

Source: two screenshots supplied by the project owner and kept outside the repository.

Direct observations:

- A terminal shows `node index.js`, `Connected to WhatsApp HD Bot!`, receipt of a document video, processing to H.264/60 fps, and completion times/file sizes.
- The author says Gemini was used to analyze PURESTATUS and produce a similar tool.

Unknowns:

- Source code, WhatsApp library, FFmpeg arguments, output metadata, and whether WhatsApp actually avoided recompression.
- The claimed `99%` similarity has no supplied test method or measurements.

These screenshots motivate a workflow; they are not implementation evidence.

## Package identification

Current public listing:

- Google Play package: `com.yulva.purepixel`
- Product: PureStatus
- Listing: https://play.google.com/store/apps/details?id=com.yulva.purepixel
- Apple listing: https://apps.apple.com/us/app/purestatus-byebye-blur-status/id6444719503
- Developer product site: https://yulvaapps.com/

A separate package, `com.damtechdesigns.purepixel`, appears on other mirrors. It was not selected because its relationship to the current `com.yulva.purepixel` listing was not established.

Public listings describe select → compress → share to WhatsApp Status. The Apple listing and developer site describe local/on-phone compression. These are vendor statements, not verified binary behavior.

## Acquired candidate

A third-party APKPure copy was downloaded solely for static analysis. It was not installed or executed.

- Source page: https://apkpure.net/purestatus/com.yulva.purepixel/download
- Download endpoint: https://d.apkpure.net/b/XAPK/com.yulva.purepixel?version=latest
- Local XAPK filename: `purestatus-2026.25R.xapk`
- Extracted components are kept outside the repository.
- Package: `com.yulva.purepixel`
- Version: `2026.25R`, version code `155`
- XAPK size: 64,412,061 uncompressed member bytes; approximately 61.4 MiB download
- XAPK SHA-256: `99bebfa0550897ca7333ce6260b015d2d6f6a56798b7a5d40d7800f504c7c457`
- The computed XAPK hash matches APKPure's published hash. This establishes consistency with that mirror's listing, not official authenticity.

Component hashes:

| Component | SHA-256 |
| --- | --- |
| `com.yulva.purepixel.apk` | `ef70aaec8ead265626f404275a5d0065fcbef8b7fbf010e391c408a58fd85ba8` |
| `config.arm64_v8a.apk` | `58003c749f56adeb904421ed4077b55dc10b172ee078368f898f3f94f8ab5c0e` |
| `config.en.apk` | `98c24e17a496a865c8d7e8a3ce1a54d7dfb06e6dc00c9f6a8fef32a9de41de23` |
| `config.mdpi.apk` | `374943076dce9be5c4da10b5d45c735ecac7c62a1304f05e19327905ad0aa48d` |

The XAPK `manifest.json` reports minimum SDK 32, target SDK 37, and the split configuration. This is mirror/package metadata, not the decoded Android binary manifest.

## Permissions reported by XAPK metadata

The XAPK metadata lists media read permissions, selected-photo access, internet, notifications, foreground services including media processing, wake lock, network state, billing, advertising ID and ad-services permissions, install referrer, push messaging, and related declarations.

These declarations justify later manifest and tracker review, but a permission declaration does not prove a permission is used maliciously or used at runtime.

## Signing-certificate observation

An ordinary local Python parser extracted certificates from the APK v2/v3 signing blocks; it did not validate the APK signatures.

All four components contain the same extracted certificate SHA-256:

`4925a39758c6a8e933ee5c282ba3f48027ee91147e570034cde5a495abbcb2e9`

OpenSSL reports the extracted certificate subject and issuer as `C=US, ST=California, L=Mountain View, O=Google Inc., OU=Android, CN=Android`, valid from 2024-11-03 to 2054-11-03.

Certificate consistency across splits does not prove the package is official or unmodified. Compare against an official Play-installed export or another independently trusted reference before drawing a provenance conclusion.

## REA evidence

### XAPK archive

`open_binary` identified the XAPK as ZIP with the SHA-256 above. `inspect_artifact` retained the complete inventory and returned a summary.

- Parent Evidence ID: `ev_c0f29c04d45916b65a7909325a9507358336cd135988fc4ce859b3a26503e58f`
- Summary Evidence ID: `ev_5e8548f21a86c91a2388a03e3fe18969614f41e14888d2642277841a81b9b505`
- Graph: 7 nodes, 7 occurrences, 6 edges
- Integrity contradictions reported: 0

### Base APK archive

- Base APK SHA-256: `ef70aaec8ead265626f404275a5d0065fcbef8b7fbf010e391c408a58fd85ba8`
- Parent Evidence ID: `ev_7d7f5d0dda7a1da383244b7e773562608a7d896acfd8fe6c19c09f3427eea7d0`
- Summary Evidence ID: `ev_3d178b9105780e6afc6fd128ffc230dbc8c051d41699824feacaa81aebcc271a`
- Graph: 1,217 nodes, 1,333 occurrences, 1,332 edges
- Integrity contradictions reported: 0

REA limitations preserved:

- Inventory derives static hypotheses and does not execute the artifact or establish runtime behavior.
- ZIP inventory covers central-directory entries. Non-entry bytes, including an APK Signing Block or ZIP comment, are outside that occurrence inventory.
- Zero inventory integrity contradictions is not signature verification, official provenance, a malware verdict, or complete format-specific analysis.

The opened REA archive sessions were closed.

## Deep Android analysis blocker

`inspect_android_package` was attempted on the base APK with summary detail. REA returned `capability_unavailable` / `unsupported_provider` because `REA_JADX_MCP_JAR` is not configured with a caller-supplied `jadx-headless-mcp 0.7.1` JAR.

The host also has no working Java runtime available to `java`, `keytool`, or `jarsigner`; no Android SDK `apksigner` was found. No JDK, JADX, or other prerequisites were installed.

Therefore there is currently no decompiled-class evidence for:

- FFmpeg/MediaCodec usage or actual encoding constants
- Trackers, endpoint usage, exported-component behavior, or runtime permission use
- On-device versus server processing beyond vendor statements
- Actual signature validity or runtime anti-tampering behavior

Resume with `inspect_android_package`, then focused class/method/reference analysis only after an approved working JDK and JADX provider are configured. Preserve artifact digests and incomplete coverage.

## Independent implementation reference

StatusDrop is a separate open-source project, not evidence about PURESTATUS:

- Repository: https://github.com/Shamanthnp1/WAstatus
- Encoding source: https://github.com/Shamanthnp1/WAstatus/blob/main/mobile/statusdrop_app/android/app/src/main/kotlin/com/wastatusvideo/statusdrop/LocalVideoProcessor.kt
- Self-hosting documentation: https://github.com/Shamanthnp1/WAstatus/blob/main/SELF_HOSTING.md
- Baileys: https://github.com/WhiskeySockets/Baileys
- WhatsApp Status information: https://faq.whatsapp.com/454876960047011

StatusDrop documents local FFmpeg/x264 processing, candidate profiles, empirical posting steps, and unofficial Baileys delivery risks. Its license is GPL/AGPL. Use it as a behavioral and testing reference; do not copy implementation code without an explicit license review.

## Clean-room conclusions and unknowns

Supported conclusions:

- A Node.js WhatsApp sidecar plus an independent media-processing service is feasible.
- FFmpeg/H.264 candidate profiles can be implemented and tested without copying PURESTATUS.
- The safest first workflow returns clips to the owner and leaves Status posting manual.

Inferences requiring validation:

- The candidate `hd29` profile will improve Status quality on the owner's devices.
- The chat-resend/forward workflow described by StatusDrop provides a benefit.
- WhatsApp will preserve any particular size, codec, frame rate, or quality.

Unresolved:

- PURESTATUS's actual implementation and quality profile.
- Whether this mirror candidate matches the official Play artifact.
- Current WhatsApp behavior across the owner's exact app/device combinations.

Do not turn these inferences or unknowns into product claims.
