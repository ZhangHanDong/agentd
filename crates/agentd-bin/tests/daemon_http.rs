//! P0.9 9b: the daemon assembles the production host behind the HTTP/SSE router,
//! driven by `tower::oneshot`. Names match `specs/e2e/p96-daemon-assembly.spec.md`.

use std::path::PathBuf;
use std::sync::Arc;

use agentd_bin::{ProductionRunHost, SystemClock, daemon};
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::RunId;
use agentd_store::{SqliteStore, run_repo};
use agentd_surface::mcp_server::dispatch;
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

/// Like [`router_with_started_run`] but drives the run to `done` (emitting the
/// `run_finished` terminal) so a oneshot `GET /events` on the now-LIVE SSE tail
/// (P1) collects a finite body instead of hanging.
async fn router_with_finished_run() -> (Router, tempfile::TempDir) {
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
    host.start_run(&run).await.expect("start run"); // parks at propose_spec
    dispatch(
        &host,
        "submit_outcome",
        serde_json::json!({
            "run_id": "r1", "node_id": "propose_spec", "attempt": 1, "status": "success",
            "context_updates": {}, "suggested_next": []
        }),
    )
    .await
    .expect("submit_outcome drives the run to done");
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

    // The run was created + started: its snapshot reflects the park. (We avoid
    // GET /events here — the live tail would not close on a still-parked run.)
    let (snap_status, snap) = get(app, "/runs/r1").await;
    assert_eq!(snap_status, StatusCode::OK);
    let sv: serde_json::Value = serde_json::from_str(&snap).expect("json");
    assert_eq!(
        sv["current_node"], "propose_spec",
        "run created + started: {snap}"
    );
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
async fn daemon_router_serves_dashboard_shell() {
    let (app, _dir) = empty_router().await;
    let (status, body) = get(app, "/dashboard").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains(r#"id="agentd-dashboard""#), "{body}");
    assert!(body.contains(r#"fetch("/runs")"#), "{body}");
    assert!(
        body.contains(r#"addEventListener("run_parked""#),
        "dashboard must listen to production run_parked events: {body}"
    );
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
    let (app, _dir) = router_with_finished_run().await;
    let (status, body) = get(app, "/runs/r1/events").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("run_parked"), "events body: {body:?}");
    assert!(
        body.contains("run_finished"),
        "the terminal closes the live stream: {body:?}"
    );
}

// --- P1 #4: startup guard — `bind_listener` clear already-running detection ---

#[tokio::test]
async fn bind_listener_succeeds_on_free_port() {
    // Port 0 → the OS assigns a free port; AddrInUse cannot occur.
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = daemon::bind_listener(addr).await.expect("free port binds");
    assert!(
        listener.local_addr().expect("local_addr").port() != 0,
        "the OS assigned a concrete port"
    );
}

#[tokio::test]
async fn bind_listener_reports_already_running() {
    // A live listener already owns the port: a second bind must report the
    // friendly already-running message (race-free via AddrInUse), not a raw OS
    // error — and name the address so the operator knows which port.
    let held = tokio::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("hold a port");
    let addr = held.local_addr().expect("addr");
    let err = daemon::bind_listener(addr)
        .await
        .expect_err("the port is taken");
    assert!(
        err.contains("already running"),
        "friendly already-running message, not a raw OS error: {err}"
    );
    assert!(err.contains(&addr.to_string()), "names the address: {err}");
}
