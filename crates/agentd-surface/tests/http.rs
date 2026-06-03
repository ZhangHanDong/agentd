//! P0.7 7b Task 5: the axum HTTP+SSE surface, driven in-process by
//! `tower::oneshot` against a `FakeRunHost` — no socket, no real engine.
//! Test names match `specs/surface/p73-http-routes.spec.md`.

use std::sync::Arc;

use agentd_surface::host::{EventRecord, RunSnapshot};
use agentd_surface::http::{AppState, router};
use agentd_surface::test_support::FakeRunHost;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt; // oneshot

fn app(host: FakeRunHost) -> Router {
    router(AppState {
        host: Arc::new(host),
    })
}

async fn get(app: Router, uri: &str) -> axum::http::Response<Body> {
    app.oneshot(Request::get(uri).body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

async fn body_string(resp: axum::http::Response<Body>) -> String {
    let bytes = resp
        .into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes();
    String::from_utf8(bytes.to_vec()).expect("utf8")
}

#[tokio::test]
async fn http_healthz_ok() {
    let resp = get(app(FakeRunHost::new()), "/healthz").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_string(resp).await, "ok");
}

#[tokio::test]
async fn http_get_run_returns_snapshot() {
    let host = FakeRunHost::new();
    host.set_snapshot(
        "r1",
        RunSnapshot {
            status: "parked".into(),
            current_node: Some("review".into()),
            completed_nodes: vec!["impl".into()],
            context: json!({"k": "v"}),
        },
    );
    let resp = get(app(host), "/runs/r1").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v: Value = serde_json::from_str(&body_string(resp).await).expect("json");
    assert_eq!(v["status"], "parked");
    assert_eq!(v["current_node"], "review");
}

#[tokio::test]
async fn http_get_run_unknown_is_404() {
    let resp = get(app(FakeRunHost::new()), "/runs/ghost").await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn http_sse_replays_from_cursor() {
    // The SSE tail is now LIVE (P1); a terminal event in the replay closes the
    // stream so this oneshot collects a finite body deterministically.
    let host = FakeRunHost::new();
    host.set_events(
        "r1",
        vec![
            EventRecord {
                seq: 1,
                kind: "run.started".into(),
                payload: "{}".into(),
            },
            EventRecord {
                seq: 2,
                kind: "node.parked".into(),
                payload: r#"{"node":"review"}"#.into(),
            },
            EventRecord {
                seq: 3,
                kind: "run_finished".into(),
                payload: "{}".into(),
            },
        ],
    );
    let resp = get(app(host), "/runs/r1/events?from_seq=1").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("node.parked"), "replays seq > 1: {body}");
    assert!(!body.contains("run.started"), "skips seq <= cursor: {body}");
    assert!(
        body.contains("run_finished"),
        "closes on the terminal event: {body}"
    );
}

#[tokio::test]
async fn http_sse_invalid_from_seq_is_400() {
    let resp = get(
        app(FakeRunHost::new()),
        "/runs/r1/events?from_seq=notanumber",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
