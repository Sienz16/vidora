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

### Split native ARM64 APK archive (`config.arm64_v8a.apk`)

Recorded on 2026-11-20:

- `config.arm64_v8a.apk` SHA-256: `58003c749f56adeb904421ed4077b55dc10b172ee078368f898f3f94f8ab5c0e`
- `identify_flutter_build`: `flutter_detected: false` (`ev_940074147242cd2aa9bd7bb09318b54a75d1c09cf185821d0be0d7abee7c8330`, complete coverage)
- `inspect_artifact` Parent Evidence ID: `ev_20731756106db310d8635cf000e127e1ec36063755ad73f848b13288dd1ea250`
- Summary Evidence ID: `ev_1d2eb9bcd455e8e087880a21376d094e7b3bd72dbb40dd6f8233d1f6d8974d74`
- Graph: 17 nodes, 17 occurrences, 16 edges; 0 integrity contradictions
- Observed native libraries under `lib/arm64-v8a/`: `libffmpegkit.so`, `libffmpegkit_abidetect.so`, `libavcodec.so`, `libavdevice.so`, `libavfilter.so`, `libavformat.so`, `libavutil.so`, `libswresample.so`, `libswscale.so`, `libc++_shared.so`, and third-party/AndroidX libraries.

### Extracted base APK DEX bytecode observations (`com.yulva.purepixel.apk`)

Recorded on 2026-11-20 using REA `extract_artifact` (`ev_5c29b5d619962e7bad30026e69c8fee300f7aaecdaf30443a0baf275472dedd0`, containment verified, 0 integrity contradictions) followed by read-only Python Dalvik bytecode disassembly of `classes.dex` and `classes2.dex`:

- Internal package namespace inside `com.yulva.purepixel.apk` is `com.damtechdesigns.purepixel.*`, establishing a concrete code link between the `com.yulva.purepixel` package and the `com.damtechdesigns.purepixel` identifier noted above.
- `CompressView`, `Lnh1`, `Lp`, `Lkm0`, and `Lkw7` implement two distinct static photo-processing paths in the Android client:
  1. **Default bot-delivery photo path (`Q0 = true` when `photoAsVideo` is false and `delivery_mode` is not `"share"`, `.jpg` output):**
     - Decodes bitmap with power-of-two `inSampleSize` pre-scaling targeting `photoJpegLongEdge` (default `3840`, or `1.5 * 3840` decode cap before exact resize).
     - Reads EXIF orientation (`3` -> 180°, `6` -> 90°, `8` -> 270°) and rotates the decoded bitmap into pixel orientation.
     - Downscales only if larger than `3840` px on the long edge (`Math.min(limit, dim)`), preserving original aspect ratio without upscaling, cropping, or padding.
     - Compresses JPEG starting at `photoJpegQuality` (default `92`, clamped `[60, 100]`). If the output exceeds `4,900,000` bytes, steps JPEG quality down by `5` to `60`; if still above `4,900,000` bytes and `max(width, height) >= 640`, scales dimensions by `0.85` and repeats.
     - Uploads the `.jpg` (`kind: "image"`, `Content-Type: image/jpeg`) via `issueUploadTicket` (`Lkw7->k` / `Lkw7->s`) so the WhatsApp bot sends it back to the user's chat for manual forwarding to Status.
  2. **Fallback `photoAsVideo` path (`Q0 = false`, `.mp4` output):**
     - Downscales to max long edge `1920`, loops the frame into a 5-second H.264 MP4 (`-filter:v 'loop=-1:1:0,trim=duration=5,crop=trunc(iw/2)*2:trunc(ih/2)*2' -pix_fmt yuv420p -c:v libx264 -x264-params no-mbtree=1 -crf 23 -maxrate 1600k`).

The opened REA archive sessions were closed.

## Deep Android analysis blocker

`inspect_android_package` was attempted on the base APK with summary detail. REA returned `capability_unavailable` / `unsupported_provider` because `REA_JADX_MCP_JAR` is not configured with a caller-supplied `jadx-headless-mcp 0.7.1` JAR.

The host also has no working Java runtime available to `java`, `keytool`, or `jarsigner`; no Android SDK `apksigner` was found. No JDK, JADX, or other prerequisites were installed.

Therefore there is currently no JADX-decompiled Java source or server-side evidence for:

- Server-side bot delivery behavior behind `issueUploadTicket` (e.g., whether the WhatsApp bot sends `imageMessage` vs `documentMessage`, or adds any server-side transform)
- Trackers, endpoint usage, exported-component behavior, or runtime permission use
- Actual signature validity or runtime anti-tampering behavior
- Whether WhatsApp Status preserves quality for forwarded `3840`-px `<= 4.9 MB` JPEGs on real devices

Resume with `inspect_android_package` only after an approved working JDK and JADX provider are configured. Preserve artifact digests and incomplete coverage.

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
- FFmpeg/H.264 video candidate profiles and FFmpeg JPEG photo preparation (`<= 3840` px long edge, `<= 4,900,000` bytes) can be implemented and tested without copying PURESTATUS.
- The safest first workflow returns prepared clips/images to the owner and leaves Status posting/forwarding manual.

Inferences requiring validation:

- The candidate `hd29` video profile and `photo_hd` JPEG profile will improve Status quality on the owner's devices.
- Forwarding the bot-delivered `imageMessage` directly to Status preserves better detail than uploading the original photo from the phone gallery.
- The chat-resend/forward workflow described by StatusDrop provides a benefit.
- WhatsApp will preserve any particular size, codec, frame rate, or quality.

Unresolved:

- PURESTATUS's actual implementation and quality profile.
- Whether this mirror candidate matches the official Play artifact.
- Current WhatsApp behavior across the owner's exact app/device combinations.

Do not turn these inferences or unknowns into product claims.

## Additional static video string observation

Static DEX string-table extraction on 2026-11-20 from the acquired PURESTATUS `2026.25R` base APK (SHA-256 `ef70aaec8ead265626f404275a5d0065fcbef8b7fbf010e391c408a58fd85ba8`) found strings for 1080×1920 fit-and-pad and the FFmpeg options shown below.

```text
-c:v libx264 -preset veryfast -crf 20 -r 30 -c:a aac -b:a 128k -movflags +faststart
```

This video observation covers string constants only. No method decompilation, control-flow trace, or runtime behavior was established for these constants. The strings support a candidate pre-encode configuration for testing. Actual invocation and WhatsApp's treatment of the result remain unknown.
