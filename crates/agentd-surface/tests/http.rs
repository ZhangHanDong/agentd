//! P0.7 7b Task 5: the axum HTTP+SSE surface, driven in-process by
//! `tower::oneshot` against a `FakeRunHost` — no socket, no real engine.
//! Test names match `specs/surface/p73-http-routes.spec.md`.

use std::fmt::Write as _;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agentd_surface::host::{EventRecord, RunSnapshot};
use agentd_surface::http::{
    AgentTokenMode, AppState, AuthConfig, MediaConfig, SchedulerConfig, router,
};
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
        auth: AuthConfig::open(),
        media: MediaConfig::default_for_tests(),
        scheduler: SchedulerConfig::default(),
    })
}

fn app_with_media(host: FakeRunHost, media_dir: PathBuf) -> Router {
    router(AppState {
        host: Arc::new(host),
        auth: AuthConfig::open(),
        media: MediaConfig { media_dir },
        scheduler: SchedulerConfig::default(),
    })
}

fn app_with_auth(host: FakeRunHost, auth: AuthConfig) -> Router {
    router(AppState {
        host: Arc::new(host),
        auth,
        media: MediaConfig::default_for_tests(),
        scheduler: SchedulerConfig::default(),
    })
}

fn app_with_scheduler_auth(
    host: FakeRunHost,
    auth: AuthConfig,
    scheduler: SchedulerConfig,
) -> Router {
    router(AppState {
        host: Arc::new(host),
        auth,
        media: MediaConfig::default_for_tests(),
        scheduler,
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

async fn patch(app: Router, uri: &str, body: &str) -> axum::http::Response<Body> {
    app.oneshot(
        Request::patch(uri)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .expect("request"),
    )
    .await
    .expect("response")
}

async fn delete(app: Router, uri: &str) -> axum::http::Response<Body> {
    app.oneshot(Request::delete(uri).body(Body::empty()).expect("request"))
        .await
        .expect("response")
}

async fn post_with_agent_token(
    app: Router,
    uri: &str,
    body: &str,
    token: Option<&str>,
) -> axum::http::Response<Body> {
    let mut req = Request::post(uri).header(CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        req = req.header("x-agent-token", token);
    }
    app.oneshot(req.body(Body::from(body.to_owned())).expect("request"))
        .await
        .expect("response")
}

async fn post_with_bearer(
    app: Router,
    uri: &str,
    body: &str,
    token: Option<&str>,
) -> axum::http::Response<Body> {
    let mut req = Request::post(uri).header(CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        req = req.header("authorization", format!("Bearer {token}"));
    }
    app.oneshot(req.body(Body::from(body.to_owned())).expect("request"))
        .await
        .expect("response")
}

async fn patch_with_agent_token(
    app: Router,
    uri: &str,
    body: &str,
    token: Option<&str>,
) -> axum::http::Response<Body> {
    let mut req = Request::patch(uri).header(CONTENT_TYPE, "application/json");
    if let Some(token) = token {
        req = req.header("x-agent-token", token);
    }
    app.oneshot(req.body(Body::from(body.to_owned())).expect("request"))
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

async fn body_bytes(resp: axum::http::Response<Body>) -> Vec<u8> {
    resp.into_body()
        .collect()
        .await
        .expect("collect")
        .to_bytes()
        .to_vec()
}

fn temp_media_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time after epoch")
        .as_nanos();
    let dir =
        std::env::temp_dir().join(format!("agentd-p222-media-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).expect("create media dir");
    dir
}

fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(&mut out, "%{byte:02X}");
        }
    }
    out
}

fn write_attachment(name: &str, bytes: &[u8]) -> String {
    let path = temp_attachment_path(name);
    fs::write(&path, bytes).expect("write attachment");
    path.to_string_lossy().to_string()
}

fn temp_attachment_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("agentd-p221-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).expect("create temp attachment dir");
    dir.join(name)
}

#[tokio::test]
async fn http_healthz_ok() {
    let resp = get(app(FakeRunHost::new()), "/healthz").await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_string(resp).await, "ok");
}

#[tokio::test]
async fn http_stream_replays_message_wakeup_events() {
    let host = FakeRunHost::new();
    host.set_stream_events(vec![
        json!({
            "seq": 1,
            "event": "message",
            "messageId": "msg-old",
            "agent": "codex-old",
            "target": "codex-old"
        }),
        json!({
            "seq": 2,
            "event": "message",
            "messageId": "msg-new",
            "agent": "codex-new",
            "target": "codex-new"
        }),
    ]);

    let resp = get(app(host), "/api/stream?from_seq=1").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("event: message"), "{body}");
    assert!(body.contains(r#""seq":2"#), "{body}");
    assert!(body.contains(r#""messageId":"msg-new""#), "{body}");
    assert!(body.contains(r#""target":"codex-new""#), "{body}");
    assert!(!body.contains("msg-old"), "{body}");
}

#[tokio::test]
async fn http_matrix_outbox_replays_non_matrix_message_wakeups() {
    let host = FakeRunHost::new();
    host.set_stream_events(vec![
        json!({
            "seq": 1,
            "event": "message",
            "messageId": "msg-old",
            "target": "codex-old",
            "source": "api"
        }),
        json!({
            "seq": 2,
            "event": "message",
            "messageId": "msg-matrix",
            "target": "codex-worker",
            "source": "matrix"
        }),
        json!({
            "seq": 3,
            "event": "message",
            "messageId": "msg-api",
            "target": "codex-worker",
            "source": "api"
        }),
        json!({
            "seq": 4,
            "event": "delivery_wakeup",
            "messageId": "delivery-only",
            "target": "codex-worker",
            "source": "api"
        }),
    ]);

    let resp = get(app(host), "/api/matrix/outbox?from_seq=1").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = serde_json::from_str(&body_string(resp).await).expect("outbox json");
    let events = body["events"].as_array().expect("events array");
    assert_eq!(events.len(), 1, "body: {body}");
    assert_eq!(events[0]["seq"], 3);
    assert_eq!(events[0]["event"], "message");
    assert_eq!(events[0]["payload"]["messageId"], "msg-api");
    assert_eq!(events[0]["payload"]["source"], "api");
}

#[tokio::test]
async fn http_router_default_keeps_agent_routes_open_without_auth_config() {
    let resp = post(
        app(FakeRunHost::new()),
        "/api/agents",
        r#"{"name":"codex-dev","runtime":"codex"}"#,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let v: Value = serde_json::from_str(&body_string(resp).await).expect("json");
    assert_eq!(v["agent"]["name"], "codex-dev");
}

#[tokio::test]
async fn http_agent_identity_patch_persists_profile_and_reports_errors() {
    let app = app(FakeRunHost::new());
    let register = post(
        app.clone(),
        "/api/agents",
        &json!({
            "name": "codex-worker",
            "runtime": "codex",
            "runtime_profile": {
                "primary": {
                    "framework": "codex"
                }
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(register.status(), StatusCode::OK);

    let patched = patch(
        app.clone(),
        "/api/agents/codex-worker",
        &json!({ "identity": "Review carefully" }).to_string(),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::OK);
    let patched: Value = serde_json::from_str(&body_string(patched).await).expect("patch json");
    assert_eq!(patched["ok"], true);
    assert_eq!(
        patched["agent"]["runtime_profile"]["identity"],
        "Review carefully"
    );
    assert_eq!(
        patched["agent"]["runtime_profile"]["primary"]["framework"],
        "codex"
    );

    let detail = get(app.clone(), "/api/agents/codex-worker").await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail: Value = serde_json::from_str(&body_string(detail).await).expect("detail json");
    assert_eq!(detail["runtime_profile"]["identity"], "Review carefully");
    assert_eq!(detail["runtime_profile"]["primary"]["framework"], "codex");

    let empty = patch(
        app.clone(),
        "/api/agents/codex-worker",
        &json!({ "identity": " \n\t " }).to_string(),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

    let after_empty = get(app.clone(), "/api/agents/codex-worker").await;
    let after_empty: Value =
        serde_json::from_str(&body_string(after_empty).await).expect("detail json");
    assert_eq!(
        after_empty["runtime_profile"]["identity"],
        "Review carefully"
    );

    let missing = patch(
        app,
        "/api/agents/ghost",
        &json!({ "identity": "Be concise" }).to_string(),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let missing: Value = serde_json::from_str(&body_string(missing).await).expect("missing json");
    assert_eq!(missing["error"], "agent_not_found");
}

#[tokio::test]
async fn http_media_stage_writes_bytes_and_fetch_returns_same_content() {
    let media_dir = temp_media_dir();
    let app = app_with_media(FakeRunHost::new(), media_dir.clone());
    let register = post(
        app.clone(),
        "/api/agents",
        &json!({"name": "codex-a", "runtime": "codex"}).to_string(),
    )
    .await;
    assert_eq!(register.status(), StatusCode::OK);

    let staged = post(
        app.clone(),
        "/api/media/stage",
        &json!({
            "from": "codex-a",
            "source_path": "/tmp/source-note.txt",
            "name": "source-note.txt",
            "mime": "text/plain",
            "kind": "file",
            "content_base64": "aGVsbG8gbWVkaWE="
        })
        .to_string(),
    )
    .await;
    assert_eq!(staged.status(), StatusCode::OK);
    let staged: Value = serde_json::from_str(&body_string(staged).await).expect("stage json");
    let attachment = &staged["attachment"];
    assert_eq!(attachment["staged"], true);
    assert_eq!(attachment["size"], 11);
    assert_eq!(attachment["name"], "source-note.txt");
    assert_eq!(attachment["mime"], "text/plain");
    assert_eq!(attachment["kind"], "file");
    assert_eq!(attachment["source_path"], "/tmp/source-note.txt");
    let staged_path = attachment["path"].as_str().expect("staged path");
    assert!(
        PathBuf::from(staged_path).starts_with(&media_dir),
        "staged path {staged_path} must be inside {media_dir:?}"
    );

    let fetched = get(
        app,
        &format!("/api/media/fetch?path={}", url_encode(staged_path)),
    )
    .await;
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(
        fetched
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        fetched
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok()),
        Some("11")
    );
    assert!(
        fetched
            .headers()
            .get("content-disposition")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.contains("inline") && value.contains("source-note.txt"))
    );
    assert_eq!(body_bytes(fetched).await, b"hello media");
}

#[tokio::test]
async fn http_media_stage_and_fetch_reject_invalid_input() {
    let media_dir = temp_media_dir();
    let app = app_with_media(FakeRunHost::new(), media_dir.clone());

    let unknown = post(
        app.clone(),
        "/api/media/stage",
        &json!({"from": "ghost", "content_base64": "aGVsbG8="}).to_string(),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let register = post(
        app.clone(),
        "/api/agents",
        &json!({"name": "codex-a", "runtime": "codex"}).to_string(),
    )
    .await;
    assert_eq!(register.status(), StatusCode::OK);

    let missing_payload = post(
        app.clone(),
        "/api/media/stage",
        &json!({"from": "codex-a"}).to_string(),
    )
    .await;
    assert_eq!(missing_payload.status(), StatusCode::BAD_REQUEST);

    let outside = get(
        app.clone(),
        "/api/media/fetch?path=/tmp/agentd-p222-outside.txt",
    )
    .await;
    assert_eq!(outside.status(), StatusCode::FORBIDDEN);

    let missing_inside = media_dir.join("missing.txt");
    let missing = get(
        app,
        &format!(
            "/api/media/fetch?path={}",
            url_encode(&missing_inside.to_string_lossy())
        ),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn http_messages_preserve_staged_attachment_metadata() {
    let media_dir = temp_media_dir();
    let app = app_with_media(FakeRunHost::new(), media_dir);
    for agent in ["codex-a", "codex-b"] {
        let resp = post(
            app.clone(),
            "/api/agents",
            &json!({"name": agent, "runtime": "codex"}).to_string(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let staged = post(
        app.clone(),
        "/api/media/stage",
        &json!({
            "from": "codex-a",
            "source_path": "/tmp/original.txt",
            "name": "original.txt",
            "mime": "text/plain",
            "kind": "file",
            "content_base64": "c3RhZ2VkIG1zZw=="
        })
        .to_string(),
    )
    .await;
    assert_eq!(staged.status(), StatusCode::OK);
    let staged: Value = serde_json::from_str(&body_string(staged).await).expect("stage json");

    let sent = post(
        app.clone(),
        "/api/messages",
        &json!({
            "from": "codex-a",
            "to": "codex-b",
            "summary": "staged attachment",
            "full": "staged attachment full",
            "attachments": [staged["attachment"].clone()]
        })
        .to_string(),
    )
    .await;
    assert_eq!(sent.status(), StatusCode::CREATED);

    let inbox = get(app, "/api/inbox/codex-b").await;
    assert_eq!(inbox.status(), StatusCode::OK);
    let inbox: Value = serde_json::from_str(&body_string(inbox).await).expect("inbox json");
    let attachment = &inbox["dm"][0]["attachments"][0];
    assert_eq!(attachment["staged"], true);
    assert_eq!(attachment["source_path"], "/tmp/original.txt");
    assert_eq!(attachment["size"], 10);
    assert_eq!(attachment["mime"], "text/plain");
    assert_eq!(attachment["kind"], "file");
}

#[tokio::test]
async fn http_messages_accept_attachment_metadata_for_direct_and_group() {
    let attachment_path = write_attachment("http-note.txt", b"http attachment");
    let app = app(FakeRunHost::new());

    for agent in ["codex-a", "codex-b"] {
        let resp = post(
            app.clone(),
            "/api/agents",
            &json!({"name": agent, "runtime": "codex"}).to_string(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let group = post(
        app.clone(),
        "/api/groups",
        &json!({"name": "factory", "members": ["codex-a", "codex-b"]}).to_string(),
    )
    .await;
    assert_eq!(group.status(), StatusCode::CREATED);

    let direct = post(
        app.clone(),
        "/api/messages",
        &json!({
            "from": "codex-a",
            "to": "codex-b",
            "summary": "direct attachment",
            "full": "direct attachment full",
            "attachments": [{
                "path": attachment_path,
                "mime": "text/plain"
            }]
        })
        .to_string(),
    )
    .await;
    assert_eq!(direct.status(), StatusCode::CREATED);

    let inbox = get(app.clone(), "/api/inbox/codex-b").await;
    assert_eq!(inbox.status(), StatusCode::OK);
    let inbox: Value = serde_json::from_str(&body_string(inbox).await).expect("inbox json");
    assert_eq!(inbox["dm"][0]["attachments"][0]["name"], "http-note.txt");
    assert_eq!(inbox["dm"][0]["attachments"][0]["staged"], false);

    let group_message = post(
        app.clone(),
        "/api/messages",
        &json!({
            "from": "codex-a",
            "group": "factory",
            "summary": "group attachment",
            "full": "group attachment full",
            "attachments": [{ "path": inbox["dm"][0]["attachments"][0]["path"] }]
        })
        .to_string(),
    )
    .await;
    assert_eq!(group_message.status(), StatusCode::OK);

    let history = get(
        app,
        "/api/groups/factory/messages?agent=codex-b&limit=10&advance=none",
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    let history: Value = serde_json::from_str(&body_string(history).await).expect("history json");
    assert_eq!(
        history["unread"][0]["attachments"][0]["name"],
        "http-note.txt"
    );
    assert_eq!(history["unread"][0]["attachments"][0]["staged"], false);
}

#[tokio::test]
async fn http_agent_chat_task_crud_filters_comments_and_delete() {
    let app = app(FakeRunHost::new());

    let first = post(
        app.clone(),
        "/api/tasks",
        &json!({
            "title": "A",
            "description": "alpha task",
            "assignee": "codex-a",
            "priority": "p1",
            "labels": ["migration", "migration", "http"]
        })
        .to_string(),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = serde_json::from_str(&body_string(first).await).expect("create json");
    assert_eq!(first["ok"], true);
    assert_eq!(first["task"]["title"], "A");
    assert_eq!(first["task"]["status"], "created");
    assert_eq!(first["task"]["priority"], "p1");
    assert_eq!(first["task"]["granularity"], "task");
    assert_eq!(first["task"]["assignee"], "codex-a");
    assert_eq!(first["task"]["labels"], json!(["migration", "http"]));
    assert_eq!(first["task"]["health"], Value::Null);
    let task_id = first["task"]["id"].as_str().expect("task id").to_string();

    let second = post(
        app.clone(),
        "/api/tasks",
        &json!({
            "title": "B",
            "assignee": "codex-b",
            "priority": "p2",
            "labels": ["review"]
        })
        .to_string(),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);

    let filtered = get(
        app.clone(),
        "/api/tasks?assignee=codex-a&status=created,blocked&priority=p1&label=http&limit=1&offset=0",
    )
    .await;
    assert_eq!(filtered.status(), StatusCode::OK);
    let filtered: Value = serde_json::from_str(&body_string(filtered).await).expect("list json");
    assert_eq!(filtered.as_array().expect("array").len(), 1);
    assert_eq!(filtered[0]["id"], task_id);

    let patched = patch(
        app.clone(),
        &format!("/api/tasks/{task_id}"),
        &json!({
            "title": "A updated",
            "priority": "p0",
            "description": "patched",
            "labels": ["patched"]
        })
        .to_string(),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::OK);
    let patched: Value = serde_json::from_str(&body_string(patched).await).expect("patch json");
    assert_eq!(patched["ok"], true);
    assert_eq!(patched["task"]["title"], "A updated");
    assert_eq!(patched["task"]["priority"], "p0");

    let commented = post(
        app.clone(),
        &format!("/api/tasks/{task_id}/comments"),
        &json!({"author": "operator", "text": "looks good"}).to_string(),
    )
    .await;
    assert_eq!(commented.status(), StatusCode::OK);
    let commented: Value =
        serde_json::from_str(&body_string(commented).await).expect("comment json");
    assert_eq!(commented["task"]["comments"][0]["author"], "operator");
    assert_eq!(commented["task"]["comments"][0]["text"], "looks good");

    let agent_tasks = get(app.clone(), "/api/agents/codex-a/tasks").await;
    assert_eq!(agent_tasks.status(), StatusCode::OK);
    let agent_tasks: Value =
        serde_json::from_str(&body_string(agent_tasks).await).expect("agent tasks json");
    assert_eq!(agent_tasks.as_array().expect("array").len(), 1);
    assert_eq!(agent_tasks[0]["id"], task_id);

    let deleted = delete(app.clone(), &format!("/api/tasks/{task_id}")).await;
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted: Value = serde_json::from_str(&body_string(deleted).await).expect("delete json");
    assert_eq!(deleted["ok"], true);
    assert_eq!(deleted["task"]["id"], task_id);

    let missing = get(app, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let missing: Value = serde_json::from_str(&body_string(missing).await).expect("missing json");
    assert_eq!(missing["error"], "task not found");
}

#[tokio::test]
async fn http_agent_chat_task_lifecycle_rejects_invalid_transitions() {
    let app = app(FakeRunHost::new());
    let created = post(
        app.clone(),
        "/api/tasks",
        &json!({"title": "Lifecycle", "assignee": "codex-a"}).to_string(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: Value = serde_json::from_str(&body_string(created).await).expect("create json");
    let task_id = created["task"]["id"].as_str().expect("task id").to_string();

    let bad_start = post(
        app.clone(),
        &format!("/api/tasks/{task_id}/transition"),
        &json!({"status": "in_progress"}).to_string(),
    )
    .await;
    assert_eq!(bad_start.status(), StatusCode::BAD_REQUEST);
    let bad_start: Value =
        serde_json::from_str(&body_string(bad_start).await).expect("bad start json");
    assert!(
        bad_start["error"]
            .as_str()
            .is_some_and(|error| error.contains("cannot transition")),
        "body: {bad_start}"
    );

    let accepted = post(app.clone(), &format!("/api/tasks/{task_id}/accept"), "{}").await;
    assert_eq!(accepted.status(), StatusCode::OK);
    let accepted: Value = serde_json::from_str(&body_string(accepted).await).expect("accept json");
    assert_eq!(accepted["task"]["status"], "accepted");
    assert_ne!(accepted["task"]["started_at"], Value::Null);

    let started = post(
        app.clone(),
        &format!("/api/tasks/{task_id}/transition"),
        &json!({"status": "in_progress"}).to_string(),
    )
    .await;
    assert_eq!(started.status(), StatusCode::OK);
    let started: Value = serde_json::from_str(&body_string(started).await).expect("start json");
    assert_eq!(started["task"]["status"], "in_progress");

    let blocked_without_waiting = post(
        app.clone(),
        &format!("/api/tasks/{task_id}/transition"),
        &json!({"status": "blocked", "waiting_reason": "waiting"}).to_string(),
    )
    .await;
    assert_eq!(blocked_without_waiting.status(), StatusCode::BAD_REQUEST);
    let still_started = get(app.clone(), &format!("/api/tasks/{task_id}")).await;
    let still_started: Value =
        serde_json::from_str(&body_string(still_started).await).expect("task json");
    assert_eq!(still_started["status"], "in_progress");

    let blocked = post(
        app.clone(),
        &format!("/api/tasks/{task_id}/transition"),
        &json!({
            "status": "blocked",
            "waiting_reason": "waiting for deploy",
            "waiting_until": "2026-07-10T00:00:00Z"
        })
        .to_string(),
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::OK);
    let blocked: Value = serde_json::from_str(&body_string(blocked).await).expect("blocked json");
    assert_eq!(blocked["task"]["status"], "blocked");
    assert_eq!(blocked["task"]["waiting_reason"], "waiting for deploy");

    let blocked_to_done = post(
        app.clone(),
        &format!("/api/tasks/{task_id}/transition"),
        &json!({"status": "done"}).to_string(),
    )
    .await;
    assert_eq!(blocked_to_done.status(), StatusCode::BAD_REQUEST);

    let resumed = post(
        app.clone(),
        &format!("/api/tasks/{task_id}/transition"),
        &json!({"status": "in_progress"}).to_string(),
    )
    .await;
    assert_eq!(resumed.status(), StatusCode::OK);
    let resumed: Value = serde_json::from_str(&body_string(resumed).await).expect("resume json");
    assert_eq!(resumed["task"]["status"], "in_progress");
    assert_eq!(resumed["task"]["waiting_reason"], Value::Null);

    let done = post(
        app,
        &format!("/api/tasks/{task_id}/transition"),
        &json!({"status": "done"}).to_string(),
    )
    .await;
    assert_eq!(done.status(), StatusCode::OK);
    let done: Value = serde_json::from_str(&body_string(done).await).expect("done json");
    assert_eq!(done["task"]["status"], "done");
    assert_ne!(done["task"]["completed_at"], Value::Null);
}

#[tokio::test]
async fn http_agent_chat_task_agent_routes_require_assignee_token() {
    let mut auth = AuthConfig::open();
    auth.agent_token_mode = AgentTokenMode::Hard;
    auth.agent_tokens
        .insert("codex-a".to_string(), "agent-secret".to_string());
    let app = app_with_auth(FakeRunHost::new(), auth);

    let created = post(
        app.clone(),
        "/api/tasks",
        &json!({"title": "Auth lifecycle", "assignee": "codex-a"}).to_string(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: Value = serde_json::from_str(&body_string(created).await).expect("create json");
    let task_id = created["task"]["id"].as_str().expect("task id").to_string();

    let missing_token = post_with_agent_token(
        app.clone(),
        &format!("/api/tasks/{task_id}/accept"),
        "{}",
        None,
    )
    .await;
    assert_eq!(missing_token.status(), StatusCode::FORBIDDEN);

    let accepted = post_with_agent_token(
        app,
        &format!("/api/tasks/{task_id}/accept"),
        "{}",
        Some("agent-secret"),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    let accepted: Value = serde_json::from_str(&body_string(accepted).await).expect("accept json");
    assert_eq!(accepted["task"]["status"], "accepted");
}

fn chain_graph_body() -> Value {
    json!({
        "id": "graph_live",
        "owner": "orchestrator",
        "label": "Live graph",
        "nodes": {
            "a": {
                "assignee": "codex-a",
                "description": "Do A"
            },
            "b": {
                "assignee": "codex-b",
                "description": "Do B",
                "depends_on": ["a"]
            }
        }
    })
}

#[tokio::test]
async fn http_agent_chat_task_graph_crud_dispatch_and_node_updates() {
    let app = app(FakeRunHost::new());

    let created = post(
        app.clone(),
        "/api/task-graphs",
        &chain_graph_body().to_string(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: Value = serde_json::from_str(&body_string(created).await).expect("create json");
    assert_eq!(created["ok"], true);
    assert_eq!(created["graph"]["id"], "graph_live");
    assert_eq!(created["graph"]["status"], "active");
    assert_eq!(created["graph"]["nodes"]["a"]["status"], "dispatched");
    assert_eq!(created["graph"]["nodes"]["b"]["status"], "pending");
    assert_ne!(created["graph"]["nodes"]["a"]["message_id"], Value::Null);

    let listed = get(app.clone(), "/api/task-graphs?status=active").await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed: Value = serde_json::from_str(&body_string(listed).await).expect("list json");
    assert_eq!(listed.as_array().expect("array").len(), 1);
    assert_eq!(listed[0]["id"], "graph_live");

    let read = get(app.clone(), "/api/task-graphs/graph_live").await;
    assert_eq!(read.status(), StatusCode::OK);
    let read: Value = serde_json::from_str(&body_string(read).await).expect("read json");
    assert_eq!(read["nodes"]["a"]["status"], "dispatched");

    let patched = patch(
        app.clone(),
        "/api/task-graphs/graph_live/nodes/a",
        &json!({"status": "complete", "result": {"ok": true}}).to_string(),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::OK);
    let patched: Value = serde_json::from_str(&body_string(patched).await).expect("patch json");
    assert_eq!(patched["ok"], true);
    assert_eq!(patched["node"]["status"], "complete");
    assert_eq!(patched["graph"]["nodes"]["b"]["status"], "dispatched");

    let deleted = delete(app, "/api/task-graphs/graph_live").await;
    assert_eq!(deleted.status(), StatusCode::OK);
    let deleted: Value = serde_json::from_str(&body_string(deleted).await).expect("delete json");
    assert_eq!(deleted["ok"], true);
    assert_eq!(deleted["graph"]["status"], "cancelled");
    assert_eq!(deleted["graph"]["nodes"]["a"]["status"], "complete");
    assert_eq!(deleted["graph"]["nodes"]["b"]["status"], "cancelled");
}

#[tokio::test]
async fn http_agent_chat_task_graph_rejects_invalid_graphs_and_requires_assignee_token() {
    let mut auth = AuthConfig::open();
    auth.agent_token_mode = AgentTokenMode::Hard;
    auth.agent_tokens
        .insert("codex-a".to_string(), "agent-secret".to_string());
    let app = app_with_auth(FakeRunHost::new(), auth);

    let cyclic = post(
        app.clone(),
        "/api/task-graphs",
        &json!({
            "owner": "orchestrator",
            "label": "bad graph",
            "nodes": {
                "a": {"assignee": "codex-a", "description": "Do A", "depends_on": ["b"]},
                "b": {"assignee": "codex-b", "description": "Do B", "depends_on": ["a"]}
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(cyclic.status(), StatusCode::BAD_REQUEST);
    let cyclic: Value = serde_json::from_str(&body_string(cyclic).await).expect("cycle json");
    assert!(
        cyclic["error"]
            .as_str()
            .is_some_and(|error| error.contains("dependency cycle")),
        "body: {cyclic}"
    );

    let created = post(
        app.clone(),
        "/api/task-graphs",
        &chain_graph_body().to_string(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);

    let missing_token = patch_with_agent_token(
        app.clone(),
        "/api/task-graphs/graph_live/nodes/a",
        &json!({"status": "complete", "result": {"ok": true}}).to_string(),
        None,
    )
    .await;
    assert_eq!(missing_token.status(), StatusCode::FORBIDDEN);

    let patched = patch_with_agent_token(
        app,
        "/api/task-graphs/graph_live/nodes/a",
        &json!({"status": "complete", "result": {"ok": true}}).to_string(),
        Some("agent-secret"),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::OK);
    let patched: Value = serde_json::from_str(&body_string(patched).await).expect("patch json");
    assert_eq!(patched["node"]["status"], "complete");
}

#[tokio::test]
async fn http_agent_chat_task_graph_result_messages_complete_assigned_nodes() {
    let app = app(FakeRunHost::new());

    let created = post(
        app.clone(),
        "/api/task-graphs",
        &chain_graph_body().to_string(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created: Value = serde_json::from_str(&body_string(created).await).expect("create json");
    let message_id = created["graph"]["nodes"]["a"]["message_id"]
        .as_str()
        .expect("dispatch message id")
        .to_string();

    let spoof = post(
        app.clone(),
        "/api/messages",
        &json!({
            "from": "codex-b",
            "to": "orchestrator",
            "summary": "spoof",
            "full": "spoof",
            "reply_to": message_id,
            "schema": {
                "kind": "task_graph_result",
                "version": 1,
                "payload": {
                    "graphId": "graph_live",
                    "nodeId": "a",
                    "result": {"ok": false}
                }
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(spoof.status(), StatusCode::CREATED);
    let spoof: Value = serde_json::from_str(&body_string(spoof).await).expect("spoof json");
    assert_eq!(spoof["taskGraph"], Value::Null);

    let handled = post(
        app.clone(),
        "/api/messages",
        &json!({
            "from": "codex-a",
            "to": "orchestrator",
            "summary": "done",
            "full": "done",
            "reply_to": message_id,
            "schema": {
                "kind": "task_graph_result",
                "version": 1,
                "payload": {
                    "graphId": "graph_live",
                    "nodeId": "a",
                    "result": {"ok": true}
                }
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(handled.status(), StatusCode::CREATED);
    let handled: Value = serde_json::from_str(&body_string(handled).await).expect("handled json");
    assert_eq!(handled["taskGraph"]["handled"], true);
    assert_eq!(handled["taskGraph"]["graphId"], "graph_live");
    assert_eq!(handled["taskGraph"]["nodeId"], "a");
    assert_eq!(handled["taskGraph"]["status"], "complete");

    let graph = get(app.clone(), "/api/task-graphs/graph_live").await;
    assert_eq!(graph.status(), StatusCode::OK);
    let graph: Value = serde_json::from_str(&body_string(graph).await).expect("graph json");
    assert_eq!(graph["nodes"]["a"]["status"], "complete");
    assert_eq!(graph["nodes"]["a"]["result"]["ok"], true);

    let missing_reply = post(
        app,
        "/api/messages",
        &json!({
            "from": "codex-a",
            "to": "orchestrator",
            "summary": "missing reply",
            "full": "missing reply",
            "schema": {
                "kind": "task_graph_result",
                "version": 1,
                "payload": {
                    "graphId": "graph_live",
                    "nodeId": "a",
                    "result": {"ok": true}
                }
            }
        })
        .to_string(),
    )
    .await;
    assert_eq!(missing_reply.status(), StatusCode::CREATED);
    let missing_reply: Value =
        serde_json::from_str(&body_string(missing_reply).await).expect("missing reply json");
    assert_eq!(missing_reply["taskGraph"], Value::Null);
}

#[tokio::test]
async fn http_agent_chat_pool_scheduler_routes_queue_and_release() {
    let app = app(FakeRunHost::new());

    for body in [
        json!({
            "name": "cod1",
            "role": "coding",
            "capability": "medium",
            "runtime": "codex",
            "tmux_target": "cod1:0.0"
        }),
        json!({
            "name": "wf_implementer",
            "runtime": "codex",
            "tmux_target": "wf_implementer:0.0"
        }),
    ] {
        let registered = post(app.clone(), "/api/agents", &body.to_string()).await;
        assert_eq!(registered.status(), StatusCode::OK);
    }

    let pool = get(app.clone(), "/api/pool").await;
    assert_eq!(pool.status(), StatusCode::OK);
    let pool: Value = serde_json::from_str(&body_string(pool).await).expect("pool json");
    assert_eq!(pool["counts"]["coding"]["medium"], 2);
    assert!(
        pool["agents"]
            .as_array()
            .expect("agents array")
            .iter()
            .any(|agent| agent["name"] == "wf_implementer"
                && agent["role"] == "coding"
                && agent["capability"] == "medium")
    );

    let first = post(
        app.clone(),
        "/api/dispatch",
        &json!({"role": "coding", "capability": "medium", "task": "A", "room": "factory"})
            .to_string(),
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = serde_json::from_str(&body_string(first).await).expect("first json");
    assert_eq!(first["status"], "routed");
    assert_eq!(first["agent"], "cod1");
    assert_eq!(first["reservation"]["agent"], "cod1");

    let second = post(
        app.clone(),
        "/api/dispatch",
        &json!({"role": "coding", "capability": "medium", "task": "B", "room": "factory"})
            .to_string(),
    )
    .await;
    assert_eq!(second.status(), StatusCode::OK);
    let second: Value = serde_json::from_str(&body_string(second).await).expect("second json");
    assert_eq!(second["status"], "routed");
    assert_eq!(second["agent"], "wf_implementer");

    let third = post(
        app.clone(),
        "/api/dispatch",
        &json!({"role": "coding", "capability": "medium", "task": "C", "room": "factory"})
            .to_string(),
    )
    .await;
    assert_eq!(third.status(), StatusCode::OK);
    let third: Value = serde_json::from_str(&body_string(third).await).expect("third json");
    assert_eq!(third["status"], "queued");
    assert_eq!(third["queueDepth"], 1);

    let busy = get(app.clone(), "/api/pool?state=busy").await;
    assert_eq!(busy.status(), StatusCode::OK);
    let busy: Value = serde_json::from_str(&body_string(busy).await).expect("busy json");
    assert_eq!(busy["total"], 2);

    let released = post(
        app,
        "/api/dispatch/release",
        &json!({"agent": "cod1"}).to_string(),
    )
    .await;
    assert_eq!(released.status(), StatusCode::OK);
    let released: Value = serde_json::from_str(&body_string(released).await).expect("release json");
    assert_eq!(released["status"], "drained");
    assert_eq!(released["agent"], "cod1");
    assert_eq!(released["task"], "C");
    assert_eq!(released["room"], "factory");
}

#[tokio::test]
async fn http_agent_chat_scheduler_provision_and_auth() {
    let mut auth = AuthConfig::open();
    auth.api_token = Some("operator-secret".to_string());
    let app = app_with_scheduler_auth(
        FakeRunHost::new(),
        auth,
        SchedulerConfig { max_per_cell: 1 },
    );

    let blocked = post(
        app.clone(),
        "/api/dispatch",
        &json!({"role": "documentation", "capability": "lightweight", "task": "docs"}).to_string(),
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::UNAUTHORIZED);

    let provision = post_with_bearer(
        app.clone(),
        "/api/dispatch",
        &json!({"role": "documentation", "capability": "lightweight", "task": "docs"}).to_string(),
        Some("operator-secret"),
    )
    .await;
    assert_eq!(provision.status(), StatusCode::OK);
    let provision: Value =
        serde_json::from_str(&body_string(provision).await).expect("provision json");
    assert_eq!(provision["status"], "provision");
    assert!(
        provision["name"]
            .as_str()
            .is_some_and(|name| name.starts_with("mx_documentation_lightweight_")),
        "provision body: {provision}"
    );
    assert_eq!(provision["runtime"]["runtime"], "claude");
    assert_eq!(provision["runtime"]["model"], "haiku");

    let queued = post_with_bearer(
        app,
        "/api/dispatch",
        &json!({"role": "documentation", "capability": "lightweight", "task": "docs again"})
            .to_string(),
        Some("operator-secret"),
    )
    .await;
    assert_eq!(queued.status(), StatusCode::OK);
    let queued: Value = serde_json::from_str(&body_string(queued).await).expect("queued json");
    assert_eq!(queued["status"], "queued");
    assert_eq!(queued["queueDepth"], 1);
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
    assert!(body.contains(r"fetch(`/runs/${"), "{body}");
    assert!(body.contains(r"new EventSource(`/runs/${"), "{body}");
    assert!(body.contains(r"/events"), "{body}");
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

async fn register_agent(app: Router, name: &str) {
    let body = json!({
        "name": name,
        "runtime": "codex",
        "workdir": "/tmp/agentd-test"
    });
    let resp = post(app, "/api/agents", &body.to_string()).await;
    assert_eq!(resp.status(), StatusCode::OK, "register {name}");
}

async fn create_group(app: Router, name: &str, members: &[&str]) {
    let body = json!({
        "name": name,
        "members": members,
    });
    let resp = post(app, "/api/groups", &body.to_string()).await;
    assert_eq!(resp.status(), StatusCode::CREATED, "create group {name}");
}

#[tokio::test]
async fn http_delete_group_removes_group_and_members() {
    let app = app(FakeRunHost::new());
    create_group(app.clone(), "factory", &["codex-a", "codex-b"]).await;

    let delete_resp = delete(app.clone(), "/api/groups/factory").await;
    assert_eq!(delete_resp.status(), StatusCode::OK);
    let deleted: Value = serde_json::from_str(&body_string(delete_resp).await).expect("json");
    assert_eq!(deleted["ok"], true);
    assert_eq!(deleted["group"]["name"], "factory");
    assert_eq!(deleted["group"]["members"], json!(["codex-a", "codex-b"]));

    let get_resp = get(app.clone(), "/api/groups/factory").await;
    assert_eq!(get_resp.status(), StatusCode::NOT_FOUND);
    let missing: Value = serde_json::from_str(&body_string(get_resp).await).expect("json");
    assert_eq!(missing["error"], "group_not_found");

    let missing_delete = delete(app, "/api/groups/factory").await;
    assert_eq!(missing_delete.status(), StatusCode::NOT_FOUND);
    let missing_delete_body: Value =
        serde_json::from_str(&body_string(missing_delete).await).expect("json");
    assert_eq!(missing_delete_body["error"], "group_not_found");
}

#[tokio::test]
async fn http_group_message_mentions_land_in_group_inbox() {
    let app = app(FakeRunHost::new());
    for agent in ["codex-a", "codex-b", "codex-c"] {
        register_agent(app.clone(), agent).await;
    }
    create_group(app.clone(), "factory", &["codex-a", "codex-b"]).await;

    let resp = post(
        app.clone(),
        "/api/messages",
        &json!({
            "from": "codex-a",
            "group": "factory",
            "type": "inform",
            "priority": "normal",
            "summary": "group summary",
            "full": "group full",
            "mentions": ["codex-b", "codex-c"]
        })
        .to_string(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let posted: Value = serde_json::from_str(&body_string(resp).await).expect("json");
    assert_eq!(posted["delivery"]["targetKind"], Value::Null);
    assert_eq!(posted["delivery"]["suppressed"], json!(["codex-c"]));
    assert!(
        posted["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning["code"] == "mentions_not_in_group"),
        "{posted}"
    );

    let b_resp = get(app.clone(), "/api/inbox/codex-b").await;
    assert_eq!(b_resp.status(), StatusCode::OK);
    let b_inbox: Value = serde_json::from_str(&body_string(b_resp).await).expect("json");
    assert_eq!(b_inbox["group"].as_array().expect("group").len(), 1);
    assert_eq!(b_inbox["group"][0]["group"], "factory");

    let c_resp = get(app, "/api/inbox/codex-c").await;
    assert_eq!(c_resp.status(), StatusCode::OK);
    let c_inbox: Value = serde_json::from_str(&body_string(c_resp).await).expect("json");
    assert!(c_inbox["group"].as_array().expect("group").is_empty());
}

#[tokio::test]
async fn http_group_messages_preview_and_advance_cursor() {
    let app = app(FakeRunHost::new());
    for agent in ["codex-a", "codex-b"] {
        register_agent(app.clone(), agent).await;
    }
    create_group(app.clone(), "factory", &["codex-a", "codex-b"]).await;
    for summary in ["one", "two", "three"] {
        let resp = post(
            app.clone(),
            "/api/messages",
            &json!({
                "from": "codex-a",
                "group": "factory",
                "summary": summary,
                "full": format!("full {summary}"),
                "mentions": ["codex-b"]
            })
            .to_string(),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK, "post {summary}");
    }

    let resp = get(
        app.clone(),
        "/api/groups/factory/messages?agent=codex-b&advance=none&limit=1&unread_limit=2",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let preview: Value = serde_json::from_str(&body_string(resp).await).expect("json");
    assert_eq!(preview["unread_total"], 3);
    assert_eq!(preview["unread_returned"], 2);
    assert_eq!(preview["unread_omitted"], 1);
    assert_eq!(preview["advance"], "none");

    let resp = get(
        app.clone(),
        "/api/groups/factory/messages?agent=codex-b&advance=all",
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = get(app, "/api/groups/factory/messages?agent=codex-b").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let after: Value = serde_json::from_str(&body_string(resp).await).expect("json");
    assert_eq!(after["unread_total"], 0);
}

#[tokio::test]
async fn http_group_message_rejects_unknown_group_and_non_member_sender() {
    let app = app(FakeRunHost::new());
    for agent in ["codex-a", "codex-b", "codex-c"] {
        register_agent(app.clone(), agent).await;
    }
    create_group(app.clone(), "factory", &["codex-a", "codex-b"]).await;

    let both = post(
        app.clone(),
        "/api/messages",
        &json!({
            "from": "codex-a",
            "to": "codex-b",
            "group": "factory",
            "summary": "bad",
            "full": "bad"
        })
        .to_string(),
    )
    .await;
    assert_eq!(both.status(), StatusCode::BAD_REQUEST);

    let unknown = post(
        app.clone(),
        "/api/messages",
        &json!({
            "from": "codex-a",
            "group": "ghost",
            "summary": "bad",
            "full": "bad"
        })
        .to_string(),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let non_member = post(
        app.clone(),
        "/api/messages",
        &json!({
            "from": "codex-c",
            "group": "factory",
            "summary": "bad",
            "full": "bad"
        })
        .to_string(),
    )
    .await;
    assert_eq!(non_member.status(), StatusCode::FORBIDDEN);

    let non_member_read = get(app.clone(), "/api/groups/factory/messages?agent=codex-c").await;
    assert_eq!(non_member_read.status(), StatusCode::FORBIDDEN);

    let missing_agent = get(app, "/api/groups/factory/messages").await;
    assert_eq!(missing_agent.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn dashboard_shell_live_state_refresh_remains_read_only() {
    let resp = get(app(FakeRunHost::new()), "/dashboard").await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains(r#"fetch("/runs")"#), "{body}");
    assert!(body.contains(r"fetch(`/runs/${"), "{body}");
    assert!(body.contains(r"new EventSource(`/runs/${"), "{body}");
    assert!(body.contains(r"/events"), "{body}");
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
    assert!(body.contains(r"fetch(`/runs/${"), "{body}");
    assert!(body.contains(r"new EventSource(`/runs/${"), "{body}");
    assert!(body.contains(r"/events"), "{body}");
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
