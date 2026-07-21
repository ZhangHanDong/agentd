//! AD-E5: worker-side HTTP adapter for the native runtime control-plane port,
//! exercised against the daemon-composed router over real TCP.

use std::sync::Arc;
use std::time::Duration;

use agentd_bin::daemon::daemon_native_runtime_router;
use agentd_bin::native_runtime_client::NativeRuntimeHttpClient;
use agentd_bin::native_worker::AgentdWorker;
use agentd_core::ports::{
    NativeRuntimeAttemptStart, NativeRuntimeAttemptState, NativeRuntimeControlError,
    NativeRuntimeControlPort,
};
use agentd_core::types::{
    AgentProfileId, NodeId, RunId, RuntimeAttemptId, RuntimeAttemptStatus, RuntimeSessionId,
    TaskRunId, WorkerId, WorkerIncarnationId,
};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::runtime_session_repo::{self, ExecutionSnapshotRef, RuntimeSessionCreate};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
use agentd_tmux::native::NativeProcessConfig;
use serde_json::json;

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

async fn serve_daemon(store: SqliteStore, token: &str) -> String {
    let app = daemon_native_runtime_router(&store, Some(token.to_string()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn http_adapter_round_trips_attempt_lifecycle_against_daemon() {
    let fixture = fixture().await;
    let base_url = serve_daemon(fixture.store.clone(), "worker-secret").await;
    let client = NativeRuntimeHttpClient::new(base_url, "worker-secret").expect("client");

    client
        .validate_session_task(&fixture.session_id, &fixture.task_id)
        .await
        .expect("session/task validates over HTTP");

    let attempt_id = RuntimeAttemptId::new();
    let started = client
        .start_attempt(&NativeRuntimeAttemptStart {
            attempt_id: attempt_id.clone(),
            session_id: fixture.session_id.clone(),
            task_id: fixture.task_id.clone(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            observed_at: 1,
        })
        .await
        .expect("attempt starts over HTTP");
    assert_eq!(started.attempt_id, attempt_id);
    assert_eq!(started.status, RuntimeAttemptStatus::Starting);

    client
        .update_attempt(&NativeRuntimeAttemptState {
            attempt_id: attempt_id.clone(),
            session_id: fixture.session_id.clone(),
            status: RuntimeAttemptStatus::Running,
            native_session_ref: Some("thread-abc123".to_string()),
            exit_code: None,
            observed_at: 2,
        })
        .await
        .expect("attempt updates over HTTP");

    client
        .mark_attempt_terminal(&NativeRuntimeAttemptState {
            attempt_id: attempt_id.clone(),
            session_id: fixture.session_id.clone(),
            status: RuntimeAttemptStatus::Exited,
            native_session_ref: None,
            exit_code: None,
            observed_at: 3,
        })
        .await
        .expect("attempt reconciles over HTTP");

    let durable = runtime_session_repo::latest_attempt(fixture.store.pool(), &fixture.session_id)
        .await
        .expect("latest attempt query")
        .expect("attempt exists");
    assert_eq!(durable.id, attempt_id);
    assert_eq!(durable.status, RuntimeAttemptStatus::Exited);
    assert_eq!(durable.native_session_ref.as_deref(), Some("thread-abc123"));

    let view = client
        .session_view(&fixture.session_id)
        .await
        .expect("view over HTTP")
        .expect("session known to control plane");
    assert_eq!(view.session_id, fixture.session_id);
    assert_eq!(view.task_id, fixture.task_id);
    assert_eq!(
        view.latest_native_session_ref.as_deref(),
        Some("thread-abc123")
    );

    let unknown = client
        .session_view(&RuntimeSessionId::new())
        .await
        .expect("view over HTTP");
    assert!(unknown.is_none());
}

#[tokio::test]
async fn remote_worker_recovers_without_a_local_runtime_session() {
    let fixture = fixture().await;
    let base_url = serve_daemon(fixture.store.clone(), "worker-secret").await;
    let client = NativeRuntimeHttpClient::new(base_url, "worker-secret").expect("client");

    let lost_attempt_id = RuntimeAttemptId::new();
    client
        .start_attempt(&NativeRuntimeAttemptStart {
            attempt_id: lost_attempt_id.clone(),
            session_id: fixture.session_id.clone(),
            task_id: fixture.task_id.clone(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            observed_at: 1,
        })
        .await
        .expect("initial attempt");
    client
        .update_attempt(&NativeRuntimeAttemptState {
            attempt_id: lost_attempt_id.clone(),
            session_id: fixture.session_id.clone(),
            status: RuntimeAttemptStatus::Running,
            native_session_ref: Some("thread-remote-1".to_string()),
            exit_code: None,
            observed_at: 2,
        })
        .await
        .expect("initial attempt running");
    client
        .mark_attempt_terminal(&NativeRuntimeAttemptState {
            attempt_id: lost_attempt_id,
            session_id: fixture.session_id.clone(),
            status: RuntimeAttemptStatus::Gone,
            native_session_ref: Some("thread-remote-1".to_string()),
            exit_code: None,
            observed_at: 3,
        })
        .await
        .expect("initial attempt lost");

    let worker_dir = tempfile::tempdir().expect("worker tempdir");
    let worker_store = SqliteStore::connect(&worker_dir.path().join("worker.db"))
        .await
        .expect("worker store");
    assert!(
        runtime_session_repo::get_session(worker_store.pool(), &fixture.session_id)
            .await
            .expect("local session lookup")
            .is_none(),
        "remote worker must not have a shadow runtime session"
    );

    let worker = AgentdWorker::with_runtime_control(worker_store, Arc::new(client));
    let handle = worker
        .recover_if_pending(
            fixture.session_id.clone(),
            fixture.incarnation_id,
            NativeProcessConfig {
                program: "/bin/sh".to_string(),
                args: vec!["-c".to_string(), "exit 0".to_string()],
                ..NativeProcessConfig::default()
            },
        )
        .await
        .expect("remote recovery")
        .expect("resume-pending session");
    handle
        .wait(Duration::from_secs(5))
        .await
        .expect("recovered process exits");

    let session = runtime_session_repo::get_session(fixture.store.pool(), &fixture.session_id)
        .await
        .expect("control-plane session lookup")
        .expect("control-plane session");
    assert_eq!(
        session.status,
        agentd_core::types::RuntimeSessionStatus::Completed
    );
}

#[tokio::test]
async fn http_adapter_classifies_terminal_and_transient_errors() {
    let fixture = fixture().await;
    let base_url = serve_daemon(fixture.store.clone(), "worker-secret").await;

    let bad_auth = NativeRuntimeHttpClient::new(base_url.clone(), "wrong-secret").expect("client");
    let auth_error = bad_auth
        .validate_session_task(&fixture.session_id, &fixture.task_id)
        .await
        .expect_err("wrong bearer token is a terminal error");
    assert!(matches!(auth_error, NativeRuntimeControlError::Conflict(_)));

    let client = NativeRuntimeHttpClient::new(base_url, "worker-secret").expect("client");
    let conflict = client
        .validate_session_task(&fixture.session_id, &TaskRunId::new())
        .await
        .expect_err("foreign task is a terminal conflict");
    assert!(matches!(conflict, NativeRuntimeControlError::Conflict(_)));

    let dead = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let dead_addr = dead.local_addr().expect("addr");
    drop(dead);
    let unreachable = NativeRuntimeHttpClient::new(format!("http://{dead_addr}"), "worker-secret")
        .expect("client");
    let outage = unreachable
        .validate_session_task(&fixture.session_id, &fixture.task_id)
        .await
        .expect_err("connection refusal is transient");
    assert!(matches!(outage, NativeRuntimeControlError::Unavailable(_)));
}

#[tokio::test]
async fn http_adapter_rejects_non_http_url() {
    assert!(NativeRuntimeHttpClient::new("https://daemon:1", "token").is_err());
    assert!(NativeRuntimeHttpClient::new("http://a/b", "token").is_err());
}
