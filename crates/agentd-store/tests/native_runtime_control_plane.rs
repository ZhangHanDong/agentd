//! AD-E5: `SQLite` adapter contract for the native runtime control-plane port.

use agentd_core::ports::{
    NativeRuntimeAttemptStart, NativeRuntimeAttemptState, NativeRuntimeControlError,
    NativeRuntimeControlPort,
};
use agentd_core::types::{
    AgentProfileId, NodeId, RunId, RuntimeAttemptId, RuntimeAttemptStatus, RuntimeSessionId,
    TaskRunId, WorkerId, WorkerIncarnationId,
};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::native_runtime_control_plane::SqliteNativeRuntimeControlPlane;
use agentd_store::runtime_session_repo::{self, ExecutionSnapshotRef, RuntimeSessionCreate};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
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

fn start_request(fixture: &Fixture, attempt_id: RuntimeAttemptId) -> NativeRuntimeAttemptStart {
    NativeRuntimeAttemptStart {
        attempt_id,
        session_id: fixture.session_id.clone(),
        task_id: fixture.task_id.clone(),
        worker_incarnation_id: fixture.incarnation_id.clone(),
        observed_at: 1,
    }
}

#[tokio::test]
async fn sqlite_adapter_validates_session_task_ownership() {
    let fixture = fixture().await;
    let plane = SqliteNativeRuntimeControlPlane::new(fixture.store.pool().clone());

    plane
        .validate_session_task(&fixture.session_id, &fixture.task_id)
        .await
        .expect("matching session/task validates");

    let mismatch = plane
        .validate_session_task(&fixture.session_id, &TaskRunId::new())
        .await
        .expect_err("foreign task must be rejected");
    assert!(matches!(mismatch, NativeRuntimeControlError::Conflict(_)));

    let missing = plane
        .validate_session_task(&RuntimeSessionId::new(), &fixture.task_id)
        .await
        .expect_err("unknown session must be rejected");
    assert!(matches!(missing, NativeRuntimeControlError::Conflict(_)));
}

#[tokio::test]
async fn sqlite_adapter_starts_updates_and_terminates_attempt() {
    let fixture = fixture().await;
    let plane = SqliteNativeRuntimeControlPlane::new(fixture.store.pool().clone());
    let attempt_id = RuntimeAttemptId::new();

    let started = plane
        .start_attempt(&start_request(&fixture, attempt_id.clone()))
        .await
        .expect("attempt starts");
    assert_eq!(started.attempt_id, attempt_id);
    assert_eq!(started.session_id, fixture.session_id);
    assert_eq!(started.status, RuntimeAttemptStatus::Starting);

    plane
        .update_attempt(&NativeRuntimeAttemptState {
            attempt_id: attempt_id.clone(),
            session_id: fixture.session_id.clone(),
            status: RuntimeAttemptStatus::Running,
            native_session_ref: Some("thread-abc123".to_string()),
            observed_at: 2,
        })
        .await
        .expect("attempt updates");
    let attempt = runtime_session_repo::latest_attempt(fixture.store.pool(), &fixture.session_id)
        .await
        .expect("latest attempt query")
        .expect("attempt exists");
    assert_eq!(attempt.status, RuntimeAttemptStatus::Running);
    assert_eq!(attempt.native_session_ref.as_deref(), Some("thread-abc123"));

    plane
        .mark_attempt_terminal(&NativeRuntimeAttemptState {
            attempt_id: attempt_id.clone(),
            session_id: fixture.session_id.clone(),
            status: RuntimeAttemptStatus::Exited,
            native_session_ref: None,
            observed_at: 3,
        })
        .await
        .expect("attempt reconciles as exited");
    let attempt = runtime_session_repo::latest_attempt(fixture.store.pool(), &fixture.session_id)
        .await
        .expect("latest attempt query")
        .expect("attempt exists");
    assert_eq!(attempt.status, RuntimeAttemptStatus::Exited);
}

#[tokio::test]
async fn sqlite_adapter_rejects_non_terminal_reconciliation() {
    let fixture = fixture().await;
    let plane = SqliteNativeRuntimeControlPlane::new(fixture.store.pool().clone());
    let attempt_id = RuntimeAttemptId::new();
    plane
        .start_attempt(&start_request(&fixture, attempt_id.clone()))
        .await
        .expect("attempt starts");

    let error = plane
        .mark_attempt_terminal(&NativeRuntimeAttemptState {
            attempt_id,
            session_id: fixture.session_id.clone(),
            status: RuntimeAttemptStatus::Running,
            native_session_ref: None,
            observed_at: 2,
        })
        .await
        .expect_err("running is not a terminal state");
    assert!(matches!(error, NativeRuntimeControlError::Invalid(_)));
}

#[tokio::test]
async fn sqlite_adapter_serves_session_view_for_resume() {
    let fixture = fixture().await;
    let plane = SqliteNativeRuntimeControlPlane::new(fixture.store.pool().clone());

    let requested = plane
        .session_view(&fixture.session_id)
        .await
        .expect("view query")
        .expect("session exists");
    assert_eq!(requested.session_id, fixture.session_id);
    assert_eq!(requested.task_id, fixture.task_id);
    assert_eq!(
        requested.status,
        agentd_core::types::RuntimeSessionStatus::Requested
    );
    assert_eq!(requested.latest_native_session_ref, None);

    let attempt_id = RuntimeAttemptId::new();
    plane
        .start_attempt(&start_request(&fixture, attempt_id.clone()))
        .await
        .expect("attempt starts");
    plane
        .update_attempt(&NativeRuntimeAttemptState {
            attempt_id,
            session_id: fixture.session_id.clone(),
            status: RuntimeAttemptStatus::Running,
            native_session_ref: Some("thread-resume-1".to_string()),
            observed_at: 2,
        })
        .await
        .expect("attempt updates");

    let running = plane
        .session_view(&fixture.session_id)
        .await
        .expect("view query")
        .expect("session exists");
    assert_eq!(
        running.latest_native_session_ref.as_deref(),
        Some("thread-resume-1")
    );

    let missing = plane
        .session_view(&RuntimeSessionId::new())
        .await
        .expect("view query");
    assert!(missing.is_none());
}

#[tokio::test]
async fn sqlite_adapter_rejects_attempt_for_foreign_task() {
    let fixture = fixture().await;
    let plane = SqliteNativeRuntimeControlPlane::new(fixture.store.pool().clone());

    let mut request = start_request(&fixture, RuntimeAttemptId::new());
    request.task_id = TaskRunId::new();
    let error = plane
        .start_attempt(&request)
        .await
        .expect_err("attempt for a foreign task must be rejected");
    assert!(matches!(error, NativeRuntimeControlError::Conflict(_)));
}
