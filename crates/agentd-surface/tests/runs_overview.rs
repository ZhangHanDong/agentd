//! P1: `GET /runs` — the at-a-glance overview (every run's current status), via
//! `tower::oneshot` against a `FakeRunHost`. Names match
//! `specs/surface/p76-runs-overview.spec.md`.

use std::sync::Arc;

use agentd_surface::host::RunSummary;
use agentd_surface::http::{AppState, AuthConfig, MediaConfig, SchedulerConfig, router};
use agentd_surface::test_support::FakeRunHost;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

fn app(host: FakeRunHost) -> Router {
    router(AppState {
        host: Arc::new(host),
        auth: AuthConfig::open(),
        media: MediaConfig::default_for_tests(),
        scheduler: SchedulerConfig::default(),
    })
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

fn summary(run_id: &str, status: &str, node: Option<&str>, started_at: i64) -> RunSummary {
    RunSummary {
        run_id: run_id.to_string(),
        status: status.to_string(),
        current_node: node.map(str::to_string),
        started_at,
    }
}

#[tokio::test]
async fn get_runs_lists_all_runs() {
    let host = FakeRunHost::new();
    host.set_runs(vec![
        summary("r1", "running", Some("implement"), 200),
        summary("r2", "finished", None, 100),
    ]);
    let (status, body) = get(app(host), "/runs").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).expect("json");
    let arr = v.as_array().expect("array");
    assert_eq!(arr.len(), 2, "both runs: {body}");
    assert!(body.contains("r1") && body.contains("running"), "{body}");
    assert!(body.contains("r2") && body.contains("finished"), "{body}");
}

#[tokio::test]
async fn get_runs_empty_is_empty_array() {
    let (status, body) = get(app(FakeRunHost::new()), "/runs").await;
    assert_eq!(status, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).expect("json");
    assert!(
        v.as_array().expect("array").is_empty(),
        "empty array: {body}"
    );
}

#[tokio::test]
async fn get_runs_store_error_is_500() {
    let host = FakeRunHost::new();
    host.fail_list_runs();
    let (status, _body) = get(app(host), "/runs").await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}
