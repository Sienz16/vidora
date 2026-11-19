use std::{path::Path, time::Duration};

use serde::{Deserialize, Serialize};

use crate::{Error, Input, Tools, tool};

const INPUT_FORMATS: &str = "mov,mp4,m4a,3gp,3g2,mj2,matroska,webm,avi,mpegts";
pub(crate) const MAX_SECONDS: f64 = 600.0;
pub(crate) const CLIP_SECONDS: f64 = 29.0;
pub(crate) const CLIP_CAP: u64 = 15_800_000;
pub(crate) const MAX_CLIPS: usize = 32;
const CAP_FPS: f64 = 30_000.0 / 1001.0;

#[derive(Debug, Deserialize)]
pub(crate) struct Probe {
    pub(crate) streams: Vec<Stream>,
    pub(crate) format: Format,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Format {
    pub(crate) format_name: String,
    duration: String,
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

impl Stream {
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
        let rotation = self
            .side_data_list
            .iter()
            .find_map(|item| item.rotation)
            .unwrap_or(0);
        Ok(Video {
            width,
            height,
            fps: average,
            nominal_fps: nominal,
            rotation,
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
}

impl Probe {
    fn streams(&self) -> Result<(&Stream, Option<&Stream>), Error> {
        let mut video = None;
        let mut audio = None;
        for stream in &self.streams {
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

    pub(crate) fn input(&self, bytes: u64) -> Result<Input, Error> {
        let duration = seconds(&self.format.duration)?;
        if duration > MAX_SECONDS {
            return Err(Error::Duration);
        }
        let (video, audio) = self.streams()?;
        let info = video.video()?;
        if matches!(
            video.color_transfer.as_deref(),
            Some("smpte2084" | "arib-std-b67")
        ) || video.side_data_list.iter().any(|item| {
            matches!(
                item.side_data_type.as_str(),
                "DOVI configuration record"
                    | "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"
                    | "Mastering display metadata"
            )
        }) {
            return Err(Error::UnsupportedHdr);
        }
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
        Ok(Input {
            bytes,
            duration,
            video_duration,
            container: self.format.format_name.clone(),
            video_codec: video.codec_name.clone().ok_or(Error::InvalidMedia)?,
            audio_codec: audio.and_then(|stream| stream.codec_name.clone()),
            enlarged: width < 1080 && height < 1920,
            video: info,
        })
    }

    pub(crate) fn validate_clip(&self, expected_audio: bool) -> Result<(f64, f64, Video), Error> {
        let duration = seconds(&self.format.duration)?;
        let (video, audio) = self.streams()?;
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
            || video.sample_aspect_ratio.as_deref() != Some("1:1")
            || (info.width, info.height, info.rotation) != (1080, 1920, 0)
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
}

pub(crate) fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).into()).collect()
}

pub(crate) async fn probe(tools: &Tools, directory: &Path, file: &str) -> Result<Probe, Error> {
    let output = tool::run(
        &tools.ffprobe,
        directory,
        &args(&[
            "-v",
            "error",
            "-max_alloc",
            "134217728",
            "-protocol_whitelist",
            "file",
            "-format_whitelist",
            INPUT_FORMATS,
            "-show_entries",
            concat!(
                "format=format_name,duration:",
                "stream=codec_type,codec_name,codec_tag_string,profile,level,width,height,",
                "pix_fmt,sample_aspect_ratio,avg_frame_rate,r_frame_rate,color_space,color_range,",
                "color_transfer,color_primaries,sample_rate,channels,duration,nb_frames:",
                "stream_disposition=attached_pic:stream_side_data=rotation,side_data_type"
            ),
            "-of",
            "json",
            "-i",
            file,
        ]),
        Duration::from_secs(20),
        Error::Probe,
    )
    .await?;
    serde_json::from_slice(&output).map_err(|_| Error::Probe)
}

fn ffmpeg_input(file: &str) -> Vec<String> {
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
        INPUT_FORMATS,
        "-i",
        file,
        "-map",
        "0:v:0",
        "-map",
        "0:a:0?",
    ])
}

pub(crate) async fn decode(
    tools: &Tools,
    directory: &Path,
    file: &str,
    expected_duration: f64,
    expected_frames: Option<u64>,
) -> Result<(), Error> {
    let mut command = ffmpeg_input(file);
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

pub(crate) fn encode_args(input: &Input, retry: bool) -> Vec<String> {
    let mut command = ffmpeg_input("source");
    let mut filters = String::from(
        "scale=1080:1920:force_original_aspect_ratio=decrease:force_divisible_by=2:reset_sar=1",
    );
    filters.push_str(",pad=1080:1920:(ow-iw)/2:(oh-ih)/2");
    if input.video.fps.max(input.video.nominal_fps) > CAP_FPS + 0.001 {
        filters.push_str(",fps=30000/1001");
    }
    let (crf, maxrate, bufsize) = if retry {
        ("27", "3000k", "4500k")
    } else {
        ("23", "3800k", "5700k")
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
