//! AD-E5: daemon-composed native runtime control-plane HTTP surface backed by
//! the real `SQLite` adapter.

use agentd_bin::daemon::daemon_native_runtime_router;
use agentd_core::types::{
    AgentProfileId, NodeId, RunId, RuntimeAttemptId, RuntimeSessionId, TaskRunId, WorkerId,
    WorkerIncarnationId,
};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::runtime_session_repo::{self, ExecutionSnapshotRef, RuntimeSessionCreate};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use tower::ServiceExt;

struct Fixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    session_id: RuntimeSessionId,
    task_id: TaskRunId,
    incarnation_id: WorkerIncarnationId,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let run_id = RunId::new();
    run_repo::insert_run(store.pool(), &run_id, "workflow-sha")
        .await
        .expect("run");
    let task_id = task_repo::insert_task_run(store.pool(), &run_id, &NodeId::parsed("impl"))
        .await
        .expect("task");
    let profile_id = AgentProfileId::new();
    agent_profile_repo::create_profile(
        store.pool(),
        AgentProfileCreate {
            id: profile_id.clone(),
            role: "implementer".to_string(),
            capability: Some("implementation".to_string()),
            runtime: "codex".to_string(),
            model: Some("gpt-5".to_string()),
            prompt_profile: Some("default".to_string()),
        },
    )
    .await
    .expect("profile");
    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "corp-coding".to_string(),
            labels: json!({}),
        },
    )
    .await
    .expect("worker");
    let incarnation_id = WorkerIncarnationId::new();
    worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        WorkerRegistration {
            id: incarnation_id.clone(),
            daemon_version: "0.0.0-ad-e5".to_string(),
            host_name: "host-a".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
        },
    )
    .await
    .expect("incarnation");
    let session_id = RuntimeSessionId::new();
    runtime_session_repo::create_session(
        store.pool(),
        RuntimeSessionCreate {
            id: session_id.clone(),
            execution_task_id: task_id.clone(),
            agent_profile_id: profile_id,
            snapshot: ExecutionSnapshotRef {
                authority_key: "specify".to_string(),
                resource_kind: "execution_snapshot".to_string(),
                resource_id: "spec-1".to_string(),
                resource_version: "v1".to_string(),
                content_sha256: "a".repeat(64),
            },
        },
    )
    .await
    .expect("session");
    Fixture {
        store,
        _dir: dir,
        session_id,
        task_id,
        incarnation_id,
    }
}

fn post(path: &str, token: Option<&str>, body: &serde_json::Value) -> Request<Body> {
    let mut request = Request::post(path).header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    request
        .body(Body::from(serde_json::to_vec(body).expect("json")))
        .expect("request")
}

#[tokio::test]
async fn daemon_native_runtime_routes_are_authenticated_and_durable() {
    let fixture = fixture().await;
    let app = daemon_native_runtime_router(&fixture.store, Some("worker-secret".to_string()));

    let unauthorized = app
        .clone()
        .oneshot(post(
            "/api/runtime/native/session/validate",
            None,
            &json!({
                "session_id": fixture.session_id,
                "task_id": fixture.task_id
            }),
        ))
        .await
        .expect("response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let validated = app
        .clone()
        .oneshot(post(
            "/api/runtime/native/session/validate",
            Some("worker-secret"),
            &json!({
                "session_id": fixture.session_id,
                "task_id": fixture.task_id
            }),
        ))
        .await
        .expect("response");
    assert_eq!(validated.status(), StatusCode::OK);

    let attempt_id = RuntimeAttemptId::new();
    let started = app
        .clone()
        .oneshot(post(
            "/api/runtime/native/attempt/start",
            Some("worker-secret"),
            &json!({
                "attempt_id": attempt_id,
                "session_id": fixture.session_id,
                "task_id": fixture.task_id,
                "worker_incarnation_id": fixture.incarnation_id,
                "observed_at": 1
            }),
        ))
        .await
        .expect("response");
    assert_eq!(started.status(), StatusCode::OK);
    let bytes = started
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let state: serde_json::Value = serde_json::from_slice(&bytes).expect("state json");
    assert_eq!(state["status"], "starting");

    let durable = runtime_session_repo::latest_attempt(fixture.store.pool(), &fixture.session_id)
        .await
        .expect("latest attempt query")
        .expect("attempt exists");
    assert_eq!(durable.id, attempt_id);

    let foreign_task = app
        .clone()
        .oneshot(post(
            "/api/runtime/native/session/validate",
            Some("worker-secret"),
            &json!({
                "session_id": fixture.session_id,
                "task_id": TaskRunId::new()
            }),
        ))
        .await
        .expect("response");
    assert_eq!(foreign_task.status(), StatusCode::CONFLICT);
}
