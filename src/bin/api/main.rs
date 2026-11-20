mod store;
#[cfg(test)]
mod tests;

use std::{
    env, fs, io,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Path as RoutePath, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::{StreamExt, stream};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use store::{ArtifactMeta, DeliveryAction, Job, Reservation, Store, remove_media};
use subtle::ConstantTimeEq;
use tokio::io::AsyncWriteExt;
use tokio_util::{io::ReaderStream, sync::CancellationToken, task::TaskTracker};
use uuid::Uuid;
use vidora::{MAX_INPUT, MediaKind, Processor, Tools};

const UPLOAD_TIMEOUT: Duration = Duration::from_secs(90);
const ARTIFACT_TIMEOUT: Duration = Duration::from_secs(45);
const PORT: u16 = 8080;

struct Api {
    store: Mutex<Store>,
    work: PathBuf,
    media: PathBuf,
    token: sha2::digest::Output<Sha256>,
    stop: CancellationToken,
    tasks: TaskTracker,
    _lock: fs::File,
}

type Shared = Arc<Api>;
type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug)]
struct ApiError(StatusCode, &'static str);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<rusqlite::Error> for ApiError {
    fn from(_: rusqlite::Error) -> Self {
        eprintln!("database_operation_failed");
        Self(StatusCode::INTERNAL_SERVER_ERROR, "database_error")
    }
}

fn invalid() -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, "invalid_request")
}

fn missing() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "not_found")
}

impl Api {
    fn open(root: &Path, token: &str) -> Result<Shared, Box<dyn std::error::Error>> {
        if token.len() < 32 || !token.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err("invalid token".into());
        }
        private_dir(root)?;
        let lock = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(root.join("instance.lock"))?;
        lock.try_lock()?;
        let work = root.join("work");
        let media = root.join("media");
        remove_media(&work)?;
        private_dir(&work)?;
        private_dir(&media)?;
        let store = Store::open(&root.join("jobs.db"))?;
        store.recover()?;
        if let Some(job) = store.active()? {
            store.expire(&job.id)?;
            if !job.intact(&media) {
                store.fail(&job.id, "artifact_missing")?;
            }
        }
        let api = Arc::new(Self {
            store: Mutex::new(store),
            work,
            media,
            token: Sha256::digest(token),
            stop: CancellationToken::new(),
            tasks: TaskTracker::new(),
            _lock: lock,
        });
        api.sweep()?;
        Ok(api)
    }

    fn cleanup(&self, id: &str) {
        if remove_media(&self.media.join(id)).is_err() {
            eprintln!("media_cleanup_failed");
        }
    }

    fn sweep(&self) -> Result<(), Box<dyn std::error::Error>> {
        let store = self.store.lock().unwrap();
        if let Some(job) = store.active()? {
            store.expire(&job.id)?;
        }
        let active = store.active()?;
        for entry in fs::read_dir(&self.media)? {
            let entry = entry?;
            if active
                .as_ref()
                .is_some_and(|job| entry.file_name() == job.id.as_str())
            {
                continue;
            }
            if entry.file_type()?.is_dir() {
                remove_media(&entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    fn fail(&self, id: &str, error: &str) -> ApiResult<()> {
        self.store.lock().unwrap().fail(id, error)?;
        self.cleanup(id);
        Ok(())
    }

    fn get(&self, id: &str) -> ApiResult<Job> {
        let parsed = Uuid::parse_str(id).map_err(|_| missing())?;
        if parsed.to_string() != id {
            return Err(missing());
        }
        let store = self.store.lock().unwrap();
        if store.expire(id)? {
            self.cleanup(id);
        }
        store.get(id)?.ok_or_else(missing)
    }
}

fn private_dir(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    if !fs::symlink_metadata(path)?.is_dir() {
        return Err(io::Error::other("untrusted data directory"));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn router(api: Shared) -> Router {
    let internal = Router::new()
        .route("/jobs", post(submit).get(status))
        .route("/jobs/{id}", get(get_job))
        .route("/jobs/{id}/artifacts/{index}", get(artifact))
        .route("/jobs/{id}/delivery", post(delivery))
        .route("/jobs/{id}/terminal", post(terminal))
        .layer(DefaultBodyLimit::max(1024))
        .route_layer(middleware::from_fn_with_state(api.clone(), authorize));
    Router::new()
        .route("/health", get(|| async { StatusCode::NO_CONTENT }))
        .nest("/internal/v1", internal)
        .with_state(api)
}

async fn authorize(State(api): State<Shared>, request: Request, next: Next) -> Response {
    let mut values = request.headers().get_all(header::AUTHORIZATION).iter();
    let allowed = values
        .next()
        .and_then(|value| value.as_bytes().strip_prefix(b"Bearer "))
        .is_some_and(|value| bool::from(Sha256::digest(value).ct_eq(&api.token)));
    if !allowed || values.next().is_some() {
        return ApiError(StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }
    if api.stop.is_cancelled() {
        return ApiError(StatusCode::SERVICE_UNAVAILABLE, "shutting_down").into_response();
    }
    tokio::select! {
        _ = api.stop.cancelled() => ApiError(StatusCode::SERVICE_UNAVAILABLE, "shutting_down").into_response(),
        response = tokio::time::timeout(UPLOAD_TIMEOUT + Duration::from_secs(5), next.run(request)) => {
            response.unwrap_or_else(|_| ApiError(StatusCode::REQUEST_TIMEOUT, "request_timeout").into_response())
        }
    }
}

fn single<'a>(headers: &'a HeaderMap, name: &str) -> ApiResult<Option<&'a str>> {
    let mut values = headers.get_all(name).iter();
    let value = values.next();
    if values.next().is_some() {
        return Err(invalid());
    }
    value
        .map(|value| value.to_str().map_err(|_| invalid()))
        .transpose()
}

fn message_hash(id: &str) -> ApiResult<String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(invalid());
    }
    Ok(hex_digest(&Sha256::digest(id)))
}

fn hex_digest(digest: &sha2::digest::Output<Sha256>) -> String {
    use std::fmt::Write;
    let mut hex = String::with_capacity(64);
    for byte in digest {
        write!(hex, "{byte:02x}").unwrap();
    }
    hex
}

async fn status(State(api): State<Shared>, headers: HeaderMap) -> ApiResult<Json<Option<Job>>> {
    api.sweep()
        .map_err(|_| ApiError(StatusCode::INTERNAL_SERVER_ERROR, "cleanup_error"))?;
    let store = api.store.lock().unwrap();
    Ok(Json(match single(&headers, "x-message-id")? {
        Some(id) => store.by_message(&message_hash(id)?)?,
        None => store.active()?.or(store.latest()?),
    }))
}

async fn get_job(
    State(api): State<Shared>,
    RoutePath(id): RoutePath<String>,
) -> ApiResult<Json<Job>> {
    api.get(&id).map(Json)
}

struct Upload {
    bytes: u64,
    digest: String,
}

impl Upload {
    fn read_headers(headers: &HeaderMap) -> ApiResult<Self> {
        let bytes = single(headers, "content-length")?
            .ok_or(ApiError(StatusCode::LENGTH_REQUIRED, "length_required"))?
            .parse::<u64>()
            .map_err(|_| invalid())?;
        if bytes > MAX_INPUT {
            return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "input_size"));
        }
        let digest = single(headers, "x-content-sha256")?.ok_or_else(invalid)?;
        if bytes == 0
            || digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(invalid());
        }
        if let Some(name) = single(headers, "x-display-name")?
            && (name.is_empty()
                || name.len() > 200
                || name
                    .bytes()
                    .any(|byte| matches!(byte, b'/' | b'\\') || byte.is_ascii_control()))
        {
            return Err(invalid());
        }
        Ok(Self {
            bytes,
            digest: digest.to_owned(),
        })
    }

    async fn write(self, mut body: Body, file: &mut tokio::fs::File) -> ApiResult<u64> {
        use axum::body::HttpBody;
        let mut received = 0;
        let mut hash = Sha256::new();
        while let Some(frame) =
            std::future::poll_fn(|cx| std::pin::Pin::new(&mut body).poll_frame(cx)).await
        {
            let frame =
                frame.map_err(|_| ApiError(StatusCode::BAD_REQUEST, "upload_interrupted"))?;
            let Ok(bytes) = frame.into_data() else {
                continue;
            };
            received += bytes.len() as u64;
            if received > self.bytes {
                return Err(ApiError(StatusCode::PAYLOAD_TOO_LARGE, "input_size"));
            }
            hash.update(&bytes);
            file.write_all(&bytes).await.map_err(|_| storage_error())?;
        }
        if received != self.bytes {
            return Err(ApiError(StatusCode::BAD_REQUEST, "upload_interrupted"));
        }
        if hex_digest(&hash.finalize()) != self.digest {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "digest_mismatch",
            ));
        }
        file.flush().await.map_err(|_| storage_error())?;
        Ok(received)
    }
}

fn storage_error() -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, "storage_error")
}

async fn submit(State(api): State<Shared>, headers: HeaderMap, body: Body) -> ApiResult<Response> {
    let hash = message_hash(single(&headers, "x-message-id")?.ok_or_else(invalid)?)?;
    let upload = Upload::read_headers(&headers)?;
    let reservation = api.store.lock().unwrap().reserve(&hash)?;
    let id = match reservation {
        Reservation::Duplicate(job) => return Ok(Json(job).into_response()),
        Reservation::Busy => return Err(ApiError(StatusCode::CONFLICT, "busy")),
        Reservation::New(id) => id,
    };
    let task_api = api.clone();
    api.tasks
        .spawn(async move {
            match receive(&task_api, &id, upload, body).await {
                Ok(()) => Ok((StatusCode::ACCEPTED, Json(task_api.get(&id)?)).into_response()),
                Err(error) => {
                    if matches!(
                        error.1,
                        "upload_interrupted" | "upload_timeout" | "shutting_down"
                    ) {
                        task_api.store.lock().unwrap().abandon_upload(&id)?;
                    } else {
                        task_api.fail(&id, error.1)?;
                    }
                    Err(error)
                }
            }
        })
        .await
        .map_err(|_| storage_error())?
}

async fn receive(api: &Shared, id: &str, upload: Upload, body: Body) -> ApiResult<()> {
    let dir = tempfile::tempdir_in(&api.work).map_err(|_| storage_error())?;
    let path = dir.path().join("pending");
    let mut file = tokio::fs::File::create_new(&path)
        .await
        .map_err(|_| storage_error())?;
    let bytes = tokio::select! {
        _ = api.stop.cancelled() => return Err(ApiError(StatusCode::SERVICE_UNAVAILABLE, "shutting_down")),
        result = tokio::time::timeout(UPLOAD_TIMEOUT, upload.write(body, &mut file)) => {
            result.map_err(|_| ApiError(StatusCode::REQUEST_TIMEOUT, "upload_timeout"))??
        }
    };
    drop(file);
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| storage_error())?;
    if !api.store.lock().unwrap().processing(id)? {
        return Err(storage_error());
    }
    let task_api = api.clone();
    let id = id.to_owned();
    api.tasks.spawn(async move {
        let result = tokio::select! {
            _ = task_api.stop.cancelled() => Err("processing_interrupted"),
            result = process(&task_api, &id, dir, file, bytes) => result,
        };
        if let Err(error) = result {
            let _ = task_api.fail(&id, error);
        }
    });
    Ok(())
}

async fn process(
    api: &Shared,
    id: &str,
    source: tempfile::TempDir,
    file: tokio::fs::File,
    bytes: u64,
) -> Result<(), &'static str> {
    let tools = Tools::from_path().map_err(|_| "media_tools_unavailable")?;
    let mut processor = Processor::new(tools, &api.work).map_err(|_| "storage_error")?;
    let job = processor.process(file, Some(bytes)).await;
    source.close().map_err(|_| "storage_error")?;
    let job = job.map_err(|record| {
        eprintln!("processing_failed error={:?}", record.error);
        match record.error {
            Some(vidora::Error::UnsupportedHdr) => "unsupported_hdr",
            Some(vidora::Error::InputSize) => "input_size",
            _ => "processing_failed",
        }
    })?;
    let record = serde_json::to_string(job.record()).map_err(|_| "storage_error")?;
    let (kind, sizes, artifacts, names): (&str, Vec<u64>, Vec<ArtifactMeta>, Vec<String>) =
        match job.kind() {
            MediaKind::Video => (
                "video",
                job.record().clips.iter().map(|clip| clip.bytes).collect(),
                job.record()
                    .clips
                    .iter()
                    .map(|_| ArtifactMeta {
                        mime: "video/mp4".into(),
                        width: None,
                        height: None,
                        thumbnail: None,
                    })
                    .collect(),
                job.record()
                    .clips
                    .iter()
                    .map(|clip| clip.name.clone())
                    .collect(),
            ),
            MediaKind::Photo => {
                let thumbnail = job.photo_thumbnail().map(str::to_owned);
                (
                    "photo",
                    job.record()
                        .photos
                        .iter()
                        .map(|photo| photo.bytes)
                        .collect(),
                    job.record()
                        .photos
                        .iter()
                        .map(|photo| ArtifactMeta {
                            mime: "image/jpeg".into(),
                            width: Some(photo.width),
                            height: Some(photo.height),
                            thumbnail: thumbnail.clone(),
                        })
                        .collect(),
                    job.record()
                        .photos
                        .iter()
                        .map(|photo| photo.name.clone())
                        .collect(),
                )
            }
        };
    let directory = api.media.join(id);
    private_dir(&directory).map_err(|_| "storage_error")?;
    let path = job.export(&directory).await.map_err(|_| "export_failed")?;
    // Commit durable artifacts before publishing their SQLite record.
    for name in &names {
        sync_path(&path.join(name)).await?;
    }
    for directory in [&path, &directory, &api.media] {
        sync_path(directory).await?;
    }
    let sizes = serde_json::to_string(&sizes).map_err(|_| "storage_error")?;
    let artifact_meta = serde_json::to_string(&artifacts).map_err(|_| "storage_error")?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("export_failed")?;
    if api
        .store
        .lock()
        .unwrap()
        .ready(id, kind, name, &sizes, &artifact_meta, &record)
        .map_err(|_| "database_error")?
    {
        Ok(())
    } else {
        Err("processing_interrupted")
    }
}

async fn sync_path(path: &Path) -> Result<(), &'static str> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| "storage_error")?;
    file.sync_all().await.map_err(|_| "storage_error")
}

async fn artifact(
    State(api): State<Shared>,
    RoutePath((id, index)): RoutePath<(String, usize)>,
) -> ApiResult<Response> {
    let job = api.get(&id)?;
    if index != job.next_index || job.pending {
        return Err(missing());
    }
    let content_type = match job.kind.as_str() {
        "video" => "video/mp4",
        "photo" => "image/jpeg",
        _ => return Err(missing()),
    };
    let path = job.artifact(&api.media, index).ok_or_else(missing)?;
    let size = job.sizes[index];
    let file = match tokio::fs::File::open(path).await {
        Ok(file)
            if file
                .metadata()
                .await
                .is_ok_and(|m| m.is_file() && m.len() == size) =>
        {
            file
        }
        _ => {
            api.fail(&id, "artifact_missing")?;
            return Err(missing());
        }
    };
    use tokio::io::AsyncReadExt;
    let stop = api.stop.clone();
    let deadline = tokio::time::Instant::now() + ARTIFACT_TIMEOUT;
    let chunks = stream::try_unfold(ReaderStream::new(file.take(size)), move |mut chunks| {
        let stop = stop.clone();
        async move {
            tokio::select! {
                biased;
                _ = stop.cancelled() => Err(io::Error::from(io::ErrorKind::Interrupted)),
                result = tokio::time::timeout_at(deadline, chunks.next()) => {
                    Ok(result.map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?.transpose()?.map(|bytes| (bytes, chunks)))
                }
            }
        }
    });
    Ok((
        [
            (header::CONTENT_TYPE, content_type.to_owned()),
            (header::CONTENT_LENGTH, size.to_string()),
            (header::CACHE_CONTROL, "no-store".to_owned()),
        ],
        Body::from_stream(chunks),
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delivery {
    action: DeliveryAction,
    index: u8,
}

async fn delivery(
    State(api): State<Shared>,
    RoutePath(id): RoutePath<String>,
    Json(update): Json<Delivery>,
) -> ApiResult<Json<Job>> {
    let job = api.get(&id)?;
    if update.index >= 32
        || !api
            .store
            .lock()
            .unwrap()
            .delivery(&id, update.index, update.action)?
    {
        return Err(ApiError(StatusCode::CONFLICT, "delivery_state"));
    }
    let job = if job.state == "delivered" {
        job
    } else {
        api.get(&id)?
    };
    if job.state == "delivered" {
        api.cleanup(&id);
    }
    Ok(Json(job))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Terminal {
    DeliveryFailed,
    DeliveryUncertain,
}

async fn terminal(
    State(api): State<Shared>,
    RoutePath(id): RoutePath<String>,
    Json(reason): Json<Terminal>,
) -> ApiResult<StatusCode> {
    let job = api.get(&id)?;
    if matches!(job.state.as_str(), "ready" | "delivering") {
        api.fail(
            &id,
            match reason {
                Terminal::DeliveryFailed if !job.pending => "delivery_failed",
                _ => "delivery_uncertain",
            },
        )?;
    } else if job.state != "failed" {
        return Err(ApiError(StatusCode::CONFLICT, "delivery_state"));
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn stop_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate()).expect("install termination handler");
    tokio::select! {
        _ = terminate.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = env::var_os("DATA_DIR")
        .map(PathBuf::from)
        .ok_or("missing data directory")?;
    let token = env::var("INTERNAL_API_TOKEN")?;
    if !root.is_absolute() {
        return Err("data directory must be absolute".into());
    }
    let ip =
        env::var("VIDORA_BIND_IP").map_or(Ok(IpAddr::V4(Ipv4Addr::LOCALHOST)), |ip| ip.parse())?;
    let api = Api::open(&root, &token)?;
    Tools::from_path()?.check(&api.work).await?;
    let listener = tokio::net::TcpListener::bind(SocketAddr::new(ip, PORT)).await?;
    let scheduler = api.clone();
    api.tasks.spawn(async move {
        let mut sweep = tokio::time::interval(Duration::from_secs(30));
        loop {
            tokio::select! {
                _ = scheduler.stop.cancelled() => break,
                _ = sweep.tick() => {
                    if scheduler.sweep().is_err() {
                        eprintln!("retention_sweep_failed");
                    }
                }
            }
        }
    });
    eprintln!("api_started");
    let server = axum::serve(listener, router(api.clone()))
        .with_graceful_shutdown(api.stop.clone().cancelled_owned());
    let mut server = std::pin::pin!(std::future::IntoFuture::into_future(server));
    let result = tokio::select! {
        result = &mut server => result,
        _ = stop_signal() => {
            api.stop.cancel();
            tokio::time::timeout(Duration::from_secs(8), &mut server).await.unwrap_or(Ok(()))
        }
    };
    api.stop.cancel();
    api.tasks.close();
    api.tasks.wait().await;
    let active = api.store.lock().unwrap().active()?;
    if let Some(job) = active {
        api.fail(
            &job.id,
            if job.pending {
                "delivery_uncertain"
            } else {
                "shutdown"
            },
        )
        .map_err(|_| "failed to clean active job")?;
    }
    api.sweep()?;
    result?;
    Ok(())
}

fn healthcheck() -> io::Result<()> {
    let timeout = Duration::from_secs(1);
    let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), PORT);
    let mut connection = TcpStream::connect_timeout(&addr, timeout)?;
    connection.set_read_timeout(Some(timeout))?;
    connection.set_write_timeout(Some(timeout))?;
    connection
        .write_all(b"GET /health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut status = [0; 12];
    connection.read_exact(&mut status)?;
    if &status != b"HTTP/1.1 204" {
        return Err(io::Error::other("API unhealthy"));
    }
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let mut args = env::args_os().skip(1);
    if let Some(command) = args.next() {
        return if command == "--healthcheck" && args.next().is_none() && healthcheck().is_ok() {
            std::process::ExitCode::SUCCESS
        } else {
            std::process::ExitCode::FAILURE
        };
    }
    if run().await.is_err() {
        eprintln!("api_startup_or_shutdown_failed");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
