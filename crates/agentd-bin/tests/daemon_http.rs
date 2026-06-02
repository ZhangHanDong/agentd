//! P0.9 9b: the daemon assembles the production host behind the HTTP/SSE router,
//! driven by `tower::oneshot`. Names match `specs/e2e/p96-daemon-assembly.spec.md`.

use std::path::PathBuf;
use std::sync::Arc;

use agentd_bin::{ProductionRunHost, SystemClock, daemon};
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::RunId;
use agentd_store::{SqliteStore, run_repo};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
}

/// Build the daemon router over a production host (real store + fakes) with a
/// `draft.dot` run already started to its `propose_spec` park.
async fn router_with_started_run() -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let host = ProductionRunHost::new(
        store,
        Box::new(FakeBackend::new()),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    let run = RunId::from_string("r1");
    run_repo::record_run(host.store().pool(), &run, "draft.dot", "sha")
        .await
        .expect("record run");
    host.start_run(&run).await.expect("start run");
    (daemon::build_router(Arc::new(host)), dir)
}

async fn get(app: Router, uri: &str) -> (StatusCode, String) {
    let resp = app
        .oneshot(Request::get(uri).body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
}

async fn post(app: Router, uri: &str, body: serde_json::Value) -> (StatusCode, String) {
    let resp = app
        .oneshot(
            Request::post(uri)
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
}

/// Build the daemon router over a production host with an empty store (no run yet).
async fn empty_router() -> (Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let host = ProductionRunHost::new(
        store,
        Box::new(FakeBackend::new()),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    (daemon::build_router(Arc::new(host)), dir)
}

#[tokio::test]
async fn post_runs_creates_and_starts_a_draft_run() {
    let (app, _dir) = empty_router().await;
    let (status, body) = post(
        app.clone(),
        "/runs",
        serde_json::json!({ "flow": "draft", "run_id": "r1" }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["status"], "parked", "parked at propose_spec: {body}");

    let (es, events) = get(app, "/runs/r1/events").await;
    assert_eq!(es, StatusCode::OK);
    assert!(events.contains("run_parked"), "events: {events:?}");
}

#[tokio::test]
async fn post_runs_unknown_flow_is_error() {
    let (app, _dir) = empty_router().await;
    let (status, _body) = post(
        app,
        "/runs",
        serde_json::json!({ "flow": "bogus", "run_id": "r1" }),
    )
    .await;
    assert!(
        !status.is_success(),
        "an unknown flow is an error, got {status}"
    );
}

#[tokio::test]
async fn daemon_router_healthz_ok() {
    let (app, _dir) = router_with_started_run().await;
    let (status, body) = get(app, "/healthz").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn daemon_router_serves_run_snapshot() {
    let (app, _dir) = router_with_started_run().await;
    let (status, body) = get(app, "/runs/r1").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["current_node"], "propose_spec", "snapshot: {body}");
}

#[tokio::test]
async fn daemon_router_streams_run_events() {
    let (app, _dir) = router_with_started_run().await;
    let (status, body) = get(app, "/runs/r1/events").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("run_parked"), "events body: {body:?}");
}
