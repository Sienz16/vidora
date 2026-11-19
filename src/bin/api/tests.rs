use std::process::Command;

use axum::{body::to_bytes, http::Request};
use serde_json::Value;
use tower::ServiceExt;

use super::*;

const TOKEN: &str = "test-token-used-only-by-the-api-tests";
const MESSAGE: &str = "synthetic-message-1";

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    auth: bool,
    headers: &[(&str, &str)],
    body: Body,
) -> Response {
    let mut request = Request::builder().method(method).uri(path);
    if auth {
        request = request.header("authorization", format!("Bearer {TOKEN}"));
    }
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    app.clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
}

async fn read_json(response: Response) -> Value {
    let bytes = to_bytes(response.into_body(), 16_384).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn upload(app: &Router, id: &str, length: u64, digest: &str, body: Body) -> Response {
    request(
        app,
        "POST",
        "/internal/v1/jobs",
        true,
        &[
            ("content-length", &length.to_string()),
            ("x-message-id", id),
            ("x-content-sha256", digest),
            ("x-display-name", "sample ; dollar $.mp4"),
        ],
        body,
    )
    .await
}

async fn delivery_action(app: &Router, job: &str, index: usize, action: &str) -> Response {
    request(
        app,
        "POST",
        &format!("/internal/v1/jobs/{job}/delivery"),
        true,
        &[("content-type", "application/json")],
        Body::from(json!({ "index": index, "action": action }).to_string()),
    )
    .await
}

fn seed_ready(api: &Shared, id: &str) -> String {
    let store = api.store.lock().unwrap();
    let Reservation::New(job) = store.reserve(&message_hash(id).unwrap()).unwrap() else {
        panic!("should reserve new job");
    };
    assert!(store.processing(&job).unwrap());
    let directory = api.media.join(&job).join("vidora-generated");
    private_dir(&directory).unwrap();
    fs::write(directory.join("clip-000.mp4"), b"first").unwrap();
    fs::write(directory.join("clip-001.mp4"), b"second").unwrap();
    assert!(
        store
            .ready(&job, "vidora-generated", "[5,6]", "{}")
            .unwrap()
    );
    job
}

#[tokio::test]
async fn all_internal_routes_require_bearer_and_validate_artifact_paths() {
    let root = tempfile::tempdir().unwrap();
    let app = router(Api::open(root.path(), TOKEN).unwrap());
    let prefix = "/internal/v1/jobs";
    for (method, path) in [
        ("POST", prefix.to_owned()),
        ("GET", prefix.to_owned()),
        ("GET", format!("{prefix}/id")),
        ("GET", format!("{prefix}/id/artifacts/0")),
        ("POST", format!("{prefix}/id/delivery")),
        ("POST", format!("{prefix}/id/terminal")),
    ] {
        let response = request(&app, method, &path, false, &[], Body::empty()).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    for path in [
        format!("{prefix}/%2e%2e/artifacts/0"),
        format!("{prefix}/id/artifacts/%2f"),
    ] {
        assert!(
            request(&app, "GET", &path, true, &[], Body::empty())
                .await
                .status()
                .is_client_error()
        );
    }
    assert_eq!(
        request(&app, "GET", "/health", false, &[], Body::empty())
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn upload_checks_length_digest_name_and_recovers_disconnected_body() {
    let root = tempfile::tempdir().unwrap();
    let api = Api::open(root.path(), TOKEN).unwrap();
    let app = router(api.clone());
    let digest = hex_digest(&Sha256::digest(b"abc"));
    assert_eq!(
        upload(&app, MESSAGE, MAX_INPUT + 1, &digest, Body::empty())
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    let missing_length = request(
        &app,
        "POST",
        "/internal/v1/jobs",
        true,
        &[("x-message-id", MESSAGE)],
        Body::empty(),
    )
    .await;
    assert_eq!(missing_length.status(), StatusCode::LENGTH_REQUIRED);
    let invalid_name = request(
        &app,
        "POST",
        "/internal/v1/jobs",
        true,
        &[
            ("content-length", "3"),
            ("x-message-id", MESSAGE),
            ("x-content-sha256", &digest),
            ("x-display-name", "../sample.mp4"),
        ],
        Body::from("abc"),
    )
    .await;
    assert_eq!(invalid_name.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        upload(&app, MESSAGE, 3, &digest, Body::from("a"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert!(api.store.lock().unwrap().latest().unwrap().is_none());
    assert_eq!(
        upload(&app, MESSAGE, 3, &digest, Body::from("abcd"))
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        upload(&app, "wrong-hash", 3, &digest, Body::from("xyz"))
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(fs::read_dir(&api.work).unwrap().count(), 0);
}

#[tokio::test]
async fn ordered_delivery_is_idempotent_across_restart() {
    let root = tempfile::tempdir().unwrap();
    let api = Api::open(root.path(), TOKEN).unwrap();
    assert!(
        Api::open(root.path(), TOKEN).is_err(),
        "instance lock should reject two workers"
    );
    let id = seed_ready(&api, MESSAGE);
    let app = router(api.clone());
    let prefix = format!("/internal/v1/jobs/{id}");
    let digest = hex_digest(&Sha256::digest(b"a"));
    let duplicate = upload(&app, MESSAGE, 1, &digest, Body::from("a")).await;
    assert_eq!(duplicate.status(), StatusCode::OK);
    assert_eq!(read_json(duplicate).await["id"], id);
    assert_eq!(
        upload(&app, "other", 1, &digest, Body::from("a"))
            .await
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        delivery_action(&app, &id, 1, "begin").await.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        delivery_action(&app, &id, 0, "ack").await.status(),
        StatusCode::CONFLICT
    );
    let first = request(
        &app,
        "GET",
        &format!("{prefix}/artifacts/0"),
        true,
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(to_bytes(first.into_body(), 16).await.unwrap(), "first");
    assert_eq!(
        delivery_action(&app, &id, 0, "begin").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        delivery_action(&app, &id, 0, "begin").await.status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        read_json(delivery_action(&app, &id, 0, "ack").await).await["next_index"],
        1
    );
    drop(app);
    drop(api);

    let api = Api::open(root.path(), TOKEN).unwrap();
    let app = router(api.clone());
    assert_eq!(
        read_json(delivery_action(&app, &id, 0, "ack").await).await["next_index"],
        1
    );
    let old = request(
        &app,
        "GET",
        &format!("{prefix}/artifacts/0"),
        true,
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(old.status(), StatusCode::NOT_FOUND);
    let second = request(
        &app,
        "GET",
        &format!("{prefix}/artifacts/1"),
        true,
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(to_bytes(second.into_body(), 16).await.unwrap(), "second");
    assert_eq!(
        delivery_action(&app, &id, 1, "begin").await.status(),
        StatusCode::OK
    );
    assert_eq!(
        read_json(delivery_action(&app, &id, 1, "ack").await).await["state"],
        "delivered"
    );
    assert_eq!(
        read_json(delivery_action(&app, &id, 1, "ack").await).await["state"],
        "delivered"
    );
    assert_eq!(fs::read_dir(&api.media).unwrap().count(), 0);
}

#[tokio::test]
async fn reconciliation_and_retention_remove_abandoned_media() {
    let root = tempfile::tempdir().unwrap();
    let api = Api::open(root.path(), TOKEN).unwrap();
    let id = seed_ready(&api, MESSAGE);
    let app = router(api.clone());
    assert_eq!(
        delivery_action(&app, &id, 0, "begin").await.status(),
        StatusCode::OK
    );
    fs::write(api.work.join("leftover"), b"media").unwrap();
    drop(app);
    drop(api);
    let api = Api::open(root.path(), TOKEN).unwrap();
    assert_eq!(
        api.get(&id).unwrap().error.as_deref(),
        Some("delivery_uncertain")
    );
    assert_eq!(fs::read_dir(&api.work).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&api.media).unwrap().count(), 0);

    let id = seed_ready(&api, "expired");
    rusqlite::Connection::open(root.path().join("jobs.db"))
        .unwrap()
        .execute(
            "UPDATE jobs SET expires = unixepoch() - 1 WHERE id = ?1",
            [&id],
        )
        .unwrap();
    api.sweep().unwrap();
    assert_eq!(api.get(&id).unwrap().error.as_deref(), Some("expired"));
    assert_eq!(fs::read_dir(&api.media).unwrap().count(), 0);

    let id = seed_ready(&api, "missing");
    let path = api.get(&id).unwrap().artifact(&api.media, 0).unwrap();
    fs::remove_file(path).unwrap();
    let app = router(api.clone());
    let response = request(
        &app,
        "GET",
        &format!("/internal/v1/jobs/{id}/artifacts/0"),
        true,
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        api.get(&id).unwrap().error.as_deref(),
        Some("artifact_missing")
    );
    assert_eq!(fs::read_dir(&api.media).unwrap().count(), 0);
}

#[tokio::test]
async fn streams_a_generated_clip_through_the_real_worker() {
    let fixture = tempfile::tempdir().unwrap();
    let source = fixture.path().join("sample.mp4");
    assert!(
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=320x240:rate=24",
                "-t",
                "1",
                "-c:v",
                "libx264",
            ])
            .arg(&source)
            .status()
            .unwrap()
            .success()
    );
    let root = tempfile::tempdir().unwrap();
    let api = Api::open(root.path(), TOKEN).unwrap();
    let app = router(api.clone());
    let data = fs::read(&source).unwrap();
    let digest = hex_digest(&Sha256::digest(&data));
    let file = tokio::fs::File::open(&source).await.unwrap();
    let response = upload(
        &app,
        MESSAGE,
        data.len() as u64,
        &digest,
        Body::from_stream(ReaderStream::new(file)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let response = read_json(response).await;
    let id = response["id"].as_str().unwrap();
    api.tasks.close();
    tokio::time::timeout(Duration::from_secs(30), api.tasks.wait())
        .await
        .unwrap();
    let job = api.get(id).unwrap();
    assert_eq!(job.state, "ready", "{job:?}");
    let response = request(
        &app,
        "GET",
        &format!("/internal/v1/jobs/{id}/artifacts/0"),
        true,
        &[],
        Body::empty(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 2_000_000)
            .await
            .unwrap()
            .len() as u64,
        job.sizes[0]
    );
    assert_eq!(fs::read_dir(&api.work).unwrap().count(), 0);
    let terminal = request(
        &app,
        "POST",
        &format!("/internal/v1/jobs/{id}/terminal"),
        true,
        &[("content-type", "application/json")],
        Body::from("\"delivery_failed\""),
    )
    .await;
    assert_eq!(terminal.status(), StatusCode::NO_CONTENT);
    assert_eq!(fs::read_dir(&api.media).unwrap().count(), 0);
}
