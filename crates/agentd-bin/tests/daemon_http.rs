//! P0.9 9b: the daemon assembles the production host behind the HTTP/SSE router,
//! driven by `tower::oneshot`. Names match `specs/e2e/p96-daemon-assembly.spec.md`.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use agentd_bin::{
    ProductionRunHost, SystemClock, daemon,
    host::{AgentLifecycle, AgentLifecycleShutdown, AgentLifecycleShutdownReport},
};
use agentd_core::CoreError;
use agentd_core::ports::AgentBackend;
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::{AgentHandle, AgentId, BackendKind, CliKind, RunId, SpawnRequest};
use agentd_store::{SqliteStore, run_repo};
use agentd_surface::http::{AgentTokenMode, AuthConfig, MediaConfig, OPERATOR_READ_COOKIE_NAME};
use agentd_surface::mcp_server::dispatch;
use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
}

#[derive(Clone, Debug)]
struct SharedBackend(Arc<FakeBackend>);

#[async_trait::async_trait]
impl AgentBackend for SharedBackend {
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        self.0.spawn(req).await
    }
}

#[derive(Clone, Debug, Default)]
struct RecordingLifecycle {
    shutdowns: Arc<Mutex<Vec<(String, PathBuf)>>>,
    rebinds: Arc<Mutex<Vec<String>>>,
    rebind_results: Arc<Mutex<BTreeMap<String, Option<AgentHandle>>>>,
}

impl RecordingLifecycle {
    fn shutdowns(&self) -> Vec<(String, PathBuf)> {
        self.shutdowns.lock().expect("shutdown lock").clone()
    }

    fn rebinds(&self) -> Vec<String> {
        self.rebinds.lock().expect("rebind lock").clone()
    }

    fn set_rebind_result(&self, target: &str, handle: Option<AgentHandle>) {
        self.rebind_results
            .lock()
            .expect("rebind result lock")
            .insert(target.to_string(), handle);
    }
}

#[async_trait::async_trait]
impl AgentLifecycle for RecordingLifecycle {
    async fn shutdown(
        &self,
        handle: &AgentHandle,
        opts: AgentLifecycleShutdown,
    ) -> Result<AgentLifecycleShutdownReport, CoreError> {
        self.shutdowns
            .lock()
            .expect("shutdown lock")
            .push((handle.address.clone(), opts.archive_to));
        Ok(AgentLifecycleShutdownReport {
            method: "kill".to_string(),
            final_capture_sha: "fake-sha".to_string(),
        })
    }

    async fn rebind(&self, target: &str) -> Result<Option<AgentHandle>, CoreError> {
        self.rebinds
            .lock()
            .expect("rebind lock")
            .push(target.to_string());
        Ok(self
            .rebind_results
            .lock()
            .expect("rebind result lock")
            .get(target)
            .cloned()
            .unwrap_or(None))
    }
}

fn lifecycle_handle(agent: &str, target: &str) -> AgentHandle {
    let session_name = target.split(':').next().unwrap_or(target).to_string();
    AgentHandle {
        agent_id: AgentId::parsed(agent),
        backend: BackendKind::NativeRuntime,
        address: target.to_string(),
        pane_id: Some("%42".to_string()),
        pid: Some(4242),
        session_name,
        spawned_at: SystemTime::UNIX_EPOCH,
    }
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

async fn get_with_headers(app: Router, uri: &str, headers: HeaderMap) -> (StatusCode, String) {
    let mut req = Request::get(uri);
    for (key, value) in headers {
        if let Some(key) = key {
            req = req.header(key, value);
        }
    }
    let resp = app
        .oneshot(req.body(Body::empty()).expect("request"))
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

async fn patch(app: Router, uri: &str, body: serde_json::Value) -> (StatusCode, String) {
    let resp = app
        .oneshot(
            Request::patch(uri)
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

async fn post_with_headers(
    app: Router,
    uri: &str,
    body: serde_json::Value,
    headers: HeaderMap,
) -> (StatusCode, String) {
    let mut req = Request::post(uri).header("content-type", "application/json");
    for (key, value) in headers {
        if let Some(key) = key {
            req = req.header(key, value);
        }
    }
    let resp = app
        .oneshot(req.body(Body::from(body.to_string())).expect("request"))
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

fn bearer(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {token}").parse().expect("header value"),
    );
    headers
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

fn agent_token(token: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-agent-token", token.parse().expect("header value"));
    headers
}

fn remote_bearer(token: &str) -> HeaderMap {
    let mut headers = bearer(token);
    headers.insert(
        "x-forwarded-for",
        "203.0.113.42".parse().expect("header value"),
    );
    headers
}

fn auth_config() -> AuthConfig {
    AuthConfig {
        api_token: Some("operator-secret".to_string()),
        agent_token_mode: AgentTokenMode::Hard,
        agent_tokens: std::collections::BTreeMap::new(),
    }
}

async fn empty_auth_router(auth: AuthConfig) -> (Router, tempfile::TempDir, Arc<FakeBackend>) {
    let backend = Arc::new(FakeBackend::new());
    let (app, dir, backend) = empty_router_with_backend_and_auth(Arc::clone(&backend), auth).await;
    (app, dir, backend)
}

async fn empty_enterprise_operator_router() -> (Router, tempfile::TempDir) {
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
    let app = daemon::build_enterprise_operator_router(
        Arc::new(host),
        auth_config(),
        MediaConfig::new(dir.path().join("media")),
    )
    .expect("enterprise operator router");
    (app, dir)
}

async fn empty_router_with_backend_and_auth(
    backend: Arc<FakeBackend>,
    auth: AuthConfig,
) -> (Router, tempfile::TempDir, Arc<FakeBackend>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let host = ProductionRunHost::new(
        store,
        Box::new(SharedBackend(Arc::clone(&backend))),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    (
        daemon::build_router_with_auth(Arc::new(host), auth),
        dir,
        backend,
    )
}

#[tokio::test]
async fn daemon_router_tools_call_routes_to_dispatch() {
    let (app, _dir) = router_with_started_run().await;
    let (status, body) = post(
        app,
        "/tools/call",
        serde_json::json!({
            "name": "query_run",
            "arguments": { "run_id": "r1" }
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["current_node"], "propose_spec", "body: {body}");
}

#[tokio::test]
async fn daemon_router_operator_message_write_feeds_check_inbox_tool() {
    let (app, _dir) = empty_router().await;

    let (post_status, post_body) = post(
        app.clone(),
        "/api/messages",
        serde_json::json!({
            "message_id": "msg_direct_http_1",
            "ts": 1_780_049_205_450_i64,
            "from": "alex",
            "to": "codex-worker",
            "type": "human",
            "priority": "normal",
            "summary": "smoke failed",
            "full": "The real execute smoke failed; inspect logs and report root cause.",
            "source": "api",
            "trustLevel": "operator",
            "fromId": "alex"
        }),
    )
    .await;
    assert_eq!(post_status, StatusCode::CREATED, "body: {post_body}");
    let posted: serde_json::Value = serde_json::from_str(&post_body).expect("json");
    assert_eq!(posted["ok"], true);
    assert_eq!(posted["message"]["id"], "msg_direct_http_1");

    let (pull_status, pull_body) = post(
        app.clone(),
        "/tools/call",
        serde_json::json!({
            "name": "check_inbox",
            "arguments": { "agent_id": "codex-worker", "drain": true }
        }),
    )
    .await;
    assert_eq!(pull_status, StatusCode::OK, "body: {pull_body}");
    let inbox: serde_json::Value = serde_json::from_str(&pull_body).expect("json");
    assert_eq!(inbox["dm"].as_array().expect("dm array").len(), 1);
    assert_eq!(inbox["dm"][0]["summary"], "smoke failed");
    assert_eq!(
        inbox["dm"][0]["full"],
        "The real execute smoke failed; inspect logs and report root cause."
    );
    assert_eq!(inbox["group"].as_array().expect("group array").len(), 0);

    let (second_status, second_body) = post(
        app,
        "/tools/call",
        serde_json::json!({
            "name": "check_inbox",
            "arguments": { "agent_id": "codex-worker", "drain": false }
        }),
    )
    .await;
    assert_eq!(second_status, StatusCode::OK, "body: {second_body}");
    let second: serde_json::Value = serde_json::from_str(&second_body).expect("json");
    assert_eq!(second["dm"].as_array().expect("dm array").len(), 0);
}

#[tokio::test]
async fn daemon_router_message_write_appends_relay_stream_event() {
    let (app, _dir) = empty_router().await;

    let (post_status, post_body) = post(
        app.clone(),
        "/api/messages",
        serde_json::json!({
            "message_id": "msg_stream_direct_1",
            "from": "alex",
            "to": "codex-worker",
            "type": "human",
            "priority": "urgent",
            "summary": "wake up",
            "full": "Call check_inbox now.",
            "source": "api"
        }),
    )
    .await;
    assert_eq!(post_status, StatusCode::CREATED, "body: {post_body}");

    let (stream_status, stream_body) = get(app, "/api/stream").await;
    assert_eq!(stream_status, StatusCode::OK, "body: {stream_body}");
    assert!(stream_body.contains("event: message"), "{stream_body}");
    assert!(
        stream_body.contains(r#""messageId":"msg_stream_direct_1""#),
        "{stream_body}"
    );
    assert!(
        stream_body.contains(r#""target":"codex-worker""#),
        "{stream_body}"
    );
}

#[tokio::test]
async fn daemon_router_tools_call_send_message_feeds_check_inbox() {
    let (app, _dir) = empty_router().await;

    let (send_status, send_body) = post(
        app.clone(),
        "/tools/call",
        serde_json::json!({
            "name": "send_message",
            "arguments": {
                "from_agent": "codex-worker",
                "to": "codex-reviewer",
                "summary": "review direct message",
                "full": "Please review the direct send_message implementation.",
                "type": "request",
                "priority": "high"
            }
        }),
    )
    .await;
    assert_eq!(send_status, StatusCode::OK, "body: {send_body}");
    let sent: serde_json::Value = serde_json::from_str(&send_body).expect("json");
    assert_eq!(sent["ok"], true);
    assert!(
        sent["message"]["id"].as_str().is_some(),
        "body: {send_body}"
    );
    assert_eq!(sent["message"]["from"], "codex-worker");
    assert_eq!(sent["message"]["to"], "codex-reviewer");
    assert_eq!(sent["message"]["type"], "request");
    assert_eq!(sent["message"]["priority"], "high");

    let (pull_status, pull_body) = post(
        app.clone(),
        "/tools/call",
        serde_json::json!({
            "name": "check_inbox",
            "arguments": { "agent_id": "codex-reviewer", "drain": true }
        }),
    )
    .await;
    assert_eq!(pull_status, StatusCode::OK, "body: {pull_body}");
    let inbox: serde_json::Value = serde_json::from_str(&pull_body).expect("json");
    assert_eq!(inbox["dm"].as_array().expect("dm array").len(), 1);
    assert_eq!(inbox["dm"][0]["summary"], "review direct message");
    assert_eq!(
        inbox["dm"][0]["full"],
        "Please review the direct send_message implementation."
    );

    let (second_status, second_body) = post(
        app,
        "/tools/call",
        serde_json::json!({
            "name": "check_inbox",
            "arguments": { "agent_id": "codex-reviewer", "drain": false }
        }),
    )
    .await;
    assert_eq!(second_status, StatusCode::OK, "body: {second_body}");
    let second: serde_json::Value = serde_json::from_str(&second_body).expect("json");
    assert_eq!(second["dm"].as_array().expect("dm array").len(), 0);
}

#[tokio::test]
async fn daemon_router_group_message_persists_mentions_and_group_cursor() {
    let (app, dir) = empty_router().await;
    for agent in ["codex-a", "codex-b"] {
        let (status, body) = post(
            app.clone(),
            "/api/agents",
            serde_json::json!({
                "name": agent,
                "runtime": "codex",
                "workdir": "/tmp/agentd-test"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "register {agent}: {body}");
    }
    let (group_status, group_body) = post(
        app.clone(),
        "/api/groups",
        serde_json::json!({
            "name": "factory",
            "members": ["codex-a", "codex-b"]
        }),
    )
    .await;
    assert_eq!(group_status, StatusCode::CREATED, "body: {group_body}");

    let (post_status, post_body) = post(
        app,
        "/api/messages",
        serde_json::json!({
            "from": "codex-a",
            "group": "factory",
            "summary": "persistent group",
            "full": "persistent group full",
            "mentions": ["codex-b"]
        }),
    )
    .await;
    assert_eq!(post_status, StatusCode::OK, "body: {post_body}");

    let app = router_for_existing_dir(&dir).await;
    let (inbox_status, inbox_body) = get(app.clone(), "/api/inbox/codex-b").await;
    assert_eq!(inbox_status, StatusCode::OK, "body: {inbox_body}");
    let inbox: serde_json::Value = serde_json::from_str(&inbox_body).expect("json");
    assert_eq!(inbox["group"].as_array().expect("group").len(), 1);
    assert_eq!(inbox["group"][0]["summary"], "persistent group");

    let (consume_status, consume_body) = get(
        app,
        "/api/groups/factory/messages?agent=codex-b&advance=all",
    )
    .await;
    assert_eq!(consume_status, StatusCode::OK, "body: {consume_body}");
    let consumed: serde_json::Value = serde_json::from_str(&consume_body).expect("json");
    assert_eq!(consumed["unread_total"], 1);

    let app = router_for_existing_dir(&dir).await;
    let (after_status, after_body) = get(app, "/api/groups/factory/messages?agent=codex-b").await;
    assert_eq!(after_status, StatusCode::OK, "body: {after_body}");
    let after: serde_json::Value = serde_json::from_str(&after_body).expect("json");
    assert_eq!(after["unread_total"], 0);
}

#[tokio::test]
async fn daemon_router_agent_chat_task_crud_persists_after_router_rebuild() {
    let (app, dir) = empty_router().await;

    let (create_status, create_body) = post(
        app.clone(),
        "/api/tasks",
        serde_json::json!({
            "title": "Persisted live task",
            "description": "created through production HTTP",
            "assignee": "codex-a",
            "priority": "p1",
            "labels": ["migration"]
        }),
    )
    .await;
    assert_eq!(create_status, StatusCode::OK, "body: {create_body}");
    let created: serde_json::Value = serde_json::from_str(&create_body).expect("create json");
    let task_id = created["task"]["id"].as_str().expect("task id").to_string();

    let (accept_status, accept_body) = post(
        app.clone(),
        &format!("/api/tasks/{task_id}/accept"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(accept_status, StatusCode::OK, "body: {accept_body}");

    let (comment_status, comment_body) = post(
        app,
        &format!("/api/tasks/{task_id}/comments"),
        serde_json::json!({
            "author": "operator",
            "text": "survives restart"
        }),
    )
    .await;
    assert_eq!(comment_status, StatusCode::OK, "body: {comment_body}");

    let rebuilt = router_for_existing_dir(&dir).await;
    let (get_status, get_body) = get(rebuilt.clone(), &format!("/api/tasks/{task_id}")).await;
    assert_eq!(get_status, StatusCode::OK, "body: {get_body}");
    let task: serde_json::Value = serde_json::from_str(&get_body).expect("task json");
    assert_eq!(task["title"], "Persisted live task");
    assert_eq!(task["status"], "accepted");
    assert_eq!(task["assignee"], "codex-a");
    assert_eq!(task["comments"][0]["text"], "survives restart");

    let (agent_status, agent_body) = get(rebuilt, "/api/agents/codex-a/tasks").await;
    assert_eq!(agent_status, StatusCode::OK, "body: {agent_body}");
    let tasks: serde_json::Value = serde_json::from_str(&agent_body).expect("agent tasks json");
    assert_eq!(tasks.as_array().expect("array").len(), 1);
    assert_eq!(tasks[0]["id"], task_id);
}

#[tokio::test]
async fn daemon_router_agent_chat_task_graphs_persist_after_router_rebuild() {
    let (app, dir) = empty_router().await;

    let (create_status, create_body) = post(
        app.clone(),
        "/api/task-graphs",
        serde_json::json!({
            "id": "graph_daemon",
            "owner": "orchestrator",
            "label": "Daemon graph",
            "nodes": {
                "a": {
                    "assignee": "codex-a",
                    "description": "Do daemon A"
                },
                "b": {
                    "assignee": "codex-b",
                    "description": "Do daemon B",
                    "depends_on": ["a"]
                }
            }
        }),
    )
    .await;
    assert_eq!(create_status, StatusCode::OK, "body: {create_body}");
    let created: serde_json::Value = serde_json::from_str(&create_body).expect("create json");
    let dispatch_id = created["graph"]["nodes"]["a"]["message_id"]
        .as_str()
        .expect("dispatch message id")
        .to_string();
    assert_eq!(created["graph"]["nodes"]["a"]["status"], "dispatched");
    assert_eq!(created["graph"]["nodes"]["b"]["status"], "pending");

    let (inbox_status, inbox_body) = get(app.clone(), "/api/inbox/codex-a").await;
    assert_eq!(inbox_status, StatusCode::OK, "body: {inbox_body}");
    let inbox: serde_json::Value = serde_json::from_str(&inbox_body).expect("inbox json");
    assert_eq!(inbox["dm"].as_array().expect("dm").len(), 1);
    assert_eq!(inbox["dm"][0]["id"], dispatch_id);
    assert_eq!(inbox["dm"][0]["schema"]["kind"], "task_graph_dispatch");
    assert_eq!(
        inbox["dm"][0]["schema"]["payload"]["graphId"],
        "graph_daemon"
    );
    assert_eq!(inbox["dm"][0]["schema"]["payload"]["nodeId"], "a");

    let (patch_status, patch_body) = patch(
        app,
        "/api/task-graphs/graph_daemon/nodes/a",
        serde_json::json!({
            "status": "complete",
            "result": {"ok": true}
        }),
    )
    .await;
    assert_eq!(patch_status, StatusCode::OK, "body: {patch_body}");
    let patched: serde_json::Value = serde_json::from_str(&patch_body).expect("patch json");
    assert_eq!(patched["graph"]["nodes"]["a"]["status"], "complete");
    assert_eq!(patched["graph"]["nodes"]["b"]["status"], "dispatched");

    let rebuilt = router_for_existing_dir(&dir).await;
    let (get_status, get_body) = get(rebuilt.clone(), "/api/task-graphs/graph_daemon").await;
    assert_eq!(get_status, StatusCode::OK, "body: {get_body}");
    let graph: serde_json::Value = serde_json::from_str(&get_body).expect("graph json");
    assert_eq!(graph["nodes"]["a"]["status"], "complete");
    assert_eq!(graph["nodes"]["a"]["result"]["ok"], true);
    assert_eq!(graph["nodes"]["b"]["status"], "dispatched");
    assert_ne!(graph["nodes"]["b"]["message_id"], serde_json::Value::Null);

    let (b_inbox_status, b_inbox_body) = get(rebuilt, "/api/inbox/codex-b").await;
    assert_eq!(b_inbox_status, StatusCode::OK, "body: {b_inbox_body}");
    let b_inbox: serde_json::Value = serde_json::from_str(&b_inbox_body).expect("b inbox json");
    assert_eq!(b_inbox["dm"].as_array().expect("dm").len(), 1);
    assert_eq!(b_inbox["dm"][0]["schema"]["kind"], "task_graph_dispatch");
    assert_eq!(
        b_inbox["dm"][0]["schema"]["payload"]["dependencyResults"][0]["nodeId"],
        "a"
    );
}

#[tokio::test]
async fn daemon_router_agent_chat_scheduler_persists_after_router_rebuild() {
    let (app, dir) = empty_router().await;

    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "cod1",
            "role": "coding",
            "capability": "medium",
            "runtime": "codex",
            "native_runtime_ref": "native://rs_cod1/ra_active",
            "workdir": "/tmp/agentd/cod1"
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (dispatch_status, dispatch_body) = post(
        app,
        "/api/dispatch",
        serde_json::json!({
            "role": "coding",
            "capability": "medium",
            "task": "daemon work",
            "room": "factory"
        }),
    )
    .await;
    assert_eq!(dispatch_status, StatusCode::OK, "body: {dispatch_body}");
    let dispatched: serde_json::Value =
        serde_json::from_str(&dispatch_body).expect("dispatch json");
    assert_eq!(dispatched["status"], "routed");
    assert_eq!(dispatched["agent"], "cod1");

    let rebuilt = router_for_existing_dir(&dir).await;
    let (busy_status, busy_body) = get(rebuilt.clone(), "/api/pool?state=busy").await;
    assert_eq!(busy_status, StatusCode::OK, "body: {busy_body}");
    let busy: serde_json::Value = serde_json::from_str(&busy_body).expect("busy json");
    assert_eq!(busy["total"], 1);
    assert_eq!(busy["agents"][0]["name"], "cod1");
    assert_eq!(busy["agents"][0]["busy"], true);

    let (release_status, release_body) = post(
        rebuilt,
        "/api/dispatch/release",
        serde_json::json!({"agent": "cod1"}),
    )
    .await;
    assert_eq!(release_status, StatusCode::OK, "body: {release_body}");
    let released: serde_json::Value = serde_json::from_str(&release_body).expect("release json");
    assert_eq!(released["status"], "released");
    assert_eq!(released["agent"], "cod1");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn daemon_router_task_graph_scheduler_routes_and_releases_nodes() {
    let (app, _dir) = empty_router().await;

    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "cod1",
            "role": "coding",
            "capability": "medium",
            "runtime": "codex",
            "native_runtime_ref": "native://rs_cod1/ra_active",
            "workdir": "/tmp/agentd/cod1"
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (create_status, create_body) = post(
        app.clone(),
        "/api/task-graphs",
        serde_json::json!({
            "id": "graph_sched_daemon",
            "owner": "orchestrator",
            "label": "Scheduled daemon graph",
            "nodes": {
                "a": {
                    "role": "coding",
                    "capability": "medium",
                    "description": "Do scheduled daemon A"
                },
                "b": {
                    "role": "coding",
                    "capability": "medium",
                    "description": "Do scheduled daemon B"
                }
            }
        }),
    )
    .await;
    assert_eq!(create_status, StatusCode::OK, "body: {create_body}");
    let created: serde_json::Value = serde_json::from_str(&create_body).expect("create json");
    assert_eq!(created["graph"]["nodes"]["a"]["status"], "dispatched");
    assert_eq!(created["graph"]["nodes"]["a"]["assignee"], "cod1");
    assert_eq!(created["graph"]["nodes"]["b"]["status"], "pending");
    assert_eq!(created["graph"]["nodes"]["b"]["schedulerStatus"], "queued");

    let (busy_status, busy_body) = get(app.clone(), "/api/pool?state=busy").await;
    assert_eq!(busy_status, StatusCode::OK, "body: {busy_body}");
    let busy: serde_json::Value = serde_json::from_str(&busy_body).expect("busy json");
    assert_eq!(busy["total"], 1);
    assert_eq!(busy["agents"][0]["name"], "cod1");
    assert_eq!(busy["agents"][0]["busy"], true);

    let (inbox_status, inbox_body) = get(app.clone(), "/api/inbox/cod1").await;
    assert_eq!(inbox_status, StatusCode::OK, "body: {inbox_body}");
    let inbox: serde_json::Value = serde_json::from_str(&inbox_body).expect("inbox json");
    assert_eq!(inbox["dm"].as_array().expect("dm").len(), 1);
    let first_message_id = inbox["dm"][0]["id"]
        .as_str()
        .expect("first message id")
        .to_string();
    let reservation_id = inbox["dm"][0]["schema"]["payload"]["schedulerReservationId"]
        .as_str()
        .expect("scheduler reservation id")
        .to_string();
    assert_eq!(
        inbox["dm"][0]["schema"]["payload"]["graphId"],
        "graph_sched_daemon"
    );
    assert_eq!(inbox["dm"][0]["schema"]["payload"]["nodeId"], "a");
    assert_eq!(
        created["graph"]["nodes"]["a"]["schedulerReservationId"],
        reservation_id
    );

    let (result_status, result_body) = post(
        app.clone(),
        "/api/messages",
        serde_json::json!({
            "from": "cod1",
            "to": "orchestrator",
            "summary": "done",
            "full": "done",
            "reply_to": first_message_id,
            "schema": {
                "kind": "task_graph_result",
                "version": 1,
                "payload": {
                    "graphId": "graph_sched_daemon",
                    "nodeId": "a",
                    "result": {"ok": true}
                }
            }
        }),
    )
    .await;
    assert_eq!(result_status, StatusCode::CREATED, "body: {result_body}");
    let result: serde_json::Value = serde_json::from_str(&result_body).expect("result json");
    assert_eq!(result["taskGraph"]["handled"], true);
    assert_eq!(
        result["taskGraph"]["graph"]["nodes"]["b"]["schedulerStatus"],
        "drained"
    );
    assert_eq!(
        result["taskGraph"]["graph"]["nodes"]["b"]["assignee"],
        "cod1"
    );

    let (inbox_status, inbox_body) = get(app, "/api/inbox/cod1").await;
    assert_eq!(inbox_status, StatusCode::OK, "body: {inbox_body}");
    let inbox: serde_json::Value = serde_json::from_str(&inbox_body).expect("inbox json");
    assert_eq!(inbox["dm"].as_array().expect("dm").len(), 2);
    assert_eq!(inbox["dm"][1]["schema"]["payload"]["nodeId"], "b");
}

#[tokio::test]
async fn daemon_router_task_graph_scheduler_rejects_spoofed_result_messages() {
    let (app, _dir) = empty_router().await;

    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "cod1",
            "role": "coding",
            "capability": "medium",
            "runtime": "codex",
            "native_runtime_ref": "native://rs_cod1/ra_active",
            "workdir": "/tmp/agentd/cod1"
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (create_status, create_body) = post(
        app.clone(),
        "/api/task-graphs",
        serde_json::json!({
            "id": "graph_spoof_sched",
            "owner": "orchestrator",
            "label": "Spoof scheduled graph",
            "nodes": {
                "a": {
                    "role": "coding",
                    "capability": "medium",
                    "description": "Do scheduled spoof A"
                }
            }
        }),
    )
    .await;
    assert_eq!(create_status, StatusCode::OK, "body: {create_body}");
    let created: serde_json::Value = serde_json::from_str(&create_body).expect("create json");
    let message_id = created["graph"]["nodes"]["a"]["message_id"]
        .as_str()
        .expect("dispatch message id")
        .to_string();

    let spoof_body = serde_json::json!({
        "from": "codex-other",
        "to": "orchestrator",
        "summary": "spoof",
        "full": "spoof",
        "reply_to": message_id,
        "schema": {
            "kind": "task_graph_result",
            "version": 1,
            "payload": {
                "graphId": "graph_spoof_sched",
                "nodeId": "a",
                "result": {"ok": false}
            }
        }
    });
    let (spoof_status, spoof_response) = post(app.clone(), "/api/messages", spoof_body).await;
    assert_eq!(spoof_status, StatusCode::CREATED, "body: {spoof_response}");
    let spoof: serde_json::Value = serde_json::from_str(&spoof_response).expect("spoof json");
    assert_eq!(spoof["taskGraph"], serde_json::Value::Null);

    let missing_reply_body = serde_json::json!({
        "from": "cod1",
        "to": "orchestrator",
        "summary": "missing reply",
        "full": "missing reply",
        "schema": {
            "kind": "task_graph_result",
            "version": 1,
            "payload": {
                "graphId": "graph_spoof_sched",
                "nodeId": "a",
                "result": {"ok": true}
            }
        }
    });
    let (missing_status, missing_response) =
        post(app.clone(), "/api/messages", missing_reply_body).await;
    assert_eq!(
        missing_status,
        StatusCode::CREATED,
        "body: {missing_response}"
    );
    let missing: serde_json::Value = serde_json::from_str(&missing_response).expect("missing json");
    assert_eq!(missing["taskGraph"], serde_json::Value::Null);

    let (busy_status, busy_body) = get(app.clone(), "/api/pool?state=busy").await;
    assert_eq!(busy_status, StatusCode::OK, "body: {busy_body}");
    let busy: serde_json::Value = serde_json::from_str(&busy_body).expect("busy json");
    assert_eq!(busy["total"], 1);
    assert_eq!(busy["agents"][0]["busy"], true);

    let (graph_status, graph_body) = get(app, "/api/task-graphs/graph_spoof_sched").await;
    assert_eq!(graph_status, StatusCode::OK, "body: {graph_body}");
    let graph: serde_json::Value = serde_json::from_str(&graph_body).expect("graph json");
    assert_eq!(graph["nodes"]["a"]["status"], "dispatched");
}

#[tokio::test]
async fn daemon_router_media_stage_fetch_survives_router_rebuild() {
    let (app, dir) = empty_router_with_media().await;
    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({"name": "codex-a", "runtime": "codex"}),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (stage_status, stage_body) = post(
        app,
        "/api/media/stage",
        serde_json::json!({
            "from": "codex-a",
            "source_path": "/tmp/daemon-media.txt",
            "name": "daemon-media.txt",
            "mime": "text/plain",
            "kind": "file",
            "content_base64": "ZGFlbW9uIG1lZGlh"
        }),
    )
    .await;
    assert_eq!(stage_status, StatusCode::OK, "body: {stage_body}");
    let staged: serde_json::Value = serde_json::from_str(&stage_body).expect("stage json");
    let staged_path = staged["attachment"]["path"]
        .as_str()
        .expect("staged path")
        .to_string();
    assert!(
        staged_path.starts_with(&dir.path().join("media").to_string_lossy().to_string()),
        "staged path should live under temp media dir: {staged_path}"
    );

    let rebuilt = router_for_existing_dir_with_media(&dir).await;
    let (fetch_status, fetch_body) = get(
        rebuilt,
        &format!("/api/media/fetch?path={}", url_encode(&staged_path)),
    )
    .await;
    assert_eq!(fetch_status, StatusCode::OK, "body: {fetch_body}");
    assert_eq!(fetch_body, "daemon media");
}

#[tokio::test]
async fn daemon_router_message_write_requires_bearer_when_configured() {
    let (app, _dir, _backend) = empty_auth_router(auth_config()).await;
    let body = serde_json::json!({
        "message_id": "msg_direct_auth_1",
        "from": "alex",
        "to": "codex-worker",
        "summary": "auth check",
        "full": "auth check"
    });

    let (blocked_status, _blocked_body) = post(app.clone(), "/api/messages", body.clone()).await;
    assert_eq!(blocked_status, StatusCode::UNAUTHORIZED);

    let (ok_status, ok_body) =
        post_with_headers(app, "/api/messages", body, bearer("operator-secret")).await;
    assert_eq!(ok_status, StatusCode::CREATED, "body: {ok_body}");
    let stored: serde_json::Value = serde_json::from_str(&ok_body).expect("json");
    assert_eq!(stored["message"]["id"], "msg_direct_auth_1");
}

#[tokio::test]
async fn daemon_router_agent_operator_routes_require_bearer_when_configured() {
    let (app, _dir, _backend) = empty_auth_router(auth_config()).await;

    let (missing_status, _missing_body) = get(app.clone(), "/api/agents").await;
    assert_eq!(missing_status, StatusCode::UNAUTHORIZED);

    let (wrong_status, _wrong_body) =
        get_with_headers(app.clone(), "/api/agents", bearer("wrong-secret")).await;
    assert_eq!(wrong_status, StatusCode::UNAUTHORIZED);

    let (ok_status, ok_body) =
        get_with_headers(app, "/api/agents", bearer("operator-secret")).await;
    assert_eq!(ok_status, StatusCode::OK, "body: {ok_body}");
    let v: serde_json::Value = serde_json::from_str(&ok_body).expect("json");
    assert!(v.as_array().expect("agent list array").is_empty());
}

#[tokio::test]
async fn daemon_router_agent_start_and_launch_env_reject_remote_operator_requests() {
    let (app, dir, backend) = empty_auth_router(auth_config()).await;
    let workdir = dir.path().join("codex-worker");
    std::fs::create_dir_all(&workdir).expect("workdir");
    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "codex-worker",
            "runtime": "codex",
            "workdir": workdir.to_string_lossy()
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (launch_status, _launch_body) = get_with_headers(
        app.clone(),
        "/api/agents/codex-worker/launch-env",
        remote_bearer("operator-secret"),
    )
    .await;
    assert_eq!(launch_status, StatusCode::FORBIDDEN);

    let (start_status, _start_body) = post_with_headers(
        app.clone(),
        "/api/agents/codex-worker/start",
        serde_json::json!({}),
        remote_bearer("operator-secret"),
    )
    .await;
    assert_eq!(start_status, StatusCode::FORBIDDEN);

    let (down_status, _down_body) = post_with_headers(
        app.clone(),
        "/api/agents/codex-worker/down",
        serde_json::json!({}),
        remote_bearer("operator-secret"),
    )
    .await;
    assert_eq!(down_status, StatusCode::FORBIDDEN);

    let (rebind_status, _rebind_body) = post_with_headers(
        app,
        "/api/agents/codex-worker/rebind",
        serde_json::json!({}),
        remote_bearer("operator-secret"),
    )
    .await;
    assert_eq!(rebind_status, StatusCode::FORBIDDEN);
    assert!(
        backend.spawned().is_empty(),
        "remote rejected lifecycle actions must not spawn"
    );
}

#[tokio::test]
async fn daemon_router_agent_owned_routes_require_agent_token_in_hard_mode() {
    let mut auth = auth_config();
    auth.api_token = None;
    auth.agent_tokens
        .insert("codex-worker".to_string(), "agent-secret".to_string());
    let (app, _dir, _backend) = empty_auth_router(auth).await;

    let register_body = serde_json::json!({
        "name": "codex-worker",
        "runtime": "codex",
        "workdir": "/tmp/codex-worker"
    });
    let (blocked_register, _blocked_register_body) =
        post(app.clone(), "/api/agents", register_body.clone()).await;
    assert_eq!(blocked_register, StatusCode::FORBIDDEN);
    let (ok_register, ok_register_body) = post_with_headers(
        app.clone(),
        "/api/agents",
        register_body,
        agent_token("agent-secret"),
    )
    .await;
    assert_eq!(ok_register, StatusCode::OK, "body: {ok_register_body}");

    for (uri, body) in [
        (
            "/api/agents/codex-worker/heartbeat",
            serde_json::json!({"server":"local"}),
        ),
        (
            "/api/agents/codex-worker/runtime",
            serde_json::json!({"blocked": true}),
        ),
        (
            "/api/agents/codex-worker/offline",
            serde_json::json!({"reason":"manual-offline"}),
        ),
    ] {
        let (blocked, _blocked_body) = post(app.clone(), uri, body.clone()).await;
        assert_eq!(blocked, StatusCode::FORBIDDEN, "blocked uri {uri}");
        let (ok, ok_body) =
            post_with_headers(app.clone(), uri, body, agent_token("agent-secret")).await;
        assert_eq!(ok, StatusCode::OK, "ok uri {uri}: {ok_body}");
    }
}

#[tokio::test]
async fn daemon_router_agent_token_audit_mode_allows_missing_tokens() {
    let mut auth = auth_config();
    auth.api_token = None;
    auth.agent_token_mode = AgentTokenMode::Audit;
    auth.agent_tokens
        .insert("codex-worker".to_string(), "agent-secret".to_string());
    let (app, _dir, _backend) = empty_auth_router(auth).await;

    let (status, body) = post(
        app,
        "/api/agents/codex-worker/heartbeat",
        serde_json::json!({"server":"local"}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["agent"]["status"], "online");
}

#[tokio::test]
async fn daemon_router_agent_registry_round_trips_register_list_inspect() {
    let (app, _dir) = empty_router().await;
    let (status, body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "codex-sec",
            "role": "reviewer",
            "capability": "strong",
            "runtime": "codex",
            "model": "gpt-5",
            "native_runtime_ref": "native://rs_codex_sec/ra_active",
            "home_dir": "/tmp/agentd/homes/agents/agent_codex_sec",
            "workdir": "/tmp/agentd/homes/agents/agent_codex_sec/workdir",
            "state_dir": "/tmp/agentd/homes/agents/agent_codex_sec/state",
            "server": "local",
            "runtime_profile": {
                "primary": {
                    "framework": "codex",
                    "provider": "openai",
                    "model": "gpt-5"
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let registered: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(registered["ok"], true);
    assert_eq!(registered["agent"]["name"], "codex-sec");
    assert_eq!(registered["agent"]["status"], "online");

    let (list_status, list_body) = get(app.clone(), "/api/agents").await;
    assert_eq!(list_status, StatusCode::OK, "body: {list_body}");
    let listed: serde_json::Value = serde_json::from_str(&list_body).expect("json");
    let arr = listed.as_array().expect("agent list array");
    assert_eq!(arr.len(), 1, "body: {list_body}");
    assert_eq!(arr[0]["name"], "codex-sec");

    let (detail_status, detail_body) = get(app, "/api/agents/codex-sec").await;
    assert_eq!(detail_status, StatusCode::OK, "body: {detail_body}");
    let detail: serde_json::Value = serde_json::from_str(&detail_body).expect("json");
    assert_eq!(detail["runtime"], "codex");
    assert_eq!(detail["model"], "gpt-5");
    assert_eq!(
        detail["workdir"],
        "/tmp/agentd/homes/agents/agent_codex_sec/workdir"
    );
    assert_eq!(detail["runtime_profile"]["primary"]["framework"], "codex");
}

#[tokio::test]
async fn daemon_router_agent_identity_patch_persists_after_router_rebuild() {
    let (app, dir) = empty_router().await;
    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "codex-worker",
            "runtime": "codex",
            "model": "gpt-5",
            "runtime_profile": {
                "primary": {
                    "framework": "codex",
                    "model": "gpt-5"
                }
            }
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (patch_status, patch_body) = patch(
        app,
        "/api/agents/codex-worker",
        serde_json::json!({
            "identity": "Be concise and report blockers"
        }),
    )
    .await;
    assert_eq!(patch_status, StatusCode::OK, "body: {patch_body}");
    let patched: serde_json::Value = serde_json::from_str(&patch_body).expect("patch json");
    assert_eq!(patched["ok"], true);
    assert_eq!(
        patched["agent"]["runtime_profile"]["identity"],
        "Be concise and report blockers"
    );
    assert_eq!(
        patched["agent"]["runtime_profile"]["primary"]["framework"],
        "codex"
    );

    let (rebuilt, _backend) =
        router_for_existing_dir_with_lifecycle(&dir, RecordingLifecycle::default()).await;
    let (detail_status, detail_body) = get(rebuilt, "/api/agents/codex-worker").await;
    assert_eq!(detail_status, StatusCode::OK, "body: {detail_body}");
    let detail: serde_json::Value = serde_json::from_str(&detail_body).expect("detail json");
    assert_eq!(
        detail["runtime_profile"]["identity"],
        "Be concise and report blockers"
    );
    assert_eq!(detail["runtime_profile"]["primary"]["framework"], "codex");
    assert_eq!(detail["runtime_profile"]["primary"]["model"], "gpt-5");
}

#[tokio::test]
async fn daemon_router_agent_heartbeat_creates_and_offline_clears_native_runtime_ref() {
    let (app, _dir) = empty_router().await;
    let (heartbeat_status, heartbeat_body) = post(
        app.clone(),
        "/api/agents/codex-worker/heartbeat",
        serde_json::json!({
            "server": "local",
            "native_runtime_ref": "native://rs_codex_worker/ra_active",
            "workspace_path": "/tmp/agentd/homes/agents/agent_codex_worker/workdir"
        }),
    )
    .await;
    assert_eq!(heartbeat_status, StatusCode::OK, "body: {heartbeat_body}");
    let heartbeat: serde_json::Value = serde_json::from_str(&heartbeat_body).expect("json");
    assert_eq!(heartbeat["created"], true);
    assert_eq!(heartbeat["agent"]["status"], "online");
    assert_eq!(
        heartbeat["agent"]["native_runtime_ref"],
        "native://rs_codex_worker/ra_active"
    );

    let (offline_status, offline_body) = post(
        app,
        "/api/agents/codex-worker/offline",
        serde_json::json!({ "reason": "manual-offline" }),
    )
    .await;
    assert_eq!(offline_status, StatusCode::OK, "body: {offline_body}");
    let offline: serde_json::Value = serde_json::from_str(&offline_body).expect("json");
    assert_eq!(offline["agent"]["status"], "offline");
    assert_eq!(offline["agent"]["offline_reason"], "manual-offline");
    assert_eq!(
        offline["agent"]["native_runtime_ref"],
        serde_json::Value::Null
    );
}

#[tokio::test]
async fn daemon_router_agent_unknown_inspect_and_offline_return_404() {
    let (app, _dir) = empty_router().await;
    let (inspect_status, _inspect_body) = get(app.clone(), "/api/agents/ghost").await;
    assert_eq!(inspect_status, StatusCode::NOT_FOUND);

    let (offline_status, _offline_body) = post(
        app,
        "/api/agents/ghost/offline",
        serde_json::json!({ "reason": "manual-offline" }),
    )
    .await;
    assert_eq!(offline_status, StatusCode::NOT_FOUND);
}

/// Build the daemon router over a production host with an empty store (no run yet).
async fn empty_router() -> (Router, tempfile::TempDir) {
    let backend = Arc::new(FakeBackend::new());
    let (app, dir, _backend) = empty_router_with_backend(backend).await;
    (app, dir)
}

async fn empty_router_with_backend(
    backend: Arc<FakeBackend>,
) -> (Router, tempfile::TempDir, Arc<FakeBackend>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let host = ProductionRunHost::new(
        store,
        Box::new(SharedBackend(Arc::clone(&backend))),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    (daemon::build_router(Arc::new(host)), dir, backend)
}

async fn empty_router_with_lifecycle(
    lifecycle: RecordingLifecycle,
) -> (Router, tempfile::TempDir, Arc<FakeBackend>) {
    let backend = Arc::new(FakeBackend::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let host = ProductionRunHost::new(
        store,
        Box::new(SharedBackend(Arc::clone(&backend))),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    )
    .with_agent_lifecycle(Box::new(lifecycle));
    (daemon::build_router(Arc::new(host)), dir, backend)
}

async fn router_for_existing_dir_with_lifecycle(
    dir: &tempfile::TempDir,
    lifecycle: RecordingLifecycle,
) -> (Router, Arc<FakeBackend>) {
    let backend = Arc::new(FakeBackend::new());
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect existing");
    let host = ProductionRunHost::new(
        store,
        Box::new(SharedBackend(Arc::clone(&backend))),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    )
    .with_agent_lifecycle(Box::new(lifecycle));
    (daemon::build_router(Arc::new(host)), backend)
}

async fn empty_router_with_media() -> (Router, tempfile::TempDir) {
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
    (
        daemon::build_router_with_media_dir(Arc::new(host), dir.path().join("media")),
        dir,
    )
}

async fn router_for_existing_dir(dir: &tempfile::TempDir) -> Router {
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect existing");
    let host = ProductionRunHost::new(
        store,
        Box::new(FakeBackend::new()),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    daemon::build_router(Arc::new(host))
}

async fn router_for_existing_dir_with_media(dir: &tempfile::TempDir) -> Router {
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect existing");
    let host = ProductionRunHost::new(
        store,
        Box::new(FakeBackend::new()),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    daemon::build_router_with_media_dir(Arc::new(host), dir.path().join("media"))
}

#[tokio::test]
async fn daemon_router_agent_launch_env_returns_runtime_profile() {
    let (app, _dir) = empty_router().await;
    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "codex-sec",
            "runtime": "codex",
            "workdir": "/tmp/agentd/codex-sec",
            "runtime_profile": {
                "primary": {
                    "framework": "codex",
                    "provider": "openai",
                    "model": "gpt-5"
                }
            }
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (status, body) = get(app, "/api/agents/codex-sec/launch-env").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["runtimeProfile"]["primary"]["framework"], "codex");
    assert_eq!(v["runtimeProfile"]["primary"]["model"], "gpt-5");
}

#[tokio::test]
async fn daemon_router_agent_start_spawns_codex_and_marks_online() {
    let backend = Arc::new(FakeBackend::new());
    let (app, dir, backend) = empty_router_with_backend(backend).await;
    let workdir = dir.path().join("codex-worker");
    std::fs::create_dir_all(&workdir).expect("workdir");
    let workdir_str = workdir.to_string_lossy().to_string();

    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "codex-worker",
            "runtime": "codex",
            "model": "gpt-5",
            "workdir": workdir_str,
            "runtime_profile": {
                "primary": {
                    "framework": "codex",
                    "model": "gpt-5",
                    "extraArgs": "--sandbox danger-full-access"
                }
            }
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (status, body) = post(app, "/api/agents/codex-worker/start", serde_json::json!({})).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(v["ok"], true);
    assert_eq!(v["agent"]["status"], "online");
    assert_eq!(v["agent"]["native_runtime_ref"], "fake://codex-worker");
    assert_eq!(v["handle"]["address"], "fake://codex-worker");

    let spawned = backend.spawned();
    assert_eq!(spawned.len(), 1, "one backend spawn");
    assert_eq!(spawned[0].agent_id.as_str(), "codex-worker");
    assert_eq!(spawned[0].cli, CliKind::Codex);
    assert_eq!(spawned[0].worktree, workdir);
    assert_eq!(
        spawned[0].env_overrides.get("AGENTCHAT_LAUNCH_MODEL"),
        Some(&"gpt-5".to_string())
    );
}

#[tokio::test]
async fn daemon_router_agent_start_rejects_unknown_online_missing_workdir_and_bad_runtime() {
    let backend = Arc::new(FakeBackend::new());
    let (app, dir, backend) = empty_router_with_backend(backend).await;

    let (unknown_status, _unknown_body) = post(
        app.clone(),
        "/api/agents/ghost/start",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(unknown_status, StatusCode::NOT_FOUND);

    let workdir = dir.path().join("online-worker");
    std::fs::create_dir_all(&workdir).expect("online workdir");
    let (online_register_status, online_register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "online-worker",
            "runtime": "codex",
            "workdir": workdir.to_string_lossy(),
            "native_runtime_ref": "native://rs_online_worker/ra_active"
        }),
    )
    .await;
    assert_eq!(
        online_register_status,
        StatusCode::OK,
        "body: {online_register_body}"
    );
    let (online_status, _online_body) = post(
        app.clone(),
        "/api/agents/online-worker/start",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(online_status, StatusCode::CONFLICT);

    let (missing_workdir_register_status, missing_workdir_register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "missing-workdir",
            "runtime": "codex"
        }),
    )
    .await;
    assert_eq!(
        missing_workdir_register_status,
        StatusCode::OK,
        "body: {missing_workdir_register_body}"
    );
    let (missing_workdir_status, _missing_workdir_body) = post(
        app.clone(),
        "/api/agents/missing-workdir/start",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(missing_workdir_status, StatusCode::BAD_REQUEST);

    let bad_runtime_workdir = dir.path().join("bad-runtime");
    std::fs::create_dir_all(&bad_runtime_workdir).expect("bad runtime workdir");
    let (bad_runtime_register_status, bad_runtime_register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "bad-runtime",
            "runtime": "gemini",
            "workdir": bad_runtime_workdir.to_string_lossy()
        }),
    )
    .await;
    assert_eq!(
        bad_runtime_register_status,
        StatusCode::OK,
        "body: {bad_runtime_register_body}"
    );
    let (bad_runtime_status, _bad_runtime_body) =
        post(app, "/api/agents/bad-runtime/start", serde_json::json!({})).await;
    assert_eq!(bad_runtime_status, StatusCode::BAD_REQUEST);

    assert!(
        backend.spawned().is_empty(),
        "rejected starts must not spawn"
    );
}

#[tokio::test]
async fn daemon_router_remote_server_heartbeat_marks_agents_online_and_missing_offline() {
    let (app, _dir) = empty_router().await;

    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "codex-old",
            "runtime": "codex",
            "server": "remote-host-1",
            "native_runtime_ref": "codex-old:runtime-ref"
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (heartbeat_status, heartbeat_body) = post(
        app.clone(),
        "/api/servers/heartbeat",
        serde_json::json!({
            "server": "remote-host-1",
            "instanceId": "inst-1",
            "bootTs": 1_780_100_100_i64,
            "agents": ["codex-new"],
            "sessions": ["codex-new:0.0"]
        }),
    )
    .await;
    assert_eq!(heartbeat_status, StatusCode::OK, "body: {heartbeat_body}");
    let heartbeat: serde_json::Value = serde_json::from_str(&heartbeat_body).expect("json");
    assert_eq!(heartbeat["ok"], true);
    assert_eq!(heartbeat["server"]["id"], "remote-host-1");
    assert_eq!(heartbeat["server"]["agent_count"], 1);

    let (new_status, new_body) = get(app.clone(), "/api/agents/codex-new").await;
    assert_eq!(new_status, StatusCode::OK, "body: {new_body}");
    let new_agent: serde_json::Value = serde_json::from_str(&new_body).expect("new agent json");
    assert_eq!(new_agent["status"], "online");
    assert_eq!(new_agent["server"], "remote-host-1");
    assert_eq!(new_agent["native_runtime_ref"], "codex-new:0.0");

    let (old_status, old_body) = get(app, "/api/agents/codex-old").await;
    assert_eq!(old_status, StatusCode::OK, "body: {old_body}");
    let old_agent: serde_json::Value = serde_json::from_str(&old_body).expect("old agent json");
    assert_eq!(old_agent["status"], "offline");
    assert_eq!(
        old_agent["offline_reason"],
        "heartbeat-missing:remote-host-1"
    );
    assert_eq!(old_agent["native_runtime_ref"], serde_json::Value::Null);
}

#[tokio::test]
async fn daemon_router_remote_server_heartbeat_rejects_missing_server() {
    let (app, _dir) = empty_router().await;

    for body in [
        serde_json::json!({}),
        serde_json::json!({"server": ""}),
        serde_json::json!({"server": "   "}),
    ] {
        let (status, response) = post(app.clone(), "/api/servers/heartbeat", body).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "body: {response}");
        let value: serde_json::Value = serde_json::from_str(&response).expect("json");
        assert_eq!(value["error"], "server required");
    }
}

#[tokio::test]
async fn daemon_router_delivery_event_records_and_lists_agent_events() {
    let (app, _dir) = empty_router().await;
    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "codex-worker",
            "runtime": "codex"
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (post_status, post_body) = post(
        app.clone(),
        "/api/delivery-events",
        serde_json::json!({
            "type": "relay.delivered",
            "messageId": "msg-remote-1",
            "agent": "codex-worker",
            "target": "codex-worker:0.0",
            "source": "push-relay",
            "context": {
                "server": "remote-host-1",
                "mode": "remote"
            }
        }),
    )
    .await;
    assert_eq!(post_status, StatusCode::CREATED, "body: {post_body}");
    let posted: serde_json::Value = serde_json::from_str(&post_body).expect("json");
    assert_eq!(posted["ok"], true);
    assert!(
        posted["event"]["id"].as_str().is_some(),
        "body: {post_body}"
    );

    let (list_status, list_body) =
        get(app, "/api/agents/codex-worker/delivery-events?limit=5").await;
    assert_eq!(list_status, StatusCode::OK, "body: {list_body}");
    let listed: serde_json::Value = serde_json::from_str(&list_body).expect("json");
    assert_eq!(listed["events"].as_array().expect("events").len(), 1);
    assert_eq!(listed["events"][0]["type"], "relay.delivered");
    assert_eq!(listed["events"][0]["messageId"], "msg-remote-1");
    assert_eq!(listed["events"][0]["context"]["server"], "remote-host-1");
}

#[tokio::test]
async fn daemon_router_remote_relay_endpoints_reject_remote_operator_without_bearer() {
    let mut tokens = std::collections::BTreeMap::new();
    tokens.insert("codex-worker".to_string(), "agent-secret".to_string());
    let (app, _dir, _backend) = empty_auth_router(AuthConfig {
        api_token: Some("operator-secret".to_string()),
        agent_token_mode: AgentTokenMode::Hard,
        agent_tokens: tokens,
    })
    .await;

    let (heartbeat_status, heartbeat_body) = post_with_headers(
        app.clone(),
        "/api/servers/heartbeat",
        serde_json::json!({
            "server": "remote-host-1",
            "agents": ["codex-worker"],
            "sessions": ["codex-worker:0.0"]
        }),
        remote_bearer("wrong-secret"),
    )
    .await;
    assert_eq!(
        heartbeat_status,
        StatusCode::UNAUTHORIZED,
        "body: {heartbeat_body}"
    );

    let (event_status, event_body) = post_with_headers(
        app.clone(),
        "/api/delivery-events",
        serde_json::json!({
            "type": "relay.delivered",
            "agent": "codex-worker"
        }),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(event_status, StatusCode::UNAUTHORIZED, "body: {event_body}");

    let (missing_status, missing_body) =
        get(app.clone(), "/api/agents/codex-worker/delivery-events").await;
    assert_eq!(
        missing_status,
        StatusCode::FORBIDDEN,
        "body: {missing_body}"
    );

    let (wrong_status, wrong_body) = get_with_headers(
        app,
        "/api/agents/codex-worker/delivery-events",
        agent_token("wrong-agent-secret"),
    )
    .await;
    assert_eq!(wrong_status, StatusCode::FORBIDDEN, "body: {wrong_body}");
}

#[tokio::test]
async fn daemon_router_matrix_room_registration_persists_mapping_and_group() {
    let (app, _dir) = empty_router().await;

    let (room_status, room_body) = post(
        app.clone(),
        "/api/matrix/rooms",
        serde_json::json!({
            "roomId": "!ops:matrix.test",
            "group": "ops",
            "trusted": true,
            "trustReason": "managed",
            "inviterMxid": "@alice:matrix.test",
            "members": ["codex-worker", "alice"]
        }),
    )
    .await;
    assert_eq!(room_status, StatusCode::OK, "body: {room_body}");
    let room: serde_json::Value = serde_json::from_str(&room_body).expect("room json");
    assert_eq!(room["ok"], true);
    assert_eq!(room["room"]["roomId"], "!ops:matrix.test");
    assert_eq!(room["room"]["group"], "ops");
    assert_eq!(room["room"]["trusted"], true);
    assert_eq!(room["room"]["trustReason"], "managed");

    let (get_status, get_body) = get(
        app.clone(),
        &format!("/api/matrix/rooms/{}", url_encode("!ops:matrix.test")),
    )
    .await;
    assert_eq!(get_status, StatusCode::OK, "body: {get_body}");
    let got: serde_json::Value = serde_json::from_str(&get_body).expect("get json");
    assert_eq!(got["room"]["roomId"], "!ops:matrix.test");
    assert_eq!(got["room"]["inviterMxid"], "@alice:matrix.test");

    let (group_status, group_body) = get(app, "/api/groups/ops").await;
    assert_eq!(group_status, StatusCode::OK, "body: {group_body}");
    let group: serde_json::Value = serde_json::from_str(&group_body).expect("group json");
    assert_eq!(group["name"], "ops");
    assert_eq!(
        group["members"],
        serde_json::json!(["codex-worker", "alice"])
    );
}

#[tokio::test]
async fn daemon_router_matrix_inbound_agent_dm_persists_source_metadata_and_dedupes_event() {
    let (app, _dir) = empty_router().await;
    let (agent_status, agent_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "codex-worker",
            "runtime": "codex"
        }),
    )
    .await;
    assert_eq!(agent_status, StatusCode::OK, "body: {agent_body}");

    let (room_status, room_body) = post(
        app.clone(),
        "/api/matrix/rooms",
        serde_json::json!({
            "roomId": "!dm:matrix.test",
            "agent": "codex-worker",
            "trusted": true,
            "trustReason": "managed"
        }),
    )
    .await;
    assert_eq!(room_status, StatusCode::OK, "body: {room_body}");

    let inbound = serde_json::json!({
        "eventId": "$dm-1",
        "roomId": "!dm:matrix.test",
        "senderMxid": "@alice:matrix.test",
        "body": "please review the patch",
        "trustLevel": "external"
    });
    let (first_status, first_body) =
        post(app.clone(), "/api/matrix/inbound", inbound.clone()).await;
    assert_eq!(first_status, StatusCode::CREATED, "body: {first_body}");
    let first: serde_json::Value = serde_json::from_str(&first_body).expect("first json");
    assert_eq!(first["ok"], true);
    assert_eq!(first["duplicate"], false);
    let message_id = first["message"]["id"].as_str().expect("message id");

    let (second_status, second_body) = post(app.clone(), "/api/matrix/inbound", inbound).await;
    assert_eq!(second_status, StatusCode::OK, "body: {second_body}");
    let second: serde_json::Value = serde_json::from_str(&second_body).expect("second json");
    assert_eq!(second["duplicate"], true);
    assert_eq!(second["messageId"], message_id);

    let (inbox_status, inbox_body) = get(app, "/api/inbox/codex-worker").await;
    assert_eq!(inbox_status, StatusCode::OK, "body: {inbox_body}");
    let inbox: serde_json::Value = serde_json::from_str(&inbox_body).expect("inbox json");
    let dm = inbox["dm"].as_array().expect("dm array");
    assert_eq!(dm.len(), 1, "body: {inbox_body}");
    assert_eq!(dm[0]["id"], message_id);
    assert_eq!(dm[0]["from"], "alice");
    assert_eq!(dm[0]["to"], "codex-worker");
    assert_eq!(dm[0]["source"], "matrix");
    assert_eq!(dm[0]["sourceRoom"], "!dm:matrix.test");
    assert_eq!(dm[0]["senderMxid"], "@alice:matrix.test");
    assert_eq!(dm[0]["trustLevel"], "external");
}

#[tokio::test]
async fn daemon_router_matrix_inbound_rejects_untrusted_room() {
    let (app, _dir) = empty_router().await;

    let (status, body) = post(
        app,
        "/api/matrix/inbound",
        serde_json::json!({
            "eventId": "$evil-1",
            "roomId": "!evil:matrix.test",
            "senderMxid": "@mallory:matrix.test",
            "body": "route this"
        }),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
    let value: serde_json::Value = serde_json::from_str(&body).expect("json");
    assert_eq!(value["error"], "matrix room not trusted");
}

#[tokio::test]
async fn daemon_router_matrix_inbound_agentignore_records_ignored_event_without_message() {
    let (app, _dir) = empty_router().await;
    let (room_status, room_body) = post(
        app.clone(),
        "/api/matrix/rooms",
        serde_json::json!({
            "roomId": "!dm-ignore:matrix.test",
            "agent": "codex-worker",
            "trusted": true,
            "trustReason": "managed"
        }),
    )
    .await;
    assert_eq!(room_status, StatusCode::OK, "body: {room_body}");

    let (ignored_status, ignored_body) = post(
        app.clone(),
        "/api/matrix/inbound",
        serde_json::json!({
            "eventId": "$ignore-1",
            "roomId": "!dm-ignore:matrix.test",
            "senderMxid": "@alice:matrix.test",
            "body": "[AGENTIGNORE] private note"
        }),
    )
    .await;
    assert_eq!(ignored_status, StatusCode::OK, "body: {ignored_body}");
    let ignored: serde_json::Value = serde_json::from_str(&ignored_body).expect("ignored json");
    assert_eq!(ignored["ignored"], true);
    assert_eq!(ignored["message"], serde_json::Value::Null);

    let (inbox_status, inbox_body) = get(app, "/api/inbox/codex-worker").await;
    assert_eq!(inbox_status, StatusCode::OK, "body: {inbox_body}");
    let inbox: serde_json::Value = serde_json::from_str(&inbox_body).expect("inbox json");
    assert!(inbox["dm"].as_array().expect("dm array").is_empty());
}

#[tokio::test]
async fn daemon_router_matrix_outbox_filters_matrix_echo_events() {
    let (app, _dir) = empty_router().await;
    let (room_status, room_body) = post(
        app.clone(),
        "/api/matrix/rooms",
        serde_json::json!({
            "roomId": "!dm-outbox:matrix.test",
            "agent": "codex-worker",
            "trusted": true,
            "trustReason": "managed"
        }),
    )
    .await;
    assert_eq!(room_status, StatusCode::OK, "body: {room_body}");

    let (matrix_status, matrix_body) = post(
        app.clone(),
        "/api/matrix/inbound",
        serde_json::json!({
            "eventId": "$matrix-echo",
            "roomId": "!dm-outbox:matrix.test",
            "senderMxid": "@alice:matrix.test",
            "body": "matrix-originated"
        }),
    )
    .await;
    assert_eq!(matrix_status, StatusCode::CREATED, "body: {matrix_body}");
    let matrix_message: serde_json::Value =
        serde_json::from_str(&matrix_body).expect("matrix body json");
    let matrix_id = matrix_message["message"]["id"].as_str().expect("matrix id");

    let (api_status, api_body) = post(
        app.clone(),
        "/api/messages",
        serde_json::json!({
            "message_id": "msg-api-outbox",
            "from": "codex-worker",
            "to": "alice",
            "summary": "api-originated",
            "full": "api-originated"
        }),
    )
    .await;
    assert_eq!(api_status, StatusCode::CREATED, "body: {api_body}");

    let (outbox_status, outbox_body) = get(app, "/api/matrix/outbox?from_seq=0").await;
    assert_eq!(outbox_status, StatusCode::OK, "body: {outbox_body}");
    let outbox: serde_json::Value = serde_json::from_str(&outbox_body).expect("outbox json");
    let events = outbox["events"].as_array().expect("events array");
    assert_eq!(events.len(), 1, "body: {outbox_body}");
    assert_eq!(events[0]["payload"]["messageId"], "msg-api-outbox");
    assert_eq!(events[0]["payload"]["source"], "api");
    assert_ne!(events[0]["payload"]["messageId"], matrix_id);
}

#[tokio::test]
async fn daemon_router_matrix_bridge_endpoints_reject_remote_operator_without_bearer() {
    let (app, _dir, _backend) = empty_auth_router(auth_config()).await;

    let (room_status, room_body) = post(
        app.clone(),
        "/api/matrix/rooms",
        serde_json::json!({
            "roomId": "!ops:matrix.test",
            "group": "ops",
            "trusted": true
        }),
    )
    .await;
    assert_eq!(room_status, StatusCode::UNAUTHORIZED, "body: {room_body}");

    let (inbound_status, inbound_body) = post_with_headers(
        app.clone(),
        "/api/matrix/inbound",
        serde_json::json!({
            "eventId": "$dm-auth",
            "roomId": "!ops:matrix.test",
            "senderMxid": "@alice:matrix.test",
            "body": "auth check"
        }),
        HeaderMap::new(),
    )
    .await;
    assert_eq!(
        inbound_status,
        StatusCode::UNAUTHORIZED,
        "body: {inbound_body}"
    );

    let (outbox_status, outbox_body) = get(app, "/api/matrix/outbox?from_seq=0").await;
    assert_eq!(
        outbox_status,
        StatusCode::UNAUTHORIZED,
        "body: {outbox_body}"
    );
}

#[tokio::test]
async fn daemon_router_agent_runtime_update_records_observation() {
    let (app, _dir) = empty_router().await;
    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "codex-worker",
            "runtime": "codex",
            "workdir": "/tmp/agentd/codex-worker"
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (runtime_status, runtime_body) = post(
        app.clone(),
        "/api/agents/codex-worker/runtime",
        serde_json::json!({
            "blocked": true,
            "reason": "waiting for reviewer",
            "activeNow": false,
            "idleDurationSec": 12,
            "lastTmuxActivitySec": 12,
            "workspacePath": "/tmp/agentd/codex-worker",
            "mcpPresent": true
        }),
    )
    .await;
    assert_eq!(runtime_status, StatusCode::OK, "body: {runtime_body}");
    let runtime: serde_json::Value = serde_json::from_str(&runtime_body).expect("json");
    assert_eq!(runtime["ok"], true);
    assert_eq!(runtime["runtime"]["agent"], "codex-worker");
    assert_eq!(runtime["runtime"]["blocked"], true);
    assert_eq!(runtime["runtime"]["blockedReason"], "waiting for reviewer");
    assert_eq!(runtime["runtime"]["mcpPresent"], true);

    let (detail_status, detail_body) = get(app, "/api/agents/codex-worker").await;
    assert_eq!(detail_status, StatusCode::OK, "body: {detail_body}");
    let detail: serde_json::Value = serde_json::from_str(&detail_body).expect("json");
    assert_eq!(detail["runtime_state"]["blocked"], true);
    assert_eq!(detail["runtime_state"]["mcpPresent"], true);
}

#[tokio::test]
async fn daemon_router_agent_down_stops_runtime_and_marks_offline() {
    let lifecycle = RecordingLifecycle::default();
    let (app, dir, _backend) = empty_router_with_lifecycle(lifecycle.clone()).await;
    let state_dir = dir.path().join("codex-worker-state");
    std::fs::create_dir_all(&state_dir).expect("state dir");

    let (register_status, register_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({
            "name": "codex-worker",
            "runtime": "codex",
            "native_runtime_ref": "native://rs_codex_worker/ra_active",
            "state_dir": state_dir.to_string_lossy()
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (down_status, down_body) = post(
        app.clone(),
        "/api/agents/codex-worker/down",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(down_status, StatusCode::OK, "body: {down_body}");
    let down: serde_json::Value = serde_json::from_str(&down_body).expect("json");
    assert_eq!(down["ok"], true);
    assert_eq!(down["action"], "agent-down-kill");
    assert_eq!(down["agent"]["status"], "offline");
    assert_eq!(down["agent"]["native_runtime_ref"], serde_json::Value::Null);
    assert_eq!(down["agent"]["offline_reason"], "agent-down");
    assert_eq!(down["agent"]["runtime_state"]["lifecycle"]["state"], "down");
    assert_eq!(
        down["agent"]["runtime_state"]["lifecycle"]["finalCaptureSha"],
        "fake-sha"
    );

    let shutdowns = lifecycle.shutdowns();
    assert_eq!(shutdowns.len(), 1, "one shutdown call");
    assert_eq!(shutdowns[0].0, "native://rs_codex_worker/ra_active");
    assert!(
        shutdowns[0].1.to_string_lossy().contains("codex-worker"),
        "archive path should identify the agent: {:?}",
        shutdowns[0].1
    );

    let (detail_status, detail_body) = get(app, "/api/agents/codex-worker").await;
    assert_eq!(detail_status, StatusCode::OK, "body: {detail_body}");
    let detail: serde_json::Value = serde_json::from_str(&detail_body).expect("json");
    assert_eq!(detail["status"], "offline");
    assert_eq!(detail["native_runtime_ref"], serde_json::Value::Null);
    assert_eq!(detail["runtime_state"]["lifecycle"]["state"], "down");
}

#[tokio::test]
async fn daemon_router_agent_rebind_recovers_live_session_and_marks_missing_offline() {
    let lifecycle = RecordingLifecycle::default();
    lifecycle.set_rebind_result(
        "native://rs_codex_live/ra_active",
        Some(lifecycle_handle(
            "codex-live",
            "native://rs_codex_live/ra_active",
        )),
    );
    lifecycle.set_rebind_result("native://rs_codex_gone/ra_active", None);
    let (app, _dir, _backend) = empty_router_with_lifecycle(lifecycle.clone()).await;

    for (name, target) in [
        ("codex-live", "native://rs_codex_live/ra_active"),
        ("codex-gone", "native://rs_codex_gone/ra_active"),
    ] {
        let (status, body) = post(
            app.clone(),
            "/api/agents",
            serde_json::json!({
                "name": name,
                "runtime": "codex",
                "native_runtime_ref": target
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
    }

    let (live_status, live_body) = post(
        app.clone(),
        "/api/agents/codex-live/rebind",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(live_status, StatusCode::OK, "body: {live_body}");
    let live: serde_json::Value = serde_json::from_str(&live_body).expect("json");
    assert_eq!(live["ok"], true);
    assert_eq!(live["rebound"], true);
    assert_eq!(live["agent"]["status"], "online");
    assert_eq!(
        live["handle"]["address"],
        "native://rs_codex_live/ra_active"
    );
    assert_eq!(
        live["agent"]["runtime_state"]["lifecycle"]["state"],
        "rebound"
    );

    let (gone_status, gone_body) = post(
        app.clone(),
        "/api/agents/codex-gone/rebind",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(gone_status, StatusCode::OK, "body: {gone_body}");
    let gone: serde_json::Value = serde_json::from_str(&gone_body).expect("json");
    assert_eq!(gone["ok"], true);
    assert_eq!(gone["rebound"], false);
    assert_eq!(gone["handle"], serde_json::Value::Null);
    assert_eq!(gone["agent"]["status"], "offline");
    assert_eq!(gone["agent"]["offline_reason"], "rebind-missing-session");
    assert_eq!(
        gone["agent"]["runtime_state"]["lifecycle"]["state"],
        "missing"
    );

    assert_eq!(
        lifecycle.rebinds(),
        vec![
            "native://rs_codex_live/ra_active".to_string(),
            "native://rs_codex_gone/ra_active".to_string()
        ]
    );
}

#[tokio::test]
async fn daemon_router_agent_rebind_recovers_after_host_rebuild() {
    let lifecycle = RecordingLifecycle::default();
    lifecycle.set_rebind_result(
        "native://rs_codex_worker/ra_active",
        Some(lifecycle_handle(
            "codex-worker",
            "native://rs_codex_worker/ra_active",
        )),
    );
    let (first_app, dir, _first_backend) = empty_router_with_lifecycle(lifecycle.clone()).await;

    let (register_status, register_body) = post(
        first_app,
        "/api/agents",
        serde_json::json!({
            "name": "codex-worker",
            "runtime": "codex",
            "native_runtime_ref": "native://rs_codex_worker/ra_active"
        }),
    )
    .await;
    assert_eq!(register_status, StatusCode::OK, "body: {register_body}");

    let (rebuilt_app, backend) =
        router_for_existing_dir_with_lifecycle(&dir, lifecycle.clone()).await;
    let (rebind_status, rebind_body) = post(
        rebuilt_app,
        "/api/agents/codex-worker/rebind",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(rebind_status, StatusCode::OK, "body: {rebind_body}");
    let rebind: serde_json::Value = serde_json::from_str(&rebind_body).expect("json");
    assert_eq!(rebind["rebound"], true);
    assert_eq!(
        rebind["handle"]["address"],
        "native://rs_codex_worker/ra_active"
    );
    assert!(
        backend.spawned().is_empty(),
        "rebind recovery must not spawn a new agent"
    );
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
#[allow(clippy::too_many_lines)]
async fn enterprise_operator_router_issues_read_only_browser_session() {
    let (app, _dir) = empty_enterprise_operator_router().await;
    let (health_status, _) = get(app.clone(), "/healthz").await;
    assert_eq!(health_status, StatusCode::OK);
    let (dashboard_status, _) = get(app.clone(), "/dashboard").await;
    assert_eq!(dashboard_status, StatusCode::OK);
    let (runs_status, _) = get(app.clone(), "/runs").await;
    assert_eq!(runs_status, StatusCode::UNAUTHORIZED);

    let missing = app
        .clone()
        .oneshot(
            Request::post("/api/operator/session")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

    let wrong = app
        .clone()
        .oneshot(
            Request::post("/api/operator/session")
                .header("authorization", "Bearer wrong-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

    let session = app
        .clone()
        .oneshot(
            Request::post("/api/operator/session")
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(session.status(), StatusCode::OK);
    assert_eq!(
        session
            .headers()
            .get("cache-control")
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    let set_cookie = session
        .headers()
        .get("set-cookie")
        .and_then(|value| value.to_str().ok())
        .expect("operator read cookie")
        .to_string();
    assert!(set_cookie.starts_with(&format!("{OPERATOR_READ_COOKIE_NAME}=")));
    assert!(set_cookie.contains("; Path=/"));
    assert!(set_cookie.contains("; Secure"));
    assert!(set_cookie.contains("; HttpOnly"));
    assert!(set_cookie.contains("; SameSite=Strict"));
    assert!(!set_cookie.contains("operator-secret"));
    assert!(!set_cookie.contains("Max-Age"));
    assert!(!set_cookie.contains("Expires="));
    assert!(!set_cookie.contains("Domain="));
    let cookie = set_cookie.split(';').next().expect("cookie pair");
    let session_body = session
        .into_body()
        .collect()
        .await
        .expect("session body")
        .to_bytes();
    assert!(
        !session_body
            .as_ref()
            .windows("operator-secret".len())
            .any(|window| { window == "operator-secret".as_bytes() })
    );

    let mut cookie_headers = HeaderMap::new();
    cookie_headers.insert("cookie", cookie.parse().expect("cookie header"));
    let (cookie_read_status, _) = get_with_headers(app.clone(), "/runs", cookie_headers).await;
    assert_eq!(cookie_read_status, StatusCode::OK);

    let cookie_write = app
        .clone()
        .oneshot(
            Request::post("/runs")
                .header("cookie", cookie)
                .header("content-type", "application/json")
                .body(Body::from(r#"{"flow":"bogus"}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(cookie_write.status(), StatusCode::UNAUTHORIZED);

    let bearer_write = app
        .oneshot(
            Request::post("/runs")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"flow":"bogus"}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_ne!(bearer_write.status(), StatusCode::UNAUTHORIZED);
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
