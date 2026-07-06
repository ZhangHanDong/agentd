//! P0.7 7b Task 5: the axum HTTP+SSE surface, driven in-process by
//! `tower::oneshot` against a `FakeRunHost` — no socket, no real engine.
//! Test names match `specs/surface/p73-http-routes.spec.md`.

use std::sync::Arc;

use agentd_surface::host::{EventRecord, RunSnapshot};
use agentd_surface::http::{AppState, router};
use agentd_surface::test_support::FakeRunHost;
use axum::Router;
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
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

async fn post(app: Router, uri: &str, body: &str) -> axum::http::Response<Body> {
    app.oneshot(
        Request::post(uri)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .expect("request"),
    )
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

#[tokio::test]
async fn dashboard_routes_serve_html_shell() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .expect("content-type");
    assert!(
        content_type.starts_with("text/html"),
        "content type: {content_type}"
    );
    let body = body_string(resp).await;
    assert!(body.contains(r#"id="agentd-dashboard""#), "{body}");
    assert!(body.contains(r#"data-region="runs-list""#), "{body}");
    assert!(body.contains(r#"data-region="run-detail""#), "{body}");
    assert!(body.contains(r#"data-region="event-log""#), "{body}");

    let slash_resp = get(app(FakeRunHost::new()), "/dashboard/").await;
    assert_eq!(slash_resp.status(), StatusCode::OK);
    let slash_body = body_string(slash_resp).await;
    assert_eq!(slash_body, body);
}

#[tokio::test]
async fn dashboard_shell_serves_without_host_reads() {
    let host = FakeRunHost::new();
    host.fail_list_runs();
    let resp = get(app(host), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("agentd-dashboard"), "{body}");
}

#[tokio::test]
async fn dashboard_shell_uses_existing_read_only_endpoints() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains(r#"fetch("/runs")"#), "{body}");
    assert!(body.contains(r#"fetch(`/runs/${"#), "{body}");
    assert!(body.contains(r#"new EventSource(`/runs/${"#), "{body}");
    assert!(body.contains(r#"/events"#), "{body}");
    assert!(!body.contains("POST /runs"), "{body}");
    assert!(!body.contains(r#"method: "POST""#), "{body}");
    assert!(!body.contains("tools/call"), "{body}");
    assert!(!body.to_lowercase().contains("specify"), "{body}");
}

#[tokio::test]
async fn dashboard_shell_listens_to_production_event_kinds() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    for kind in ["run_parked", "run_finished", "run_failed", "state_resync"] {
        assert!(
            body.contains(&format!(r#"addEventListener("{kind}""#)),
            "missing EventSource listener for {kind}: {body}"
        );
    }
    assert!(
        body.contains(r#"addEventListener("node.parked""#),
        "legacy replay listener should remain available: {body}"
    );
}

#[tokio::test]
async fn dashboard_shell_remains_read_only_after_production_event_alignment() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(!body.contains("POST /runs"), "{body}");
    assert!(!body.contains(r#"method: "POST""#), "{body}");
    assert!(!body.contains("tools/call"), "{body}");
    assert!(!body.contains("Specify"), "{body}");
}

#[tokio::test]
async fn dashboard_shell_refreshes_state_after_live_events() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("async function refreshSelectedRunState()"),
        "missing selected-run refresh helper: {body}"
    );
    assert!(
        body.contains("loadRunDetail(selectedRunId)") && body.contains("loadRuns()"),
        "refresh helper must use existing read-only endpoints: {body}"
    );
    for kind in ["run_parked", "run_finished", "run_failed", "state_resync"] {
        assert!(
            body.contains(&format!(
                r#"addEventListener("{kind}", (event) => appendRunEvent("{kind}""#
            )),
            "production event {kind} must append and refresh selected run state: {body}"
        );
    }
}

#[tokio::test]
async fn dashboard_shell_keeps_generic_and_compat_events_log_only() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains(
            r#"eventSource.onmessage = (event) => appendEvent("message", event.lastEventId, event.data);"#
        ),
        "generic messages stay log-only: {body}"
    );
    assert!(
        body.contains(
            r#"eventSource.addEventListener("node.parked", (event) => appendEvent("node.parked", event.lastEventId, event.data));"#
        ),
        "compat node.parked listener stays log-only: {body}"
    );
}

#[tokio::test]
async fn dashboard_shell_live_state_refresh_remains_read_only() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains(r#"fetch("/runs")"#), "{body}");
    assert!(body.contains(r#"fetch(`/runs/${"#), "{body}");
    assert!(body.contains(r#"new EventSource(`/runs/${"#), "{body}");
    assert!(body.contains(r#"/events"#), "{body}");
    assert!(!body.contains("POST /runs"), "{body}");
    assert!(!body.contains(r#"method: "POST""#), "{body}");
    assert!(!body.contains("tools/call"), "{body}");
    assert!(!body.contains("Specify"), "{body}");
}

#[tokio::test]
async fn dashboard_shell_pretty_prints_json_event_payloads() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("function formatEventData(data)"),
        "missing event payload formatter: {body}"
    );
    assert!(
        body.contains("JSON.parse(data)") && body.contains("JSON.stringify(parsed, null, 2)"),
        "formatter must parse and pretty-print JSON payloads: {body}"
    );
    assert!(
        body.contains(r#"<span class="event-data">${formatEventData(data)}</span>"#),
        "appendEvent must render formatted event data: {body}"
    );
}

#[tokio::test]
async fn dashboard_shell_keeps_raw_fallback_for_non_json_event_payloads() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("catch"),
        "formatter needs parse fallback: {body}"
    );
    assert!(
        body.contains("return escapeText(data);"),
        "malformed/non-JSON payloads must keep escaped raw rendering: {body}"
    );
}

#[tokio::test]
async fn dashboard_shell_event_payload_formatting_remains_read_only() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("white-space: pre-wrap;"),
        "pretty-printed event data must preserve whitespace: {body}"
    );
    assert!(!body.contains("POST /runs"), "{body}");
    assert!(!body.contains(r#"method: "POST""#), "{body}");
    assert!(!body.contains("tools/call"), "{body}");
    assert!(!body.contains("Specify"), "{body}");
}

#[tokio::test]
async fn dashboard_shell_refresh_button_updates_selected_run_state() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains(r#"refreshButton.addEventListener("click", () => refreshDashboard());"#),
        "refresh button must call refreshDashboard: {body}"
    );
    assert!(
        body.contains("async function refreshDashboard()"),
        "missing refreshDashboard helper: {body}"
    );
    assert!(
        body.contains("if (selectedRunId)") && body.contains("await refreshSelectedRunState();"),
        "selected run refresh path must reuse refreshSelectedRunState: {body}"
    );
}

#[tokio::test]
async fn dashboard_shell_refresh_button_falls_back_to_runs_without_selection() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(
        body.contains("async function refreshDashboard()"),
        "missing refreshDashboard helper: {body}"
    );
    assert!(
        body.contains("await loadRuns();"),
        "refreshDashboard must fall back to loading the overview: {body}"
    );
}

#[tokio::test]
async fn dashboard_shell_refresh_button_remains_read_only_and_keeps_event_tail() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains(r#"fetch("/runs")"#), "{body}");
    assert!(body.contains(r#"fetch(`/runs/${"#), "{body}");
    assert!(body.contains(r#"new EventSource(`/runs/${"#), "{body}");
    assert!(body.contains(r#"/events"#), "{body}");
    assert!(
        !body.contains(r#"refreshButton.addEventListener("click", () => tailEvents"#),
        "refresh button must not recreate the event tail: {body}"
    );
    assert!(!body.contains("POST /runs"), "{body}");
    assert!(!body.contains(r#"method: "POST""#), "{body}");
    assert!(!body.contains("tools/call"), "{body}");
    assert!(!body.contains("Specify"), "{body}");
}

#[tokio::test]
async fn dashboard_route_rejects_post() {
    let resp = post(app(FakeRunHost::new()), "/dashboard", r#"{"flow":"draft"}"#).await;
    assert_ne!(resp.status(), StatusCode::OK);
    assert_ne!(resp.status(), StatusCode::CREATED);
}
