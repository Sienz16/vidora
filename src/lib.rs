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

use media::{CLIP_CAP, MAX_CLIPS, Video};
use serde::Serialize;
use tempfile::TempDir;
use tokio::{
    fs::{self, File},
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    time::timeout,
};

const MAX_INPUT: u64 = 300 * 1024 * 1024;

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
}

#[derive(Debug, Serialize)]
struct Input {
    bytes: u64,
    duration: f64,
    video_duration: Option<f64>,
    container: String,
    video_codec: String,
    audio_codec: Option<String>,
    enlarged: bool,
    video: Video,
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
pub struct Record {
    pub job_id: Option<String>,
    pub profile: &'static str,
    pub ffmpeg_version: Option<String>,
    input: Option<Input>,
    pub encode_attempts: Vec<Vec<String>>,
    pub clips: Vec<Clip>,
    pub elapsed_ms: u128,
    pub error: Option<Error>,
}

#[derive(Debug)]
pub struct Job {
    work: TempDir,
    record: Record,
}

impl Job {
    pub fn record(&self) -> &Record {
        &self.record
    }

    /// Copy validated clips into a new private directory below a trusted destination.
    /// Working media is removed before the export is published.
    pub async fn export(mut self, destination: &Path) -> Result<PathBuf, Box<Record>> {
        let result = async {
            let export = tempfile::Builder::new()
                .prefix("vidora-")
                .permissions(Permissions::from_mode(0o700))
                .tempdir_in(destination)?;
            for clip in &self.record.clips {
                fs::copy(
                    self.work.path().join(&clip.name),
                    export.path().join(&clip.name),
                )
                .await?;
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
    clip_cap: u64,
}

impl Processor {
    /// The work root is a trusted directory. Share one processor per worker.
    pub fn new(tools: Tools, root: &Path) -> Result<Self, Error> {
        Ok(Self {
            tools,
            root: root.canonicalize()?,
            clip_cap: CLIP_CAP,
        })
    }

    pub async fn process(
        &mut self,
        reader: impl AsyncRead + Unpin,
        declared_size: Option<u64>,
    ) -> Result<Job, Box<Record>> {
        let started = Instant::now();
        let mut record = Record {
            job_id: None,
            profile: "hd29",
            ffmpeg_version: None,
            input: None,
            encode_attempts: Vec::new(),
            clips: Vec::new(),
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
            if let Err(error) = result {
                work.close()?;
                return Err(error);
            }
            Ok(work)
        }
        .await;
        record.elapsed_ms = started.elapsed().as_millis();
        match result {
            Ok(work) => Ok(Job { work, record }),
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
    ) -> Result<(), Error> {
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
        let input = media::probe(&self.tools, directory, "source")
            .await?
            .input(bytes)?;
        let input = record.input.insert(input);
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
            let args = media::encode_args(input, retry);
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
                .inspect_clips(directory, input.audio_codec.is_some(), expected_duration)
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

    async fn inspect_clips(
        &self,
        directory: &Path,
        expected_audio: bool,
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
                .validate_clip(expected_audio)
                .map_err(|_| Error::InvalidOutput)?;
            media::decode(&self.tools, directory, &name, duration, video.frames).await?;
            total += video_duration;
            clips.push(Clip {
                name,
                bytes,
                duration,
                video_duration,
                video,
                audio: expected_audio,
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
                "h264_metadata=colour_primaries=9:transfer_characteristics=18:matrix_coefficients=9",
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
}
