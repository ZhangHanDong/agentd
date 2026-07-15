use agentd_core::types::{
    AgentProfileId, AuditEventId, ExecutionArtifactId, NodeId, RunId, RuntimeAttemptId,
    RuntimeSessionId, TaskRunId, WorkerId, WorkerIncarnationId,
};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::execution_artifact_repo::{self, ExecutionArtifactCreate, ExecutionArtifactKind};
use agentd_store::execution_audit_repo::{self, AuditActorKind, AuditEventCreate};
use agentd_store::runtime_session_repo::{
    self, ExecutionSnapshotRef, RuntimeAttemptCreate, RuntimeSessionCreate,
};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, StoreError, run_repo, task_repo};
use serde_json::json;

struct Fixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    run_id: RunId,
    task_id: TaskRunId,
    snapshot: ExecutionSnapshotRef,
    worker_id: WorkerId,
    incarnation_id: WorkerIncarnationId,
    session_id: RuntimeSessionId,
    attempt_id: RuntimeAttemptId,
    artifact_id: ExecutionArtifactId,
}

struct RuntimeSeed {
    run_id: RunId,
    task_id: TaskRunId,
    snapshot: ExecutionSnapshotRef,
    worker_id: WorkerId,
    incarnation_id: WorkerIncarnationId,
    session_id: RuntimeSessionId,
    attempt_id: RuntimeAttemptId,
}

async fn seed_runtime(store: &SqliteStore) -> RuntimeSeed {
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
            labels: json!({"team": "runtime"}),
        },
    )
    .await
    .expect("worker");
    let incarnation_id = WorkerIncarnationId::new();
    worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        registration(incarnation_id.clone(), "host-a"),
    )
    .await
    .expect("incarnation");
    let snapshot = ExecutionSnapshotRef {
        authority_key: "specify:corp".to_string(),
        resource_kind: "execution_snapshot".to_string(),
        resource_id: "snapshot-42".to_string(),
        resource_version: "7".to_string(),
        content_sha256: "a".repeat(64),
    };
    let session_id = RuntimeSessionId::new();
    runtime_session_repo::create_session(
        store.pool(),
        RuntimeSessionCreate {
            id: session_id.clone(),
            execution_task_id: task_id.clone(),
            agent_profile_id: profile_id,
            snapshot: snapshot.clone(),
        },
    )
    .await
    .expect("session");
    let attempt_id = RuntimeAttemptId::new();
    runtime_session_repo::start_attempt(
        store.pool(),
        &session_id,
        RuntimeAttemptCreate {
            id: attempt_id.clone(),
            worker_incarnation_id: incarnation_id.clone(),
            backend_target: Some("native://attempt".to_string()),
            session_name: None,
            pane_id: None,
            pid: Some(100),
            native_session_ref: Some("codex-resume".to_string()),
            workdir: Some("/tmp/worktree".to_string()),
        },
    )
    .await
    .expect("attempt");
    RuntimeSeed {
        run_id,
        task_id,
        snapshot,
        worker_id,
        incarnation_id,
        session_id,
        attempt_id,
    }
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let seed = seed_runtime(&store).await;
    let artifact_id = ExecutionArtifactId::new();
    execution_artifact_repo::create_artifact(
        store.pool(),
        ExecutionArtifactCreate {
            id: artifact_id.clone(),
            kind: ExecutionArtifactKind::TestReport,
            content_sha256: "b".repeat(64),
            size_bytes: 42,
            media_type: "application/json".to_string(),
            storage_ref: "cas://sha256/test-report".to_string(),
            provenance: json!({"tool": "cargo-test"}),
            execution_run_id: seed.run_id.clone(),
            execution_task_id: Some(seed.task_id.clone()),
            runtime_session_id: Some(seed.session_id.clone()),
            runtime_attempt_id: Some(seed.attempt_id.clone()),
            snapshot: seed.snapshot.clone(),
            target_repository_id: "repo_01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
            target_base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            producer_worker_incarnation_id: Some(seed.incarnation_id.clone()),
        },
    )
    .await
    .expect("artifact");
    Fixture {
        store,
        _dir: dir,
        run_id: seed.run_id,
        task_id: seed.task_id,
        snapshot: seed.snapshot,
        worker_id: seed.worker_id,
        incarnation_id: seed.incarnation_id,
        session_id: seed.session_id,
        attempt_id: seed.attempt_id,
        artifact_id,
    }
}

fn registration(id: WorkerIncarnationId, host: &str) -> WorkerRegistration {
    WorkerRegistration {
        id,
        daemon_version: "0.0.0-p268".to_string(),
        host_name: host.to_string(),
        network_zone: Some("dev".to_string()),
        capabilities: json!({"runtime": ["codex"]}),
    }
}

fn event(
    id: AuditEventId,
    idempotency_key: &str,
    occurred_at: i64,
    fixture: &Fixture,
) -> AuditEventCreate {
    AuditEventCreate {
        id,
        idempotency_scope: format!("run:{}", fixture.run_id),
        idempotency_key: idempotency_key.to_string(),
        event_type: "artifact.recorded".to_string(),
        actor_kind: AuditActorKind::Worker,
        actor_ref: fixture.incarnation_id.to_string(),
        payload_sha256: "d".repeat(64),
        payload: json!({"artifact_id": fixture.artifact_id}),
        execution_run_id: fixture.run_id.clone(),
        execution_task_id: Some(fixture.task_id.clone()),
        runtime_session_id: Some(fixture.session_id.clone()),
        runtime_attempt_id: Some(fixture.attempt_id.clone()),
        execution_artifact_id: Some(fixture.artifact_id.clone()),
        worker_incarnation_id: Some(fixture.incarnation_id.clone()),
        snapshot: fixture.snapshot.clone(),
        target_repository_id: "repo_01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        target_base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        occurred_at,
    }
}

#[tokio::test]
async fn audit_append_is_ordered_idempotent_and_append_only() {
    let fixture = fixture().await;
    let first_request = event(AuditEventId::new(), "artifact-1", 200, &fixture);
    let first = execution_audit_repo::append_event(fixture.store.pool(), first_request.clone())
        .await
        .expect("append first");
    let retry = execution_audit_repo::append_event(fixture.store.pool(), first_request)
        .await
        .expect("retry first");
    assert_eq!(retry, first);

    let second_request = event(AuditEventId::new(), "artifact-2", 100, &fixture);
    let second = execution_audit_repo::append_event(fixture.store.pool(), second_request)
        .await
        .expect("append second");
    assert!(second.sequence > first.sequence);
    assert!(second.occurred_at < first.occurred_at);

    let all = execution_audit_repo::read_from(fixture.store.pool(), &fixture.run_id, 0)
        .await
        .expect("replay all");
    assert_eq!(all, vec![first.clone(), second.clone()]);
    let after_first =
        execution_audit_repo::read_from(fixture.store.pool(), &fixture.run_id, first.sequence)
            .await
            .expect("replay cursor");
    assert_eq!(after_first, vec![second]);

    let update = sqlx::query("UPDATE execution_audit_events SET event_type='changed'")
        .execute(fixture.store.pool())
        .await;
    assert!(update.is_err(), "audit update must be rejected");
    let delete = sqlx::query("DELETE FROM execution_audit_events")
        .execute(fixture.store.pool())
        .await;
    assert!(delete.is_err(), "audit delete must be rejected");
}

#[tokio::test]
async fn audit_append_rejects_changed_retries_and_mismatched_links() {
    let fixture = fixture().await;
    let original_request = event(AuditEventId::new(), "artifact-1", 200, &fixture);
    let original =
        execution_audit_repo::append_event(fixture.store.pool(), original_request.clone())
            .await
            .expect("append original");

    let mut changed_retry = original_request.clone();
    changed_retry.payload_sha256 = "e".repeat(64);
    changed_retry.payload = json!({"changed": true});
    let error = execution_audit_repo::append_event(fixture.store.pool(), changed_retry)
        .await
        .expect_err("changed idempotent retry");
    assert!(matches!(error, StoreError::Conflict(_)), "got {error:?}");

    let mut reused_id = original_request.clone();
    reused_id.idempotency_key = "different-key".to_string();
    let error = execution_audit_repo::append_event(fixture.store.pool(), reused_id)
        .await
        .expect_err("event id reused for another key");
    assert!(matches!(error, StoreError::Conflict(_)), "got {error:?}");

    let other_run = RunId::new();
    run_repo::insert_run(fixture.store.pool(), &other_run, "other-workflow")
        .await
        .expect("other run");
    let other_task =
        task_repo::insert_task_run(fixture.store.pool(), &other_run, &NodeId::parsed("other"))
            .await
            .expect("other task");
    let mut wrong_task = event(AuditEventId::new(), "wrong-task", 201, &fixture);
    wrong_task.execution_task_id = Some(other_task);
    let error = execution_audit_repo::append_event(fixture.store.pool(), wrong_task)
        .await
        .expect_err("task from another run");
    assert!(matches!(error, StoreError::Conflict(_)), "got {error:?}");

    let second_incarnation = WorkerIncarnationId::new();
    worker_repo::register_incarnation(
        fixture.store.pool(),
        &fixture.worker_id,
        registration(second_incarnation.clone(), "host-b"),
    )
    .await
    .expect("second incarnation");
    let mut wrong_worker = event(AuditEventId::new(), "wrong-worker", 202, &fixture);
    wrong_worker.worker_incarnation_id = Some(second_incarnation);
    let error = execution_audit_repo::append_event(fixture.store.pool(), wrong_worker)
        .await
        .expect_err("worker differs from attempt/artifact producer");
    assert!(matches!(error, StoreError::Conflict(_)), "got {error:?}");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_audit_events")
        .fetch_one(fixture.store.pool())
        .await
        .expect("audit count");
    let max_sequence: i64 = sqlx::query_scalar("SELECT MAX(sequence) FROM execution_audit_events")
        .fetch_one(fixture.store.pool())
        .await
        .expect("max sequence");
    assert_eq!(count, 1);
    assert_eq!(max_sequence, original.sequence);
    assert_eq!(
        execution_audit_repo::read_from(fixture.store.pool(), &fixture.run_id, 0)
            .await
            .expect("original remains"),
        vec![original]
    );
}
