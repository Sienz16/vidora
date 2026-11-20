mod media;
mod tool;

use std::{
    env, fmt,
    fs::Permissions,
    io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use media::{
    CLIP_CAP, ClassifiedInput, MAX_CLIPS, PHOTO_CAP, PHOTO_MIN_LONG_EDGE, PHOTO_QUALITIES,
    PHOTO_TARGET_LONG_EDGE, Video,
};
use serde::Serialize;
use tempfile::TempDir;
use tokio::{
    fs::{self, File},
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    time::timeout,
};

pub const MAX_INPUT: u64 = 300 * 1024 * 1024;
pub const MAX_PHOTO_INPUT: u64 = 50 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoProfile {
    #[default]
    Hd29,
    Status720,
}

impl std::str::FromStr for VideoProfile {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "hd29" => Ok(Self::Hd29),
            "status720" => Ok(Self::Status720),
            _ => Err("invalid_video_profile"),
        }
    }
}

impl VideoProfile {
    pub fn from_env() -> Result<Self, &'static str> {
        match env::var("VIDORA_VIDEO_PROFILE") {
            Ok(value) => value.parse(),
            Err(env::VarError::NotPresent) => Ok(Self::default()),
            Err(env::VarError::NotUnicode(_)) => Err("invalid_video_profile"),
        }
    }

    fn canvas(self) -> (u32, u32) {
        match self {
            Self::Hd29 => (1080, 1920),
            Self::Status720 => (720, 1280),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Hd29 => "hd29",
            Self::Status720 => "status720",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Video,
    Photo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    Empty,
    InputSize,
    SizeMismatch,
    Io,
    ToolUnavailable,
    ToolOutput,
    Timeout,
    Probe,
    Decode,
    InvalidMedia,
    UnsupportedHdr,
    Duration,
    Encode,
    ClipSize,
    InvalidOutput,
}

impl From<io::Error> for Error {
    fn from(_: io::Error) -> Self {
        Self::Io
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for Error {}

#[derive(Debug)]
pub struct Tools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

impl Tools {
    pub fn from_path() -> Result<Self, Error> {
        let path = env::var_os("PATH").ok_or(Error::ToolUnavailable)?;
        let find = |name: &str| {
            env::split_paths(&path)
                .filter(|dir| dir.is_absolute())
                .map(|dir| dir.join(name))
                .find(|file| file.is_file())
                .ok_or(Error::ToolUnavailable)
        };
        Ok(Self {
            ffmpeg: find("ffmpeg")?,
            ffprobe: find("ffprobe")?,
        })
    }

    pub async fn check(&self, directory: &Path) -> Result<(), Error> {
        for executable in [&self.ffmpeg, &self.ffprobe] {
            tool::run(
                executable,
                directory,
                &media::args(&["-version"]),
                Duration::from_secs(5),
                Error::ToolUnavailable,
            )
            .await?;
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct VideoInput {
    bytes: u64,
    duration: f64,
    video_duration: Option<f64>,
    container: String,
    video_codec: String,
    audio_codec: Option<String>,
    hdr_transfer: Option<&'static str>,
    enlarged: bool,
    video: Video,
}

#[derive(Debug, Serialize)]
pub(crate) struct PhotoInput {
    bytes: u64,
    container: String,
    codec: String,
    width: u32,
    height: u32,
    rotation: i32,
    #[serde(skip)]
    pix_fmt: Option<String>,
    hdr_transfer: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum Input {
    Video(VideoInput),
    Photo(PhotoInput),
}

#[derive(Debug, Serialize)]
pub struct Clip {
    pub name: String,
    pub bytes: u64,
    pub duration: f64,
    video_duration: f64,
    video: Video,
    audio: bool,
}

#[derive(Debug, Serialize)]
pub struct Photo {
    pub name: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pix_fmt: String,
    quality: u32,
    downscaled: bool,
}

#[derive(Debug, Serialize)]
pub struct Record {
    pub job_id: Option<String>,
    pub kind: Option<MediaKind>,
    pub profile: Option<&'static str>,
    pub ffmpeg_version: Option<String>,
    input: Option<Input>,
    pub encode_attempts: Vec<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub clips: Vec<Clip>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub photos: Vec<Photo>,
    pub elapsed_ms: u128,
    pub error: Option<Error>,
}

#[derive(Debug)]
pub struct Job {
    work: TempDir,
    record: Record,
    photo_thumbnail: Option<String>,
}

impl Job {
    pub fn record(&self) -> &Record {
        &self.record
    }

    pub fn kind(&self) -> MediaKind {
        self.record.kind.expect("completed job has media kind")
    }

    pub fn photo_thumbnail(&self) -> Option<&str> {
        self.photo_thumbnail.as_deref()
    }

    /// Copy validated outputs into a new private directory below a trusted destination.
    /// Working media is removed before the export is published.
    pub async fn export(mut self, destination: &Path) -> Result<PathBuf, Box<Record>> {
        let result = async {
            let export = tempfile::Builder::new()
                .prefix("vidora-")
                .permissions(Permissions::from_mode(0o700))
                .tempdir_in(destination)?;
            for name in self
                .record
                .clips
                .iter()
                .map(|clip| clip.name.as_str())
                .chain(self.record.photos.iter().map(|photo| photo.name.as_str()))
            {
                fs::copy(self.work.path().join(name), export.path().join(name)).await?;
            }
            self.work.close()?;
            Ok::<_, io::Error>(export.keep())
        }
        .await;
        result.map_err(|_| {
            self.record.error = Some(Error::Io);
            Box::new(self.record)
        })
    }
}

async fn receive(
    reader: impl AsyncRead + Unpin,
    declared_size: Option<u64>,
    directory: &Path,
    max_size: u64,
) -> Result<u64, Error> {
    if declared_size.is_some_and(|bytes| bytes > max_size) {
        return Err(Error::InputSize);
    }
    let mut output = File::create_new(directory.join("source")).await?;
    let bytes = tokio::io::copy(&mut reader.take(max_size + 1), &mut output).await?;
    output.flush().await?;
    if bytes > max_size {
        return Err(Error::InputSize);
    }
    if bytes == 0 {
        return Err(Error::Empty);
    }
    if declared_size.is_some_and(|declared| declared != bytes) {
        return Err(Error::SizeMismatch);
    }
    Ok(bytes)
}

#[derive(Debug)]
pub struct Processor {
    tools: Tools,
    root: PathBuf,
    video_profile: VideoProfile,
    clip_cap: u64,
    photo_cap: u64,
}

impl Processor {
    /// The work root is a trusted directory. Share one processor per worker.
    pub fn new(tools: Tools, root: &Path) -> Result<Self, Error> {
        Ok(Self {
            tools,
            root: root.canonicalize()?,
            video_profile: VideoProfile::default(),
            clip_cap: CLIP_CAP,
            photo_cap: PHOTO_CAP,
        })
    }

    pub fn with_video_profile(mut self, profile: VideoProfile) -> Self {
        self.video_profile = profile;
        self
    }

    pub async fn process(
        &mut self,
        reader: impl AsyncRead + Unpin,
        declared_size: Option<u64>,
    ) -> Result<Job, Box<Record>> {
        let started = Instant::now();
        let mut record = Record {
            job_id: None,
            kind: None,
            profile: None,
            ffmpeg_version: None,
            input: None,
            encode_attempts: Vec::new(),
            clips: Vec::new(),
            photos: Vec::new(),
            elapsed_ms: 0,
            error: None,
        };
        let result = async {
            let work = tempfile::Builder::new()
                .prefix("job-")
                .rand_bytes(24)
                .permissions(Permissions::from_mode(0o700))
                .tempdir_in(&self.root)?;
            record.job_id = work
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned);
            let result = timeout(
                Duration::from_secs(1200),
                self.process_in(work.path(), reader, declared_size, &mut record),
            )
            .await
            .unwrap_or(Err(Error::Timeout));
            match result {
                Ok(thumbnail) => Ok((work, thumbnail)),
                Err(error) => {
                    work.close()?;
                    Err(error)
                }
            }
        }
        .await;
        record.elapsed_ms = started.elapsed().as_millis();
        match result {
            Ok((work, photo_thumbnail)) => Ok(Job {
                work,
                record,
                photo_thumbnail,
            }),
            Err(error) => {
                record.error = Some(error);
                Err(Box::new(record))
            }
        }
    }

    async fn process_in(
        &self,
        directory: &Path,
        reader: impl AsyncRead + Unpin,
        declared_size: Option<u64>,
        record: &mut Record,
    ) -> Result<Option<String>, Error> {
        let bytes = timeout(
            Duration::from_secs(90),
            receive(reader, declared_size, directory, MAX_INPUT),
        )
        .await
        .map_err(|_| Error::Timeout)??;
        let version = tool::run(
            &self.tools.ffmpeg,
            directory,
            &media::args(&["-version"]),
            Duration::from_secs(5),
            Error::ToolUnavailable,
        )
        .await?;
        record.ffmpeg_version = Some(
            std::str::from_utf8(&version)
                .ok()
                .and_then(|value| value.lines().next())
                .filter(|line| line.starts_with("ffmpeg version "))
                .ok_or(Error::ToolUnavailable)?
                .to_owned(),
        );
        match media::probe(&self.tools, directory, "source")
            .await?
            .classify(bytes, self.video_profile)?
        {
            ClassifiedInput::Video(video) => {
                record.kind = Some(MediaKind::Video);
                record.profile = Some(self.video_profile.name());
                record.input = Some(Input::Video(video));
                self.process_video(directory, record).await?;
                Ok(None)
            }
            ClassifiedInput::Photo(photo) => {
                record.kind = Some(MediaKind::Photo);
                record.profile = Some("photo_hd");
                record.input = Some(Input::Photo(photo));
                let thumbnail = self.process_photo(directory, record).await?;
                Ok(Some(thumbnail))
            }
        }
    }

    async fn process_video(&self, directory: &Path, record: &mut Record) -> Result<(), Error> {
        let Some(Input::Video(input)) = record.input.as_ref() else {
            return Err(Error::InvalidMedia);
        };
        let expected_duration = input.video_duration.unwrap_or(input.duration);
        media::decode(
            &self.tools,
            directory,
            "source",
            input.duration,
            input.video.frames,
        )
        .await?;
        for retry in [false, true] {
            let args = media::encode_args(input, self.video_profile, retry);
            record.encode_attempts.push(args);
            tool::run(
                &self.tools.ffmpeg,
                directory,
                record.encode_attempts.last().unwrap(),
                Duration::from_secs(480),
                Error::Encode,
            )
            .await?;
            match self
                .inspect_clips(directory, input, expected_duration)
                .await
            {
                Ok(clips) => {
                    fs::remove_file(directory.join("source")).await?;
                    record.clips = clips;
                    return Ok(());
                }
                Err(Error::ClipSize) if !retry => {
                    for path in clip_paths(directory).await? {
                        fs::remove_file(path).await?;
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(Error::ClipSize)
    }

    async fn process_photo(&self, directory: &Path, record: &mut Record) -> Result<String, Error> {
        let Some(Input::Photo(input)) = record.input.as_ref() else {
            return Err(Error::InvalidMedia);
        };
        media::decode_photo(&self.tools, directory, "source", false).await?;
        let (src_w, src_h) = if input.rotation.rem_euclid(180) == 90 {
            (input.height, input.width)
        } else {
            (input.width, input.height)
        };
        let mut max_edge = PHOTO_TARGET_LONG_EDGE;
        for _ in 0..6 {
            if max_edge < PHOTO_MIN_LONG_EDGE {
                break;
            }
            for &quality in &PHOTO_QUALITIES {
                let args = media::photo_encode_args(input, max_edge, quality);
                record.encode_attempts.push(args);
                tool::run(
                    &self.tools.ffmpeg,
                    directory,
                    record.encode_attempts.last().unwrap(),
                    Duration::from_secs(60),
                    Error::Encode,
                )
                .await?;
                let name = "photo-000.jpg";
                let output_path = directory.join(name);
                let out_bytes = fs::metadata(&output_path).await?.len();
                if out_bytes == 0 {
                    return Err(Error::InvalidOutput);
                }
                if out_bytes > self.photo_cap {
                    fs::remove_file(&output_path).await?;
                    continue;
                }
                let (width, height, pix_fmt) = media::probe_jpeg(&self.tools, directory, name)
                    .await?
                    .validate_photo(max_edge, input.hdr_transfer.is_some())
                    .map_err(|_| Error::InvalidOutput)?;
                media::decode_photo(&self.tools, directory, name, true).await?;
                let thumbnail = media::photo_thumbnail(&self.tools, directory, name).await?;
                fs::remove_file(directory.join("source")).await?;
                record.photos.push(Photo {
                    name: name.into(),
                    bytes: out_bytes,
                    width,
                    height,
                    pix_fmt,
                    quality,
                    downscaled: width.max(height) < src_w.max(src_h)
                        || width.min(height) < src_w.min(src_h),
                });
                return Ok(thumbnail);
            }
            max_edge = (max_edge * 85) / 100;
        }
        Err(Error::ClipSize)
    }

    async fn inspect_clips(
        &self,
        directory: &Path,
        input: &VideoInput,
        expected_duration: f64,
    ) -> Result<Vec<Clip>, Error> {
        let mut clips = Vec::new();
        let mut total = 0.0;
        for (index, path) in clip_paths(directory).await?.into_iter().enumerate() {
            let name = format!("clip-{index:03}.mp4");
            if path.file_name().and_then(|value| value.to_str()) != Some(&name) {
                return Err(Error::InvalidOutput);
            }
            let bytes = fs::metadata(&path).await?.len();
            if bytes > self.clip_cap {
                return Err(Error::ClipSize);
            }
            if bytes == 0 {
                return Err(Error::InvalidOutput);
            }
            let (duration, video_duration, video) = media::probe(&self.tools, directory, &name)
                .await?
                .validate_clip(
                    self.video_profile.canvas(),
                    input.audio_codec.is_some(),
                    input.hdr_transfer.is_some(),
                )
                .map_err(|_| Error::InvalidOutput)?;
            media::decode(&self.tools, directory, &name, duration, video.frames).await?;
            total += video_duration;
            clips.push(Clip {
                name,
                bytes,
                duration,
                video_duration,
                video,
                audio: input.audio_codec.is_some(),
            });
        }
        if (total - expected_duration).abs() > 0.5 {
            return Err(Error::InvalidOutput);
        }
        Ok(clips)
    }
}

async fn clip_paths(directory: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut paths = Vec::new();
    let mut entries = fs::read_dir(directory).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_name() == "source" {
            continue;
        }
        if paths.len() == MAX_CLIPS || !entry.file_type().await?.is_file() {
            return Err(Error::InvalidOutput);
        }
        paths.push(entry.path());
    }
    if paths.is_empty() {
        return Err(Error::InvalidOutput);
    }
    paths.sort_unstable();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(directory: &Path, output: &str, options: &[&str]) -> PathBuf {
        let file = directory.join(output);
        let result = std::process::Command::new(Tools::from_path().unwrap().ffmpeg)
            .current_dir(directory)
            .args(["-hide_banner", "-nostdin", "-v", "error"])
            .args(options)
            .arg(&file)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        file
    }

    async fn from_file(processor: &mut Processor, file: &Path) -> Result<Job, Box<Record>> {
        let input = File::open(file).await.unwrap();
        processor
            .process(input, Some(file.metadata().unwrap().len()))
            .await
    }

    fn assert_empty(root: &Path) {
        assert!(root.read_dir().unwrap().next().is_none());
    }

    #[tokio::test]
    async fn bounds_declared_and_received_input() {
        for (declared, input, expected) in [
            (Some(5), &b"a"[..], Error::InputSize),
            (None, &b"12345"[..], Error::InputSize),
            (Some(2), &b"a"[..], Error::SizeMismatch),
            (None, &b""[..], Error::Empty),
        ] {
            let work = tempfile::tempdir().unwrap();
            assert_eq!(
                receive(input, declared, work.path(), 4)
                    .await
                    .unwrap_err()
                    .to_string(),
                expected.to_string(),
            );
        }
    }

    #[tokio::test]
    async fn converts_generated_sdr_matrix_and_cleans_work() {
        let fixtures = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(Tools::from_path().unwrap(), work.path()).unwrap();
        let root = fixtures.path();
        let golden = fixture(
            root,
            "video ; $(touch injected).mp4",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=1080x1920:r=30:d=0.8",
                "-f",
                "lavfi",
                "-i",
                "sine=duration=0.8",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-threads:v",
                "2",
                "-c:a",
                "aac",
                "-ac",
                "2",
                "-colorspace",
                "bt709",
                "-color_trc",
                "bt709",
                "-color_primaries",
                "bt709",
            ],
        );
        let landscape = fixture(
            root,
            "landscape.mp4",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=s=3840x2160:r=60:d=0.25",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-threads:v",
                "2",
            ],
        );
        let rotated = fixture(
            root,
            "rotated.mp4",
            &[
                "-display_rotation",
                "90",
                "-i",
                "landscape.mp4",
                "-c",
                "copy",
            ],
        );
        let variable = fixture(
            root,
            "variable.mp4",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=90x160:r=24:d=2",
                "-vf",
                "select='if(lt(t,1),1,not(mod(n,3)))'",
                "-fps_mode",
                "vfr",
                "-c:v",
                "libx264",
                "-threads:v",
                "2",
            ],
        );
        let long = fixture(
            root,
            "long.mp4",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=s=90x160:r=5:d=29.6",
                "-f",
                "lavfi",
                "-i",
                "sine=duration=29.6",
                "-c:v",
                "libx264",
                "-threads:v",
                "2",
                "-c:a",
                "aac",
            ],
        );
        for (file, count, audio) in [
            (golden, 1, true),
            (landscape, 1, false),
            (rotated, 1, false),
            (variable, 1, false),
            (long, 2, true),
        ] {
            let job = from_file(&mut processor, &file)
                .await
                .unwrap_or_else(|record| {
                    panic!("{}: {:?}", file.display(), record);
                });
            let record = serde_json::to_value(job.record()).unwrap();
            assert_eq!(
                job.work.path().metadata().unwrap().permissions().mode() & 0o777,
                0o700
            );
            let clips = &job.record().clips;
            assert_eq!(clips.len(), count);
            for (index, clip) in clips.iter().enumerate() {
                assert_eq!(clip.name, format!("clip-{index:03}.mp4"));
                assert_eq!(clip.audio, audio);
                assert!(clip.duration <= 29.0 && clip.bytes <= CLIP_CAP);
                assert_eq!(record["clips"][index]["video"]["rotation"], 0);
            }
            match file.file_name().unwrap().to_str().unwrap() {
                "rotated.mp4" => assert_eq!(
                    record["input"]["video"]["rotation"].as_i64().unwrap().abs(),
                    90
                ),
                "variable.mp4" => assert_eq!(record["input"]["video"]["vfr_indicated"], true),
                "long.mp4" => {
                    assert_eq!(record["input"]["enlarged"], true);
                    assert_eq!(record["clips"][0]["video"]["fps"], 5.0);
                }
                _ => {}
            }
            let saved = job.export(output.path()).await.unwrap();
            assert_eq!(saved.read_dir().unwrap().count(), count);
            assert_eq!(
                saved.metadata().unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_empty(work.path());
            assert!(
                !record
                    .to_string()
                    .contains(file.file_name().unwrap().to_str().unwrap())
            );
        }
        assert!(!root.join("injected").exists());
    }

    #[tokio::test]
    async fn converts_status720_clip_and_cleans_work() {
        let fixtures = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let source = fixture(
            fixtures.path(),
            "portrait.mp4",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=1080x1920:r=30:d=0.8",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-threads:v",
                "2",
            ],
        );
        let mut processor = Processor::new(Tools::from_path().unwrap(), work.path())
            .unwrap()
            .with_video_profile(VideoProfile::Status720);
        let job = from_file(&mut processor, &source).await.unwrap();
        assert_eq!(job.record().profile, Some("status720"));
        assert_eq!(job.record().clips.len(), 1);
        assert!(job.record().clips[0].duration <= 29.0);

        let saved = job.export(output.path()).await.unwrap();
        assert_empty(work.path());
        let clip = saved.join("clip-000.mp4");
        assert!((1..=15_800_000).contains(&clip.metadata().unwrap().len()));
        let probe = std::process::Command::new(&processor.tools.ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=codec_name,profile,level,pix_fmt,width,height",
                "-of",
                "json",
            ])
            .arg(clip)
            .output()
            .unwrap();
        assert!(probe.status.success());
        let metadata: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert_eq!(
            metadata["streams"][0],
            serde_json::json!({
                "codec_name": "h264", "profile": "High", "level": 40,
                "pix_fmt": "yuv420p", "width": 720, "height": 1280
            })
        );
    }

    #[tokio::test]
    async fn converts_pq_and_hlg_to_sdr_and_segments_long_hdr_video() {
        let fixtures = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(Tools::from_path().unwrap(), work.path()).unwrap();
        for (transfer, duration, expected_parts) in
            [("smpte2084", "1", 1), ("arib-std-b67", "30", 2)]
        {
            let tags = format!(
                "format=yuv420p10le,setparams=range=limited:color_primaries=bt2020:color_trc={transfer}:colorspace=bt2020nc"
            );
            let source = fixture(
                fixtures.path(),
                &format!("{transfer}.mp4"),
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=s=96x64:r=5",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:sample_rate=44100",
                    "-t",
                    duration,
                    "-c:v",
                    "libx264",
                    "-vf",
                    &tags,
                    "-c:a",
                    "aac",
                ],
            );
            let job = from_file(&mut processor, &source)
                .await
                .unwrap_or_else(|record| panic!("{transfer}: {record:#?}"));
            assert!(matches!(
                job.record().input.as_ref().unwrap(),
                Input::Video(video) if video.hdr_transfer == Some(transfer)
            ));
            assert_eq!(job.record().clips.len(), expected_parts);
            assert!(
                job.record()
                    .clips
                    .iter()
                    .all(|clip| clip.duration <= 29.0 && clip.audio)
            );
            drop(job);
            assert_empty(work.path());
        }
    }

    #[tokio::test]
    async fn ignores_only_supported_phone_camera_metadata_tracks() {
        let fixtures = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(Tools::from_path().unwrap(), work.path()).unwrap();
        let root = fixtures.path();
        std::fs::write(root.join("data"), b"fixture").unwrap();
        let generic = fixture(
            root,
            "tagged.mov",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=32x32:r=25:d=1",
                "-f",
                "data",
                "-i",
                "data",
                "-map",
                "0:v",
                "-map",
                "1:0",
                "-c:v",
                "libx264",
                "-c:d",
                "copy",
                "-tag:d",
                "gpmd",
            ],
        );
        let failure = from_file(&mut processor, &generic).await.unwrap_err();
        assert_eq!(failure.error, Some(Error::InvalidMedia));

        // FFmpeg can mux gpmd data, so relabel its track to model camera metadata.
        let mut bytes = std::fs::read(generic).unwrap();
        for start in 0..bytes.len() - 3 {
            if &bytes[start..start + 4] == b"gpmd" {
                bytes[start..start + 4].copy_from_slice(b"mebx");
            }
        }
        let camera = root.join("camera.mov");
        std::fs::write(&camera, bytes).unwrap();
        let job = from_file(&mut processor, &camera).await.unwrap();
        assert_eq!(job.record().clips.len(), 1);
        drop(job);
        assert_empty(work.path());
    }

    #[tokio::test]
    async fn rejects_invalid_sources_before_encode_and_cleans_work() {
        let fixtures = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(Tools::from_path().unwrap(), work.path()).unwrap();
        let root = fixtures.path();
        let healthy = fixture(
            root,
            "base.mp4",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=90x160:r=24:d=1",
                "-c:v",
                "libx264",
                "-movflags",
                "+faststart",
            ],
        );
        let mut contents = healthy.metadata().unwrap().len();
        contents /= 2;
        let truncated = root.join("truncated.mp4");
        std::fs::copy(healthy, &truncated).unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&truncated)
            .unwrap()
            .set_len(contents)
            .unwrap();
        let corrupt = root.join("corrupt.mp4");
        std::fs::write(&corrupt, b"no video here").unwrap();
        let extra = fixture(
            root,
            "extra.mp4",
            &["-i", "base.mp4", "-map", "0:v", "-map", "0:v", "-c", "copy"],
        );
        std::fs::write(
            root.join("extra.srt"),
            "1\n00:00:00,000 --> 00:00:00,500\nfixture\n",
        )
        .unwrap();
        let subtitle = fixture(
            root,
            "subtitle.mp4",
            &[
                "-i",
                "base.mp4",
                "-i",
                "extra.srt",
                "-map",
                "0:v",
                "-map",
                "1:s",
                "-c:v",
                "copy",
                "-c:s",
                "mov_text",
            ],
        );
        let overlong = fixture(
            root,
            "too-long.mp4",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=s=16x16:r=1:d=601",
                "-c:v",
                "libx264",
            ],
        );
        let hdr = fixture(
            root,
            "hdr.mp4",
            &[
                "-i",
                "base.mp4",
                "-c:v",
                "copy",
                "-bsf:v",
                "h264_metadata=colour_primaries=1:transfer_characteristics=18:matrix_coefficients=1",
            ],
        );
        for (file, errors) in [
            (truncated, &[Error::Probe, Error::Decode][..]),
            (corrupt, &[Error::Probe]),
            (extra, &[Error::InvalidMedia]),
            (subtitle, &[Error::InvalidMedia]),
            (overlong, &[Error::Duration]),
            (hdr, &[Error::UnsupportedHdr]),
        ] {
            let failure = from_file(&mut processor, &file)
                .await
                .expect_err(file.file_name().unwrap().to_str().unwrap());
            assert!(
                errors.contains(&failure.error.unwrap()),
                "{file:?}: {failure:?}"
            );
            assert!(failure.encode_attempts.is_empty());
            assert_empty(work.path());
        }
    }

    #[tokio::test]
    async fn cleans_up_when_probe_fails_or_returns_malformed_metadata() {
        let work = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(Tools::from_path().unwrap(), work.path()).unwrap();
        for executable in ["/usr/bin/true", "/usr/bin/false"] {
            processor.tools.ffprobe = executable.into();
            let result = processor.process(&b"fixture"[..], None).await.unwrap_err();
            assert_eq!(result.error, Some(Error::Probe));
            assert!(result.encode_attempts.is_empty());
            assert_empty(work.path());
        }
    }

    #[tokio::test]
    async fn retries_oversized_output_once_then_cleans_work() {
        let fixtures = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(Tools::from_path().unwrap(), work.path()).unwrap();
        processor.clip_cap = 1;
        let file = fixture(
            fixtures.path(),
            "silent.mp4",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=s=90x160:r=10:d=0.3",
                "-c:v",
                "libx264",
            ],
        );
        let result = from_file(&mut processor, &file).await.unwrap_err();
        assert_eq!(result.error, Some(Error::ClipSize));
        assert_eq!(result.encode_attempts.len(), 2);
        assert!(
            result.encode_attempts[1]
                .windows(2)
                .any(|pair| pair == ["-crf", "27"])
        );
        assert_empty(work.path());
    }

    #[tokio::test]
    async fn cleans_work_when_receiving_is_cancelled() {
        let work = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(Tools::from_path().unwrap(), work.path()).unwrap();
        let (reader, _writer) = tokio::io::duplex(16);
        assert!(
            timeout(Duration::from_millis(80), processor.process(reader, None))
                .await
                .is_err()
        );
        assert_empty(work.path());
    }

    fn inject_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II");
        tiff.extend_from_slice(&42u16.to_le_bytes());
        tiff.extend_from_slice(&8u32.to_le_bytes());
        tiff.extend_from_slice(&1u16.to_le_bytes());
        tiff.extend_from_slice(&0x0112u16.to_le_bytes());
        tiff.extend_from_slice(&3u16.to_le_bytes());
        tiff.extend_from_slice(&1u32.to_le_bytes());
        tiff.extend_from_slice(&u32::from(orientation).to_le_bytes());
        tiff.extend_from_slice(&0u32.to_le_bytes());
        let mut app1 = Vec::from(b"Exif\0\0".as_slice());
        app1.extend_from_slice(&tiff);
        let len = u16::try_from(app1.len() + 2).unwrap();
        let mut out = Vec::with_capacity(jpeg.len() + app1.len() + 4);
        out.extend_from_slice(&jpeg[..2]);
        out.extend_from_slice(&[0xff, 0xe1]);
        out.extend_from_slice(&len.to_be_bytes());
        out.extend_from_slice(&app1);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    fn lossless_webp_4x4() -> Vec<u8> {
        vec![
            0x52, 0x49, 0x46, 0x46, 0x1c, 0x00, 0x00, 0x00, 0x57, 0x45, 0x42, 0x50, 0x56, 0x50,
            0x38, 0x4c, 0x0f, 0x00, 0x00, 0x00, 0x2f, 0x03, 0xc0, 0x00, 0x00, 0x07, 0x10, 0xfd,
            0x8f, 0xfe, 0x07, 0x22, 0xa2, 0xff, 0x01, 0x00,
        ]
    }

    #[tokio::test]
    async fn converts_photo_matrix_and_cleans_work() {
        let fixtures = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(Tools::from_path().unwrap(), work.path()).unwrap();
        let root = fixtures.path();

        let golden = fixture(
            root,
            "photo ; $(touch injected_photo).jpg",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=1080x1920:d=1",
                "-frames:v",
                "1",
                "-c:v",
                "mjpeg",
            ],
        );
        let oversized = fixture(
            root,
            "oversized.png",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=4200x2800:d=1",
                "-frames:v",
                "1",
                "-c:v",
                "png",
            ],
        );
        let alpha = fixture(
            root,
            "alpha.png",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=c=red@0.5:s=300x200:d=1",
                "-frames:v",
                "1",
                "-pix_fmt",
                "rgba",
                "-c:v",
                "png",
            ],
        );
        let base_rot = fixture(
            root,
            "base_rot.jpg",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=200x100:d=1",
                "-frames:v",
                "1",
                "-c:v",
                "mjpeg",
            ],
        );
        let exif_rot = root.join("exif_rot.jpg");
        std::fs::write(
            &exif_rot,
            inject_exif_orientation(&std::fs::read(base_rot).unwrap(), 6),
        )
        .unwrap();
        let webp = root.join("still.webp");
        std::fs::write(&webp, lossless_webp_4x4()).unwrap();

        for (file, expected_dims, expected_downscaled) in [
            (golden, (1080, 1920), false),
            (oversized, (3840, 2560), true),
            (alpha, (300, 200), false),
            (exif_rot, (100, 200), false),
            (webp, (4, 4), false),
        ] {
            let job = from_file(&mut processor, &file)
                .await
                .unwrap_or_else(|record| panic!("{}: {record:#?}", file.display()));
            assert_eq!(job.kind(), MediaKind::Photo);
            assert!(
                job.photo_thumbnail()
                    .is_some_and(|thumb| !thumb.is_empty() && thumb.starts_with("/9j/"))
            );
            let record = serde_json::to_value(job.record()).unwrap();
            assert_eq!(record["kind"], "photo");
            assert_eq!(record["profile"], "photo_hd");
            assert!(record.get("clips").is_none());
            assert!(!record.to_string().contains(job.photo_thumbnail().unwrap()));
            assert_eq!(job.record().photos.len(), 1);
            let photo = &job.record().photos[0];
            assert_eq!(photo.name, "photo-000.jpg");
            assert!(photo.bytes > 0 && photo.bytes <= PHOTO_CAP);
            assert_eq!((photo.width, photo.height), expected_dims);
            assert_eq!(photo.downscaled, expected_downscaled);
            assert_eq!(photo.pix_fmt, "yuvj420p");

            let saved = job.export(output.path()).await.unwrap();
            assert_eq!(saved.read_dir().unwrap().count(), 1);
            assert!(saved.join("photo-000.jpg").is_file());
            assert_empty(work.path());
            assert!(
                !record
                    .to_string()
                    .contains(file.file_name().unwrap().to_str().unwrap())
            );
        }
        assert!(!root.join("injected_photo").exists());
    }

    #[tokio::test]
    async fn retries_photo_quality_and_scale_on_size_cap_and_cleans_work() {
        let fixtures = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(Tools::from_path().unwrap(), work.path()).unwrap();
        let detailed = fixture(
            fixtures.path(),
            "detailed.png",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=1600x1200:d=1,noise=alls=20:allf=t+u",
                "-frames:v",
                "1",
                "-c:v",
                "png",
            ],
        );

        let full = from_file(&mut processor, &detailed).await.unwrap();
        let q2_bytes = full.record().photos[0].bytes;
        assert_eq!(full.record().encode_attempts.len(), 1);
        drop(full);

        processor.photo_cap = q2_bytes / 2;
        let stepped = from_file(&mut processor, &detailed).await.unwrap();
        assert!(stepped.record().encode_attempts.len() > 1);
        assert!(stepped.record().photos[0].bytes <= processor.photo_cap);
        drop(stepped);
        assert_empty(work.path());

        processor.photo_cap = 1;
        let small = fixture(
            fixtures.path(),
            "small.png",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=800x600:d=1",
                "-frames:v",
                "1",
                "-c:v",
                "png",
            ],
        );
        let failed = from_file(&mut processor, &small).await.unwrap_err();
        assert_eq!(failed.error, Some(Error::ClipSize));
        assert!(failed.encode_attempts.len() > PHOTO_QUALITIES.len());
        assert_empty(work.path());
    }

    #[tokio::test]
    async fn rejects_invalid_photos_before_encode_and_cleans_work() {
        let fixtures = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut processor = Processor::new(Tools::from_path().unwrap(), work.path()).unwrap();
        let root = fixtures.path();

        let corrupt = root.join("corrupt.jpg");
        std::fs::write(&corrupt, b"\xff\xd8not-a-valid-jpeg").unwrap();

        let tiny = fixture(
            root,
            "tiny.png",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=s=1x1:d=1",
                "-frames:v",
                "1",
                "-c:v",
                "png",
            ],
        );
        let too_wide = fixture(
            root,
            "too_wide.png",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=s=12300x16:d=1",
                "-frames:v",
                "1",
                "-c:v",
                "png",
            ],
        );
        let too_many_pixels = fixture(
            root,
            "too_many_pixels.png",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=s=7300x7300:d=1",
                "-frames:v",
                "1",
                "-c:v",
                "png",
            ],
        );
        let multi_frame = fixture(
            root,
            "multi.tiff",
            &[
                "-f",
                "lavfi",
                "-i",
                "testsrc2=s=32x32:r=2:d=1",
                "-frames:v",
                "2",
                "-f",
                "image2pipe",
                "-c:v",
                "tiff",
            ],
        );
        let over_50mb = fixture(
            root,
            "over50mb.bmp",
            &[
                "-f",
                "lavfi",
                "-i",
                "color=s=4500x4000:d=1",
                "-frames:v",
                "1",
                "-c:v",
                "bmp",
            ],
        );
        assert!(over_50mb.metadata().unwrap().len() > MAX_PHOTO_INPUT);

        for (file, errors) in [
            (
                corrupt,
                &[Error::Probe, Error::InvalidMedia, Error::Decode][..],
            ),
            (tiny, &[Error::InvalidMedia]),
            (too_wide, &[Error::InvalidMedia]),
            (too_many_pixels, &[Error::InvalidMedia]),
            (multi_frame, &[Error::InvalidMedia, Error::Decode]),
            (over_50mb, &[Error::InputSize]),
        ] {
            let failure = from_file(&mut processor, &file)
                .await
                .expect_err(file.file_name().unwrap().to_str().unwrap());
            assert!(
                errors.contains(&failure.error.unwrap()),
                "{file:?}: {failure:?}"
            );
            assert!(failure.encode_attempts.is_empty());
            assert_empty(work.path());
        }
    }
}
