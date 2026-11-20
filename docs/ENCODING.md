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

## MVP photo profile: `photo_hd`

Informed by static Dalvik bytecode inspection of `com.yulva.purepixel` (`2026.25R`, documented in `RE-EVIDENCE.md`), `photo_hd` prepares still images as sRGB baseline-compatible JPEGs for chat delivery and manual forwarding to WhatsApp Status:

| Setting | Candidate value |
| --- | --- |
| Target long edge | At most `3840` px (downscale only; never upscale smaller photos) |
| Aspect handling | Preserve original aspect ratio; no cropping or padding |
| Orientation | Auto-apply EXIF / display-matrix rotation into pixel order, strip rotation metadata |
| Transparency | Flatten alpha channels onto solid white before JPEG conversion |
| HDR handling | Perceptual tone-map supported BT.2020 HLG / PQ sources to full-range SDR BT.709 (`pc`) |
| Resampler | Lanczos (`flags=lanczos`) |
| Output codec | JPEG (`mjpeg`) with `-huffman optimal` |
| Pixel format | `yuvj420p` |
| Quality ladder | `-q:v` in `[2, 3, 4, 5, 6, 8, 10]` |
| Output cap | `4,900,000` bytes; if exceeded after quality `10`, shrink long edge by `0.85` (floor `640` px, up to 6 scale steps) |
| Preview thumbnail | 32 px wide JPEG (`flags=area`, `-q:v 10`, <= 8192 bytes) passed in memory to the sidecar, never retained after terminal cleanup |

A separate `photoAsVideo` fallback (`1920` px max edge, 5-second looped H.264 MP4 at `CRF 23` / `1600k`) observed in `com.yulva.purepixel` is recorded as a future fallback candidate and is **not** implemented in MVP.

## Later profiles

Do not implement these until `hd29` passes the manual quality matrix:

- `long59`: 720×1280, at most 59 seconds, CRF 24, maxrate 2200 kbit/s, buffer 3300 kbit/s.
- `hd90`: 1080×1920, at most 89 seconds, CRF 23, maxrate 3800 kbit/s, buffer 5700 kbit/s, with a larger file-size limit.

## Processing rules

- Probe first and record source rotation, variable frame rate, color metadata, stream counts, and actual duration.
- Scale and pad without distorting the source. Do not upscale beyond the selected profile canvas; record when a lower-resolution source is enlarged.
- Preserve the original frame rate when it is lower than the cap. For sources above the cap, use a documented frame-selection strategy rather than claiming improved motion.
- Phase 1 keeps variable timestamps when the reported average and nominal rates are within the cap. If either exceeds the cap, FFmpeg resamples the timeline to 30000/1001 fps, dropping or duplicating frames as needed for that cadence.
- Ignore `mebx` camera metadata tracks in MOV/MP4 input. Reject other extra tracks, and map only the video and optional audio into the output.
- Use no shell command strings. Each option and path is a separate process argument.
- Bound segmenting, encoding, probing, temporary storage, and output size.
- If a clip exceeds its cap, use a documented bounded retry with adjusted CRF/rate limits; never silently deliver an invalid clip.
- Phase 1 requests closed-GOP splits every 28 seconds to leave room for frame and audio packet boundaries. A clip that exceeds 29 seconds fails validation. A clip above 15,800,000 bytes triggers one full-job retry at CRF 27, maxrate 3000 kbit/s, and buffer 4500 kbit.
- The initial video demuxer allowlist covers the MOV/MP4 family, Matroska/WebM, AVI, and MPEG-TS (`duration > 0`, max `4096` px per side, max 600 s). The photo allowlist covers `jpeg_pipe`, `png_pipe`, `webp_pipe`, `bmp_pipe`, `tiff_pipe`, and single-frame HEIC/AVIF in MOV containers (`50 MiB` max input, max `12,288` px per side, max `52,000,000` pixels, single decoded frame only; animated WebP / multi-frame TIFF are rejected).
- HDR conversion for videos and photos requires BT.2020 primaries, a BT.2020 non-constant-luminance matrix, and either an HLG or PQ transfer. FFmpeg 9 applies perceptual tone mapping and outputs SDR BT.709 (`tv` range for video, `pc` range for JPEG). HDR10+ dynamic metadata is not carried into the output. Dolby Vision Profile 8 requires a matching HLG or HDR10-compatible base layer. SDR HEIC stills carrying harmless `Content light level metadata` without HDR transfer tags are processed as SDR. Other Dolby Vision profiles or HDR media with missing or mismatched color metadata are rejected.

## Output validation

Every artifact must pass ffprobe checks for:

- MP4 container with exactly one H.264 video stream and at most one AAC audio stream
- `yuv420p` pixel format
- Expected canvas and no unexpected rotation
- Square pixels, signaled as `1:1` or left implicit by the MP4
- Limited-range BT.709 color tags and no HDR metadata for tone-mapped clips
- Frame rate no greater than 30.1 fps
- Duration within the profile limit and expected segment boundary
- File size within the selected cap
- Decodability from beginning to end without fatal FFmpeg errors

### Photo outputs (`photo_hd`)

Every photo artifact (`photo-000.jpg`) must pass ffprobe (`-f jpeg_pipe`) and full decode checks for:

- `jpeg_pipe` container with one MJPEG video stream and zero audio/data/subtitle streams
- `yuvj420p` pixel format
- Width and height between `2` and the attempt's `max_edge`, with `0` residual rotation
- Square pixels, signaled as `1:1` or implicit
- No HDR transfer tags (`smpte2084` / `arib-std-b67`), no HDR side data, and no residual BT.2020 tags when tone-mapped
- File size `1..=4,900,000` bytes
- Single-frame decodability without fatal FFmpeg errors

## Required A/B quality experiment

For each representative source, record device model, OS, WhatsApp version, settings, source metadata, output metadata, and date.

For videos, compare:

- **A:** original video posted directly to Status
- **B:** processed video posted directly to Status
- **C:** processed video sent in a chat with HD selected, then the new message forwarded to My Status

For photos, compare:

- **A:** original photo posted directly to Status from the phone gallery
- **B:** processed `photo_hd` JPEG saved to gallery and posted to Status
- **C:** processed `photo_hd` JPEG delivered by the bot as an `imageMessage` in chat, then forwarded directly to My Status without saving to the gallery first

Path C is an experiment based on StatusDrop's manual notes, not a guaranteed workflow. Do not add it to user instructions unless testing reproduces a meaningful benefit.

Evaluate visible detail, text edges, motion, banding, color shifts, audio sync, rejected uploads, output duration, and delivered file size when measurable. A local VMAF/SSIM score does not prove what WhatsApp delivered to viewers.

## Reference

- StatusDrop source and documented test notes: https://github.com/Shamanthnp1/WAstatus
- Reference Kotlin encoding implementation: https://github.com/Shamanthnp1/WAstatus/blob/main/mobile/statusdrop_app/android/app/src/main/kotlin/com/wastatusvideo/statusdrop/LocalVideoProcessor.kt
- WhatsApp Status information: https://faq.whatsapp.com/454876960047011

StatusDrop has GPL/AGPL licensing. Its values and documented behavior may guide experiments; copying its implementation creates license obligations and requires explicit review.
