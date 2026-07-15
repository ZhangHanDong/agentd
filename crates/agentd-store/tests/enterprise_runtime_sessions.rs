use agentd_core::types::{
    AgentProfileId, NodeId, RunId, RuntimeAttemptId, RuntimeAttemptStatus, RuntimeSessionId,
    RuntimeSessionStatus, TaskRunId, WorkerId, WorkerIncarnationId,
};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::runtime_session_repo::{
    self, ExecutionSnapshotRef, RuntimeAttemptCreate, RuntimeSessionCreate,
};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, StoreError, run_repo, task_repo};
use serde_json::json;

struct Fixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    task_id: TaskRunId,
    profile_id: AgentProfileId,
    worker_id: WorkerId,
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
            daemon_version: "0.0.0-p267".to_string(),
            host_name: "host-a".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
        },
    )
    .await
    .expect("incarnation");
    Fixture {
        store,
        _dir: dir,
        task_id,
        profile_id,
        worker_id,
        incarnation_id,
    }
}

fn snapshot() -> ExecutionSnapshotRef {
    ExecutionSnapshotRef {
        authority_key: "specify:corp".to_string(),
        resource_kind: "execution_snapshot".to_string(),
        resource_id: "snapshot-42".to_string(),
        resource_version: "7".to_string(),
        content_sha256: "a".repeat(64),
    }
}

fn session(id: RuntimeSessionId, fixture: &Fixture) -> RuntimeSessionCreate {
    RuntimeSessionCreate {
        id,
        execution_task_id: fixture.task_id.clone(),
        agent_profile_id: fixture.profile_id.clone(),
        snapshot: snapshot(),
    }
}

fn attempt(
    id: RuntimeAttemptId,
    incarnation_id: WorkerIncarnationId,
    target: &str,
) -> RuntimeAttemptCreate {
    RuntimeAttemptCreate {
        id,
        worker_incarnation_id: incarnation_id,
        backend_target: Some(target.to_string()),
        session_name: Some(format!("session-{target}")),
        pane_id: None,
        pid: Some(1234),
        native_session_ref: Some("codex-resume-ref".to_string()),
        workdir: Some("/tmp/worktree".to_string()),
    }
}

#[tokio::test]
async fn runtime_session_attempt_resume_keeps_session_and_immutable_placement() {
    let fixture = fixture().await;
    let session_id = RuntimeSessionId::new();
    let created = runtime_session_repo::create_session(
        fixture.store.pool(),
        session(session_id.clone(), &fixture),
    )
    .await
    .expect("create session");
    assert_eq!(created.status, RuntimeSessionStatus::Requested);
    assert_eq!(created.snapshot, snapshot());

    let first_id = RuntimeAttemptId::new();
    let first = runtime_session_repo::start_attempt(
        fixture.store.pool(),
        &session_id,
        attempt(first_id.clone(), fixture.incarnation_id.clone(), "first"),
    )
    .await
    .expect("first attempt");
    assert!(first.is_current);
    assert_eq!(first.runtime_session_id, session_id);
    assert_eq!(first.worker_incarnation_id, fixture.incarnation_id);

    let gone =
        runtime_session_repo::mark_attempt_gone(fixture.store.pool(), &session_id, &first_id)
            .await
            .expect("mark gone");
    assert_eq!(gone.status, RuntimeAttemptStatus::Gone);
    assert!(!gone.is_current);
    let pending = runtime_session_repo::get_session(fixture.store.pool(), &session_id)
        .await
        .expect("session")
        .expect("exists");
    assert_eq!(pending.status, RuntimeSessionStatus::ResumePending);

    let second_id = RuntimeAttemptId::new();
    let second = runtime_session_repo::start_attempt(
        fixture.store.pool(),
        &session_id,
        attempt(second_id.clone(), fixture.incarnation_id.clone(), "second"),
    )
    .await
    .expect("resume attempt");
    assert!(second.is_current);
    assert_eq!(second.runtime_session_id, session_id);
    assert_eq!(
        runtime_session_repo::current_attempt(fixture.store.pool(), &session_id)
            .await
            .expect("current")
            .expect("current exists")
            .id,
        second_id
    );
    let first_after = runtime_session_repo::get_attempt(fixture.store.pool(), &first_id)
        .await
        .expect("first")
        .expect("exists");
    assert_eq!(first_after.status, RuntimeAttemptStatus::Gone);
    assert!(!first_after.is_current);

    let session_after = runtime_session_repo::get_session(fixture.store.pool(), &session_id)
        .await
        .expect("session")
        .expect("exists");
    assert_eq!(session_after.snapshot, snapshot());
    assert_eq!(session_after.execution_task_id, fixture.task_id);
    assert_eq!(session_after.agent_profile_id, fixture.profile_id);
}

#[tokio::test]
async fn runtime_session_rejects_terminal_or_stale_worker_attempt() {
    let fixture = fixture().await;
    let terminal_session_id = RuntimeSessionId::new();
    runtime_session_repo::create_session(
        fixture.store.pool(),
        session(terminal_session_id.clone(), &fixture),
    )
    .await
    .expect("terminal session seed");
    runtime_session_repo::transition_session_status(
        fixture.store.pool(),
        &terminal_session_id,
        RuntimeSessionStatus::Cancelled,
        Some("operator_cancelled"),
    )
    .await
    .expect("cancel session");
    let terminal_error = runtime_session_repo::start_attempt(
        fixture.store.pool(),
        &terminal_session_id,
        attempt(
            RuntimeAttemptId::new(),
            fixture.incarnation_id.clone(),
            "terminal",
        ),
    )
    .await
    .expect_err("terminal session attempt");
    assert!(
        matches!(terminal_error, StoreError::Conflict(_)),
        "got {terminal_error:?}"
    );

    let stale_incarnation = fixture.incarnation_id.clone();
    worker_repo::register_incarnation(
        fixture.store.pool(),
        &fixture.worker_id,
        WorkerRegistration {
            id: WorkerIncarnationId::new(),
            daemon_version: "0.0.0-p267".to_string(),
            host_name: "host-b".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
        },
    )
    .await
    .expect("supersede worker");
    let active_session_id = RuntimeSessionId::new();
    runtime_session_repo::create_session(
        fixture.store.pool(),
        session(active_session_id.clone(), &fixture),
    )
    .await
    .expect("active session");
    let stale_error = runtime_session_repo::start_attempt(
        fixture.store.pool(),
        &active_session_id,
        attempt(RuntimeAttemptId::new(), stale_incarnation, "stale"),
    )
    .await
    .expect_err("stale incarnation attempt");
    assert!(
        matches!(stale_error, StoreError::Conflict(_)),
        "got {stale_error:?}"
    );

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_attempts")
        .fetch_one(fixture.store.pool())
        .await
        .expect("count attempts");
    assert_eq!(count, 0);
}
