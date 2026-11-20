use std::{path::Path, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{Error, MAX_PHOTO_INPUT, PhotoInput, Tools, VideoInput, VideoProfile, tool};

const VIDEO_FORMATS: &str = "mov,mp4,m4a,3gp,3g2,mj2,matroska,webm,avi,mpegts";
const ALL_INPUT_FORMATS: &str = "mov,mp4,m4a,3gp,3g2,mj2,matroska,webm,avi,mpegts,jpeg_pipe,png_pipe,webp_pipe,bmp_pipe,tiff_pipe";
pub(crate) const MAX_SECONDS: f64 = 600.0;
pub(crate) const CLIP_SECONDS: f64 = 29.0;
pub(crate) const CLIP_CAP: u64 = 15_800_000;
pub(crate) const MAX_CLIPS: usize = 32;
const CAP_FPS: f64 = 30_000.0 / 1001.0;

pub(crate) const PHOTO_CAP: u64 = 4_900_000;
pub(crate) const PHOTO_TARGET_LONG_EDGE: u32 = 3840;
pub(crate) const PHOTO_MIN_LONG_EDGE: u32 = 640;
pub(crate) const PHOTO_QUALITIES: [u32; 7] = [2, 3, 4, 5, 6, 8, 10];
const PHOTO_MAX_SOURCE_EDGE: u32 = 12_288;
const PHOTO_MAX_SOURCE_PIXELS: u64 = 52_000_000;
const PHOTO_ALLOC_LIMIT: &str = "268435456";

#[derive(Debug, Deserialize)]
pub(crate) struct Probe {
    pub(crate) streams: Vec<Stream>,
    pub(crate) format: Format,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Format {
    pub(crate) format_name: String,
    duration: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Stream {
    codec_type: String,
    codec_name: Option<String>,
    codec_tag_string: Option<String>,
    profile: Option<String>,
    level: Option<i32>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    sample_aspect_ratio: Option<String>,
    avg_frame_rate: Option<String>,
    r_frame_rate: Option<String>,
    color_space: Option<String>,
    color_range: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    duration: Option<String>,
    nb_frames: Option<String>,
    disposition: Option<Disposition>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
}

#[derive(Debug, Deserialize)]
struct Disposition {
    attached_pic: u8,
}

#[derive(Debug, Deserialize)]
struct SideData {
    rotation: Option<i32>,
    side_data_type: String,
    dv_profile: Option<u8>,
    dv_bl_signal_compatibility_id: Option<u8>,
}

#[derive(Debug, Serialize)]
pub(crate) struct Video {
    width: u32,
    height: u32,
    fps: f64,
    nominal_fps: f64,
    rotation: i32,
    vfr_indicated: bool,
    color_space: Option<String>,
    color_range: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    pub(crate) frames: Option<u64>,
}

pub(crate) enum ClassifiedInput {
    Video(VideoInput),
    Photo(PhotoInput),
}

pub(crate) fn seconds(value: &str) -> Result<f64, Error> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or(Error::InvalidMedia)
}

fn fps(value: Option<&str>) -> Result<f64, Error> {
    let (num, den) = value
        .and_then(|rate| rate.split_once('/'))
        .ok_or(Error::InvalidMedia)?;
    let result = seconds(num)? / seconds(den)?;
    (result.is_finite() && result > 0.0)
        .then_some(result)
        .ok_or(Error::InvalidMedia)
}

pub(crate) fn has_alpha(pix_fmt: Option<&str>) -> bool {
    pix_fmt.is_some_and(|fmt| {
        matches!(
            fmt,
            "rgba"
                | "bgra"
                | "argb"
                | "abgr"
                | "pal8"
                | "ya8"
                | "ya16le"
                | "ya16be"
                | "rgba64le"
                | "rgba64be"
                | "bgra64le"
                | "bgra64be"
        ) || fmt.starts_with("yuva")
            || fmt.starts_with("gbrap")
            || fmt.starts_with("ayuv")
    })
}

impl Stream {
    fn has_hdr_metadata(&self) -> bool {
        matches!(self.codec_tag_string.as_deref(), Some("dvh1" | "dvhe"))
            || self.side_data_list.iter().any(|item| {
                matches!(
                    item.side_data_type.as_str(),
                    "DOVI configuration record"
                        | "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"
                        | "HDR Dynamic Metadata SMPTE2094-40"
                        | "Mastering display metadata"
                        | "Content light level metadata"
                )
            })
    }

    fn validate_hdr_transfer(&self, compatibility: u8) -> Result<(), Error> {
        if self.color_primaries.as_deref() != Some("bt2020")
            || self.color_space.as_deref() != Some("bt2020nc")
        {
            return Err(Error::UnsupportedHdr);
        }
        let dolby = self
            .side_data_list
            .iter()
            .find(|data| data.side_data_type == "DOVI configuration record");
        if let Some(dolby) = dolby {
            if dolby.dv_profile != Some(8)
                || dolby.dv_bl_signal_compatibility_id != Some(compatibility)
            {
                return Err(Error::UnsupportedHdr);
            }
        } else if matches!(self.codec_tag_string.as_deref(), Some("dvh1" | "dvhe")) {
            return Err(Error::UnsupportedHdr);
        }
        Ok(())
    }

    fn hdr_transfer(&self) -> Result<Option<&'static str>, Error> {
        let (transfer, compatibility) = match self.color_transfer.as_deref() {
            Some("smpte2084") => ("smpte2084", 1),
            Some("arib-std-b67") => ("arib-std-b67", 4),
            _ if self.has_hdr_metadata() => return Err(Error::UnsupportedHdr),
            _ => return Ok(None),
        };
        self.validate_hdr_transfer(compatibility)?;
        Ok(Some(transfer))
    }

    fn photo_hdr_transfer(&self) -> Result<Option<&'static str>, Error> {
        // SDR photos (notably HEIC exports) can carry mastering/content light level side data
        // without HDR transfer characteristics; only reject or tone-map true HDR transfers.
        let (transfer, compatibility) = match self.color_transfer.as_deref() {
            Some("smpte2084") => ("smpte2084", 1),
            Some("arib-std-b67") => ("arib-std-b67", 4),
            _ => return Ok(None),
        };
        self.validate_hdr_transfer(compatibility)?;
        Ok(Some(transfer))
    }

    fn rotation(&self) -> i32 {
        self.side_data_list
            .iter()
            .find_map(|item| item.rotation)
            .unwrap_or(0)
    }

    fn video(&self) -> Result<Video, Error> {
        let (width, height) = self.width.zip(self.height).ok_or(Error::InvalidMedia)?;
        if !(2..=4096).contains(&width) || !(2..=4096).contains(&height) {
            return Err(Error::InvalidMedia);
        }
        let nominal = fps(self.r_frame_rate.as_deref()).ok();
        let average = fps(self.avg_frame_rate.as_deref())
            .ok()
            .or(nominal)
            .ok_or(Error::InvalidMedia)?;
        let nominal = nominal.unwrap_or(average);
        Ok(Video {
            width,
            height,
            fps: average,
            nominal_fps: nominal,
            rotation: self.rotation(),
            vfr_indicated: (average - nominal).abs() > 0.01,
            color_space: self.color_space.clone(),
            color_range: self.color_range.clone(),
            color_transfer: self.color_transfer.clone(),
            color_primaries: self.color_primaries.clone(),
            frames: self
                .nb_frames
                .as_deref()
                .and_then(|count| count.parse().ok()),
        })
    }

    fn photo_dimensions(&self) -> Result<(u32, u32), Error> {
        let (width, height) = self.width.zip(self.height).ok_or(Error::InvalidMedia)?;
        if !(2..=PHOTO_MAX_SOURCE_EDGE).contains(&width)
            || !(2..=PHOTO_MAX_SOURCE_EDGE).contains(&height)
            || u64::from(width) * u64::from(height) > PHOTO_MAX_SOURCE_PIXELS
        {
            return Err(Error::InvalidMedia);
        }
        Ok((width, height))
    }
}

impl Probe {
    fn is_mov_family(&self) -> bool {
        self.format.format_name.split(',').any(|name| name == "mov")
    }

    fn is_image_pipe(&self) -> bool {
        self.format.format_name.split(',').any(|name| {
            matches!(
                name,
                "jpeg_pipe" | "png_pipe" | "webp_pipe" | "bmp_pipe" | "tiff_pipe"
            )
        })
    }

    fn streams(&self, allow_camera_metadata: bool) -> Result<(&Stream, Option<&Stream>), Error> {
        let ignore_mebx = allow_camera_metadata && self.is_mov_family();
        let mut video = None;
        let mut audio = None;
        for stream in &self.streams {
            if ignore_mebx
                && stream.codec_type == "data"
                && stream.codec_tag_string.as_deref() == Some("mebx")
            {
                continue;
            }
            if matches!(stream.codec_tag_string.as_deref(), Some("encv" | "enca"))
                || stream.codec_name.is_none()
            {
                return Err(Error::InvalidMedia);
            }
            match stream.codec_type.as_str() {
                "video"
                    if video.is_none()
                        && stream
                            .disposition
                            .as_ref()
                            .is_some_and(|d| d.attached_pic == 0) =>
                {
                    video = Some(stream);
                }
                "audio" if audio.is_none() => audio = Some(stream),
                _ => return Err(Error::InvalidMedia),
            }
        }
        Ok((video.ok_or(Error::InvalidMedia)?, audio))
    }

    fn photo_streams(&self, allow_heic_extras: bool) -> Result<&Stream, Error> {
        let is_mov = allow_heic_extras && self.is_mov_family();
        let mut video = None;
        for stream in &self.streams {
            if is_mov
                && stream.codec_type == "data"
                && stream.codec_tag_string.as_deref() == Some("mebx")
            {
                continue;
            }
            if matches!(stream.codec_tag_string.as_deref(), Some("encv" | "enca"))
                || stream.codec_name.is_none()
            {
                return Err(Error::InvalidMedia);
            }
            if stream.codec_type == "video" {
                let is_attached_pic = stream
                    .disposition
                    .as_ref()
                    .is_some_and(|d| d.attached_pic == 1);
                if is_mov && is_attached_pic {
                    continue;
                }
                if video.is_none() && !is_attached_pic {
                    video = Some(stream);
                    continue;
                }
            }
            return Err(Error::InvalidMedia);
        }
        video.ok_or(Error::InvalidMedia)
    }

    pub(crate) fn classify(
        &self,
        bytes: u64,
        profile: VideoProfile,
    ) -> Result<ClassifiedInput, Error> {
        if self.is_image_pipe() {
            if bytes > MAX_PHOTO_INPUT {
                return Err(Error::InputSize);
            }
            let stream = self.photo_streams(false)?;
            let codec = stream.codec_name.as_deref().ok_or(Error::InvalidMedia)?;
            if !matches!(codec, "mjpeg" | "png" | "webp" | "bmp" | "tiff")
                || stream
                    .nb_frames
                    .as_deref()
                    .is_some_and(|count| count != "1")
            {
                return Err(Error::InvalidMedia);
            }
            return self.build_photo_input(bytes, stream, codec);
        }

        if self.is_mov_family()
            && self
                .format
                .duration
                .as_deref()
                .and_then(|value| seconds(value).ok())
                .is_none()
            && !self.streams.iter().any(|s| s.codec_type == "audio")
            && let Ok(stream) = self.photo_streams(true)
            && let Some(codec @ ("hevc" | "av1")) = stream.codec_name.as_deref()
            && stream.nb_frames.as_deref() == Some("1")
        {
            if bytes > MAX_PHOTO_INPUT {
                return Err(Error::InputSize);
            }
            return self.build_photo_input(bytes, stream, codec);
        }

        self.input(bytes, profile.canvas())
            .map(ClassifiedInput::Video)
    }

    fn build_photo_input(
        &self,
        bytes: u64,
        stream: &Stream,
        codec: &str,
    ) -> Result<ClassifiedInput, Error> {
        let (width, height) = stream.photo_dimensions()?;
        let hdr_transfer = stream.photo_hdr_transfer()?;
        let rotation = stream.rotation();
        Ok(ClassifiedInput::Photo(PhotoInput {
            bytes,
            container: self.format.format_name.clone(),
            codec: codec.to_owned(),
            width,
            height,
            rotation,
            pix_fmt: stream.pix_fmt.clone(),
            hdr_transfer,
        }))
    }

    fn input(
        &self,
        bytes: u64,
        (canvas_width, canvas_height): (u32, u32),
    ) -> Result<VideoInput, Error> {
        let duration = seconds(self.format.duration.as_deref().ok_or(Error::InvalidMedia)?)?;
        if duration > MAX_SECONDS {
            return Err(Error::Duration);
        }
        let (video, audio) = self.streams(true)?;
        let info = video.video()?;
        let hdr_transfer = video.hdr_transfer()?;
        let video_duration = video
            .duration
            .as_deref()
            .and_then(|value| seconds(value).ok());
        if video_duration.is_some_and(|duration| duration > MAX_SECONDS) {
            return Err(Error::Duration);
        }
        let (width, height) = if info.rotation.rem_euclid(180) == 90 {
            (info.height, info.width)
        } else {
            (info.width, info.height)
        };
        Ok(VideoInput {
            bytes,
            duration,
            video_duration,
            container: self.format.format_name.clone(),
            video_codec: video.codec_name.clone().ok_or(Error::InvalidMedia)?,
            audio_codec: audio.and_then(|stream| stream.codec_name.clone()),
            hdr_transfer,
            enlarged: width < canvas_width && height < canvas_height,
            video: info,
        })
    }

    pub(crate) fn validate_clip(
        &self,
        (width, height): (u32, u32),
        expected_audio: bool,
        tone_mapped: bool,
    ) -> Result<(f64, f64, Video), Error> {
        let duration = seconds(self.format.duration.as_deref().ok_or(Error::InvalidMedia)?)?;
        let (video, audio) = self.streams(false)?;
        let info = video.video()?;
        let video_duration = seconds(video.duration.as_deref().ok_or(Error::InvalidMedia)?)?;
        if !self
            .format
            .format_name
            .split(',')
            .any(|format| format == "mp4")
            || duration > CLIP_SECONDS
            || video_duration > CLIP_SECONDS
            || video.codec_name.as_deref() != Some("h264")
            || video.profile.as_deref() != Some("High")
            || video.level != Some(40)
            || video.pix_fmt.as_deref() != Some("yuv420p")
            || matches!(
                video.color_transfer.as_deref(),
                Some("smpte2084" | "arib-std-b67")
            )
            || video.has_hdr_metadata()
            || (tone_mapped
                && (video.color_transfer.as_deref() != Some("bt709")
                    || video.color_primaries.as_deref() != Some("bt709")
                    || video.color_space.as_deref() != Some("bt709")
                    || video.color_range.as_deref() != Some("tv")))
            || video
                .sample_aspect_ratio
                .as_deref()
                .is_some_and(|ratio| ratio != "1:1")
            || (info.width, info.height, info.rotation) != (width, height, 0)
            || info.fps > 30.1
            || audio.is_some() != expected_audio
            || audio.is_some_and(|stream| {
                stream.codec_name.as_deref() != Some("aac")
                    || stream.sample_rate.as_deref() != Some("44100")
                    || stream.channels != Some(2)
            })
        {
            return Err(Error::InvalidMedia);
        }
        Ok((duration, video_duration, info))
    }

    pub(crate) fn validate_photo(
        &self,
        max_edge: u32,
        tone_mapped: bool,
    ) -> Result<(u32, u32, String), Error> {
        if self.format.format_name != "jpeg_pipe" {
            return Err(Error::InvalidMedia);
        }
        let stream = self.photo_streams(false)?;
        let (width, height) = stream.width.zip(stream.height).ok_or(Error::InvalidMedia)?;
        let pix_fmt = stream.pix_fmt.as_deref().ok_or(Error::InvalidMedia)?;
        if stream.codec_name.as_deref() != Some("mjpeg")
            || pix_fmt != "yuvj420p"
            || !(2..=max_edge).contains(&width)
            || !(2..=max_edge).contains(&height)
            || stream.rotation() != 0
            || stream
                .sample_aspect_ratio
                .as_deref()
                .is_some_and(|ratio| ratio != "1:1")
            || matches!(
                stream.color_transfer.as_deref(),
                Some("smpte2084" | "arib-std-b67")
            )
            || stream.has_hdr_metadata()
            || (tone_mapped
                && (stream.color_primaries.as_deref() == Some("bt2020")
                    || matches!(stream.color_space.as_deref(), Some("bt2020nc" | "bt2020c"))))
        {
            return Err(Error::InvalidMedia);
        }
        Ok((width, height, pix_fmt.to_owned()))
    }
}

pub(crate) fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

pub(crate) async fn probe(tools: &Tools, directory: &Path, file: &str) -> Result<Probe, Error> {
    probe_with(tools, directory, file, ALL_INPUT_FORMATS, None).await
}

pub(crate) async fn probe_jpeg(
    tools: &Tools,
    directory: &Path,
    file: &str,
) -> Result<Probe, Error> {
    probe_with(tools, directory, file, "jpeg_pipe", Some("jpeg_pipe")).await
}

async fn probe_with(
    tools: &Tools,
    directory: &Path,
    file: &str,
    format_whitelist: &str,
    forced_format: Option<&str>,
) -> Result<Probe, Error> {
    let mut command = args(&[
        "-v",
        "error",
        "-max_alloc",
        PHOTO_ALLOC_LIMIT,
        "-protocol_whitelist",
        "file",
        "-format_whitelist",
        format_whitelist,
    ]);
    if let Some(format) = forced_format {
        command.extend(args(&["-f", format]));
    }
    command.extend(args(&[
        "-show_entries",
        concat!(
            "format=format_name,duration:",
            "stream=codec_type,codec_name,codec_tag_string,profile,level,width,height,",
            "pix_fmt,sample_aspect_ratio,avg_frame_rate,r_frame_rate,color_space,color_range,",
            "color_transfer,color_primaries,sample_rate,channels,duration,nb_frames:",
            "stream_disposition=attached_pic:",
            "stream_side_data=rotation,side_data_type,dv_profile,dv_bl_signal_compatibility_id"
        ),
        "-of",
        "json",
        "-i",
        file,
    ]));
    let output = tool::run(
        &tools.ffprobe,
        directory,
        &command,
        Duration::from_secs(20),
        Error::Probe,
    )
    .await?;
    serde_json::from_slice(&output).map_err(|_| Error::Probe)
}

fn ffmpeg_video_input(file: &str) -> Vec<String> {
    args(&[
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-xerror",
        "-max_alloc",
        "134217728",
        "-threads",
        "2",
        "-err_detect",
        "explode",
        "-protocol_whitelist",
        "file",
        "-format_whitelist",
        VIDEO_FORMATS,
        "-i",
        file,
        "-map",
        "0:v:0",
        "-map",
        "0:a:0?",
    ])
}

fn ffmpeg_photo_input(
    file: &str,
    protocol_whitelist: &str,
    format_whitelist: &str,
    forced_format: Option<&str>,
    max_alloc: &str,
) -> Vec<String> {
    let mut command = args(&[
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-xerror",
        "-max_alloc",
        max_alloc,
        "-threads",
        "2",
        "-err_detect",
        "explode",
        "-protocol_whitelist",
        protocol_whitelist,
        "-format_whitelist",
        format_whitelist,
    ]);
    if let Some(format) = forced_format {
        command.extend(args(&["-f", format]));
    }
    command.extend(args(&["-i", file, "-map", "0:v:0"]));
    command
}

pub(crate) async fn decode(
    tools: &Tools,
    directory: &Path,
    file: &str,
    expected_duration: f64,
    expected_frames: Option<u64>,
) -> Result<(), Error> {
    let mut command = ffmpeg_video_input(file);
    command.extend(args(&[
        "-t",
        "602",
        "-fps_mode",
        "passthrough",
        "-progress",
        "pipe:1",
        "-f",
        "null",
        "-",
    ]));
    let output = tool::run(
        &tools.ffmpeg,
        directory,
        &command,
        Duration::from_secs(180),
        Error::Decode,
    )
    .await?;
    let progress = std::str::from_utf8(&output).map_err(|_| Error::Decode)?;
    let decoded_frames = progress
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("frame="))
        .and_then(|count| count.trim().parse::<u64>().ok())
        .filter(|count| *count > 0)
        .ok_or(Error::Decode)?;
    let decoded_seconds = progress
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("out_time_us="))
        .and_then(|value| seconds(value).ok())
        .ok_or(Error::Decode)?
        / 1_000_000.0;
    if expected_frames.is_some_and(|count| count != decoded_frames)
        || (decoded_seconds - expected_duration).abs() > 0.5
    {
        return Err(Error::Decode);
    }
    Ok(())
}

pub(crate) async fn decode_photo(
    tools: &Tools,
    directory: &Path,
    file: &str,
    jpeg_output: bool,
) -> Result<(), Error> {
    let mut command = if jpeg_output {
        ffmpeg_photo_input(file, "file", "jpeg_pipe", Some("jpeg_pipe"), "134217728")
    } else {
        ffmpeg_photo_input(file, "file", ALL_INPUT_FORMATS, None, PHOTO_ALLOC_LIMIT)
    };
    command.extend(args(&[
        "-frames:v",
        "2",
        "-progress",
        "pipe:1",
        "-f",
        "null",
        "-",
    ]));
    let output = tool::run(
        &tools.ffmpeg,
        directory,
        &command,
        Duration::from_secs(60),
        Error::Decode,
    )
    .await?;
    let progress = std::str::from_utf8(&output).map_err(|_| Error::Decode)?;
    let decoded_frames = progress
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix("frame="))
        .and_then(|count| count.trim().parse::<u64>().ok())
        .ok_or(Error::Decode)?;
    if decoded_frames != 1 {
        return Err(Error::Decode);
    }
    Ok(())
}

pub(crate) fn encode_args(input: &VideoInput, profile: VideoProfile, retry: bool) -> Vec<String> {
    let mut command = ffmpeg_video_input("source");
    let (width, height) = profile.canvas();
    let mut filters = format!(
        "scale={width}:{height}:force_original_aspect_ratio=decrease:force_divisible_by=2:reset_sar=1",
    );
    if let Some(transfer) = input.hdr_transfer {
        filters.push_str(":in_primaries=bt2020:in_color_matrix=bt2020nc:in_transfer=");
        filters.push_str(transfer);
        filters.push_str(
            ":out_primaries=bt709:out_color_matrix=bt709:out_transfer=bt709:out_range=tv:intent=perceptual,format=yuv420p,sidedata=mode=delete",
        );
    }
    filters.push_str(&format!(",pad={width}:{height}:(ow-iw)/2:(oh-ih)/2"));
    if input.video.fps.max(input.video.nominal_fps) > CAP_FPS + 0.001 {
        filters.push_str(",fps=30000/1001");
    }
    let (crf, maxrate, bufsize) = if retry {
        ("27", "3000k", "4500k")
    } else {
        let crf = match profile {
            VideoProfile::Hd29 => "23",
            VideoProfile::Status720 => "20",
        };
        (crf, "3800k", "5700k")
    };
    command.extend(args(&[
        "-map_metadata",
        "-1",
        "-map_metadata:s",
        "-1",
        "-map_chapters",
        "-1",
        "-filter_threads",
        "2",
        "-vf",
        &filters,
        "-fps_mode",
        "vfr",
        "-c:v",
        "libx264",
        "-threads:v",
        "2",
        "-profile:v",
        "high",
        "-level:v",
        "4.0",
        "-pix_fmt",
        "yuv420p",
        "-preset",
        "medium",
        "-crf",
        crf,
        "-maxrate",
        maxrate,
        "-bufsize",
        bufsize,
        "-g",
        "250",
        "-flags",
        "+cgop",
        "-force_key_frames",
        "expr:gte(t,n_forced*28)",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        "-ac",
        "2",
        "-ar",
        "44100",
        "-t",
        "602",
        "-f",
        "segment",
        "-segment_time",
        "28",
        "-segment_time_delta",
        "0.017",
        "-segment_format",
        "mp4",
        "-segment_format_options",
        "movflags=+faststart",
        "-reset_timestamps",
        "1",
        "clip-%03d.mp4",
    ]));
    command
}

pub(crate) fn photo_encode_args(input: &PhotoInput, max_edge: u32, quality: u32) -> Vec<String> {
    let mut command =
        ffmpeg_photo_input("source", "file", ALL_INPUT_FORMATS, None, PHOTO_ALLOC_LIMIT);
    let filters = if has_alpha(input.pix_fmt.as_deref()) {
        format!(
            "format=rgba,scale=w='min({max_edge},iw)':h='min({max_edge},ih)':force_original_aspect_ratio=decrease:flags=lanczos:reset_sar=1,split[a][b];[a]drawbox=color=white@1:t=fill[bg];[bg][b]overlay=format=auto,sidedata=mode=delete,format=yuvj420p"
        )
    } else {
        let mut chain = format!(
            "scale=w='min({max_edge},iw)':h='min({max_edge},ih)':force_original_aspect_ratio=decrease:flags=lanczos:reset_sar=1"
        );
        if let Some(transfer) = input.hdr_transfer {
            chain.push_str(":in_primaries=bt2020:in_color_matrix=bt2020nc:in_transfer=");
            chain.push_str(transfer);
            chain.push_str(
                ":out_primaries=bt709:out_color_matrix=bt709:out_transfer=bt709:out_range=pc:intent=perceptual",
            );
        }
        chain.push_str(",sidedata=mode=delete,format=yuvj420p");
        chain
    };
    let quality = quality.to_string();
    command.extend(args(&[
        "-map_metadata",
        "-1",
        "-map_metadata:s",
        "-1",
        "-map_chapters",
        "-1",
        "-filter_threads",
        "2",
        "-vf",
        &filters,
        "-frames:v",
        "1",
        "-c:v",
        "mjpeg",
        "-q:v",
        &quality,
        "-huffman",
        "optimal",
        "-f",
        "image2",
        "-update",
        "1",
        "photo-000.jpg",
    ]));
    command
}

pub(crate) async fn photo_thumbnail(
    tools: &Tools,
    directory: &Path,
    file: &str,
) -> Result<String, Error> {
    let mut command = ffmpeg_photo_input(
        file,
        "file,pipe",
        "jpeg_pipe",
        Some("jpeg_pipe"),
        "134217728",
    );
    command.extend(args(&[
        "-map_metadata",
        "-1",
        "-vf",
        "scale=32:-2:flags=area,format=yuvj420p",
        "-frames:v",
        "1",
        "-c:v",
        "mjpeg",
        "-q:v",
        "10",
        "-huffman",
        "optimal",
        "-f",
        "mjpeg",
        "pipe:1",
    ]));
    let bytes = tool::run(
        &tools.ffmpeg,
        directory,
        &command,
        Duration::from_secs(20),
        Error::InvalidOutput,
    )
    .await?;
    if bytes.is_empty() || bytes.len() > 8192 || !bytes.starts_with(&[0xff, 0xd8]) {
        return Err(Error::InvalidOutput);
    }
    Ok(encode_base64(&bytes))
}

fn encode_base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        let triple = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        output.push(TABLE[((triple >> 18) & 0x3f) as usize] as char);
        output.push(TABLE[((triple >> 12) & 0x3f) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn selects_video_profile_and_rejects_unknown_names() {
        assert_eq!(VideoProfile::default(), VideoProfile::Hd29);
        assert_eq!("hd29".parse(), Ok(VideoProfile::Hd29));
        assert_eq!("status720".parse(), Ok(VideoProfile::Status720));
        assert_eq!("720p".parse::<VideoProfile>(), Err("invalid_video_profile"));
        assert!("".parse::<VideoProfile>().is_err());
    }

    #[test]
    fn builds_profile_specific_video_arguments() {
        let probe: Probe = serde_json::from_value(json!({
            "format": {"format_name": "mov,mp4", "duration": "1"},
            "streams": [{
                "codec_type": "video", "codec_name": "h264", "width": 720, "height": 1280,
                "avg_frame_rate": "60/1", "duration": "1", "disposition": {"attached_pic": 0}
            }]
        }))
        .unwrap();
        for (profile, canvas, crf, enlarged) in [
            (VideoProfile::Hd29, "1080:1920", "23", true),
            (VideoProfile::Status720, "720:1280", "20", false),
        ] {
            let ClassifiedInput::Video(input) = probe.classify(1000, profile).unwrap() else {
                panic!("expected video");
            };
            assert_eq!(input.enlarged, enlarged);
            for (retry, crf, rate, buffer) in [
                (false, crf, "3800k", "5700k"),
                (true, "27", "3000k", "4500k"),
            ] {
                let args = encode_args(&input, profile, retry);
                let value =
                    |key| args[args.iter().position(|arg| arg == key).unwrap() + 1].as_str();
                assert_eq!(
                    value("-vf"),
                    format!(
                        "scale={canvas}:force_original_aspect_ratio=decrease:force_divisible_by=2:reset_sar=1,pad={canvas}:(ow-iw)/2:(oh-ih)/2,fps=30000/1001"
                    )
                );
                assert_eq!(value("-crf"), crf);
                assert_eq!(value("-maxrate"), rate);
                assert_eq!(value("-bufsize"), buffer);
                assert_eq!(value("-preset"), "medium");
            }
        }
    }

    #[test]
    fn validates_square_pixels_with_or_without_an_explicit_ratio() {
        let mut probe: Probe = serde_json::from_value(json!({
            "format": {"format_name": "mov,mp4", "duration": "1"},
            "streams": [{
                "codec_type": "video", "codec_name": "h264", "profile": "High", "level": 40,
                "width": 1080, "height": 1920, "pix_fmt": "yuv420p", "avg_frame_rate": "25/1",
                "duration": "1", "disposition": {"attached_pic": 0}
            }]
        }))
        .unwrap();
        let hd29 = VideoProfile::Hd29.canvas();
        let status720 = VideoProfile::Status720.canvas();
        assert!(probe.validate_clip(hd29, false, false).is_ok());
        assert_eq!(
            probe.validate_clip(status720, false, false).unwrap_err(),
            Error::InvalidMedia
        );
        probe.streams[0].sample_aspect_ratio = Some("1:1".into());
        assert!(probe.validate_clip(hd29, false, false).is_ok());
        probe.streams[0].width = Some(status720.0);
        probe.streams[0].height = Some(status720.1);
        assert!(probe.validate_clip(status720, false, false).is_ok());
        assert_eq!(
            probe.validate_clip(hd29, false, false).unwrap_err(),
            Error::InvalidMedia
        );
        probe.streams[0].sample_aspect_ratio = Some("4:3".into());
        assert_eq!(
            probe.validate_clip(status720, false, false).unwrap_err(),
            Error::InvalidMedia
        );
    }

    #[test]
    fn accepts_only_matching_dolby_vision_base_layers() {
        for (transfer, compatibility) in [("arib-std-b67", 4), ("smpte2084", 1)] {
            let probe: Probe = serde_json::from_value(json!({
                "format": {"format_name": "mov", "duration": "1"},
                "streams": [{
                    "codec_type": "video",
                    "color_transfer": transfer, "color_primaries": "bt2020", "color_space": "bt2020nc",
                    "side_data_list": [{"side_data_type": "DOVI configuration record",
                        "dv_profile": 8, "dv_bl_signal_compatibility_id": compatibility}]
                }]
            }))
            .unwrap();
            let mut streams = probe.streams;
            let video = &mut streams[0];
            assert_eq!(video.hdr_transfer().unwrap(), Some(transfer));
            video.side_data_list[0].dv_bl_signal_compatibility_id = Some(0);
            assert_eq!(video.hdr_transfer().unwrap_err(), Error::UnsupportedHdr);
            video.side_data_list[0].dv_profile = Some(5);
            video.side_data_list[0].dv_bl_signal_compatibility_id = Some(compatibility);
            assert_eq!(video.hdr_transfer().unwrap_err(), Error::UnsupportedHdr);
        }
    }

    #[test]
    fn classifies_sdr_heic_with_content_light_metadata_and_hdr_heic() {
        let sdr_heic: Probe = serde_json::from_value(json!({
            "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2"},
            "streams": [
                {
                    "codec_type": "video", "codec_name": "hevc", "width": 4032, "height": 3024,
                    "pix_fmt": "yuvj420p", "nb_frames": "1", "disposition": {"attached_pic": 0},
                    "side_data_list": [{"side_data_type": "Content light level metadata"}]
                },
                {
                    "codec_type": "video", "codec_name": "hevc", "width": 320, "height": 240,
                    "disposition": {"attached_pic": 1}
                }
            ]
        }))
        .unwrap();
        let ClassifiedInput::Photo(photo) =
            sdr_heic.classify(1_000_000, VideoProfile::Hd29).unwrap()
        else {
            panic!("expected HEIC photo");
        };
        assert_eq!(photo.codec, "hevc");
        assert_eq!(photo.hdr_transfer, None);
        assert_eq!((photo.width, photo.height), (4032, 3024));

        let hdr_heic: Probe = serde_json::from_value(json!({
            "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2"},
            "streams": [{
                "codec_type": "video", "codec_name": "hevc", "width": 4032, "height": 3024,
                "pix_fmt": "yuv420p10le", "nb_frames": "1",
                "color_transfer": "arib-std-b67", "color_primaries": "bt2020", "color_space": "bt2020nc",
                "disposition": {"attached_pic": 0}
            }]
        }))
        .unwrap();
        let ClassifiedInput::Photo(hdr) = hdr_heic.classify(1_000_000, VideoProfile::Hd29).unwrap()
        else {
            panic!("expected HDR HEIC photo");
        };
        assert_eq!(hdr.hdr_transfer, Some("arib-std-b67"));

        let bad_hdr_heic: Probe = serde_json::from_value(json!({
            "format": {"format_name": "mov,mp4,m4a,3gp,3g2,mj2"},
            "streams": [{
                "codec_type": "video", "codec_name": "hevc", "width": 4032, "height": 3024,
                "pix_fmt": "yuv420p10le", "nb_frames": "1",
                "color_transfer": "arib-std-b67", "color_primaries": "bt709", "color_space": "bt709",
                "disposition": {"attached_pic": 0}
            }]
        }))
        .unwrap();
        assert_eq!(
            bad_hdr_heic.classify(1_000_000, VideoProfile::Hd29).err(),
            Some(Error::UnsupportedHdr)
        );
    }

    #[test]
    fn validates_base64_and_alpha_detection() {
        assert_eq!(encode_base64(b"Man"), "TWFu");
        assert_eq!(encode_base64(b"Ma"), "TWE=");
        assert_eq!(encode_base64(b"M"), "TQ==");
        assert!(has_alpha(Some("rgba")));
        assert!(has_alpha(Some("yuva420p")));
        assert!(!has_alpha(Some("yuvj420p")));
        assert!(!has_alpha(None));
    }
}
