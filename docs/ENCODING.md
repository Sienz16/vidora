# Encoding profiles and quality validation

## Evidence status

The settings below are candidate starting points derived from the independent open-source StatusDrop project, not verified PURESTATUS settings and not official WhatsApp encoding requirements.

The supplied screenshot contains a log saying `Processing to H.264 60fps`, but no source code or FFmpeg arguments are visible. That log is not evidence that 60 fps produces better Status quality.

WhatsApp publicly documents a maximum Status duration of 90 seconds. The 15.8 MB boundary and the posting behaviors described by StatusDrop are empirical claims from that project and must be tested independently.

## MVP profile: `hd29`

| Setting | Candidate value |
| --- | --- |
| Canvas | 1080×1920 |
| Aspect handling | Scale to fit, then pad; never stretch |
| Video codec | H.264 via `libx264` |
| H.264 profile/level | High / 4.0 |
| Pixel format | `yuv420p` |
| Frame rate | 29.97 fps; do not duplicate frames to force 60 fps |
| Preset | `medium` |
| CRF | 23 |
| Maximum rate | 3800 kbit/s |
| Buffer size | 5700 kbit/s |
| GOP | 250 as a starting candidate |
| Audio | AAC, 128 kbit/s, stereo, 44.1 kHz when audio exists |
| Container | MP4 with `+faststart` |
| Segment length | At most 29 seconds |
| Candidate clip cap | 15,800,000 bytes; validate empirically |

These values are a test profile, not a promise of transparent quality or passthrough behavior.

## Later profiles

Do not implement these until `hd29` passes the manual quality matrix:

- `long59`: 720×1280, at most 59 seconds, CRF 24, maxrate 2200 kbit/s, buffer 3300 kbit/s.
- `hd90`: 1080×1920, at most 89 seconds, CRF 23, maxrate 3800 kbit/s, buffer 5700 kbit/s, with a larger file-size limit.

## Processing rules

- Probe first and record source rotation, variable frame rate, color metadata, stream counts, and actual duration.
- Scale and pad without distorting the source. Do not upscale beyond the selected profile canvas; record when a lower-resolution source is enlarged.
- Preserve the original frame rate when it is lower than the cap. For sources above the cap, use a documented frame-selection strategy rather than claiming improved motion.
- Phase 1 keeps variable timestamps when the reported average and nominal rates are within the cap. If either exceeds the cap, FFmpeg resamples the timeline to 30000/1001 fps, dropping or duplicating frames as needed for that cadence.
- Do not copy unknown data or subtitle streams into MVP output.
- Use no shell command strings. Each option and path is a separate process argument.
- Bound segmenting, encoding, probing, temporary storage, and output size.
- If a clip exceeds its cap, use a documented bounded retry with adjusted CRF/rate limits; never silently deliver an invalid clip.
- Phase 1 requests closed-GOP splits every 28 seconds to leave room for frame and audio packet boundaries. A clip that exceeds 29 seconds fails validation. A clip above 15,800,000 bytes triggers one full-job retry at CRF 27, maxrate 3000 kbit/s, and buffer 4500 kbit.
- The initial demuxer allowlist covers the MOV/MP4 family, Matroska/WebM, AVI, and MPEG-TS. The probe must report positive duration, and either source dimension may be at most 4096 pixels. HDR detected from transfer characteristics or recognized stream metadata is rejected. Full decode checks catch signaled corruption, frame-count mismatches when counts are present, and substantial timing discrepancies.
- Preserve valid color metadata where possible. HDR-to-SDR tone mapping is unresolved and must not be represented as supported until tested.

## Output validation

Every artifact must pass ffprobe checks for:

- MP4 container with exactly one H.264 video stream and at most one AAC audio stream
- `yuv420p` pixel format
- Expected canvas and no unexpected rotation
- Frame rate no greater than 30.1 fps
- Duration within the profile limit and expected segment boundary
- File size within the selected cap
- Decodability from beginning to end without fatal FFmpeg errors

## Required A/B quality experiment

For each representative source, record device model, OS, WhatsApp version, settings, source metadata, output metadata, and date. Compare:

- **A:** original video posted directly to Status
- **B:** processed video posted directly to Status
- **C:** processed video sent in a chat with HD selected, then the new message forwarded to My Status

Path C is an experiment based on StatusDrop's manual notes, not a guaranteed workflow. Do not add it to user instructions unless testing reproduces a meaningful benefit.

Evaluate visible detail, text edges, motion, banding, color shifts, audio sync, rejected uploads, output duration, and delivered file size when measurable. A local VMAF/SSIM score does not prove what WhatsApp delivered to viewers.

## Reference

- StatusDrop source and documented test notes: https://github.com/Shamanthnp1/WAstatus
- Reference Kotlin encoding implementation: https://github.com/Shamanthnp1/WAstatus/blob/main/mobile/statusdrop_app/android/app/src/main/kotlin/com/wastatusvideo/statusdrop/LocalVideoProcessor.kt
- WhatsApp Status information: https://faq.whatsapp.com/454876960047011

StatusDrop has GPL/AGPL licensing. Its values and documented behavior may guide experiments; copying its implementation creates license obligations and requires explicit review.
