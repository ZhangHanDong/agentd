use std::path::PathBuf;

use agentd_core::types::{
    AgentProfileId, Artifact, ArtifactKind, ExecutionArtifactId, NodeId, RunId, RuntimeAttemptId,
    RuntimeSessionId, TaskRunId, WorkerId, WorkerIncarnationId,
};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::execution_artifact_repo::{
    self, CertificationRefKind, ExecutionArtifactCreate, ExecutionArtifactKind,
};
use agentd_store::runtime_session_repo::{
    self, ExecutionSnapshotRef, RuntimeAttemptCreate, RuntimeSessionCreate,
};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, StoreError, artifact_repo, run_repo, task_repo};
use serde_json::json;

struct Fixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    run_id: RunId,
    task_id: TaskRunId,
    profile_id: AgentProfileId,
    snapshot: ExecutionSnapshotRef,
    worker_id: WorkerId,
    incarnation_id: WorkerIncarnationId,
    session_id: RuntimeSessionId,
    attempt_id: RuntimeAttemptId,
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
    let snapshot = snapshot();
    let session_id = RuntimeSessionId::new();
    runtime_session_repo::create_session(
        store.pool(),
        RuntimeSessionCreate {
            id: session_id.clone(),
            execution_task_id: task_id.clone(),
            agent_profile_id: profile_id.clone(),
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
    Fixture {
        store,
        _dir: dir,
        run_id,
        task_id,
        profile_id,
        snapshot,
        worker_id,
        incarnation_id,
        session_id,
        attempt_id,
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

fn registration(id: WorkerIncarnationId, host: &str) -> WorkerRegistration {
    WorkerRegistration {
        id,
        daemon_version: "0.0.0-p268".to_string(),
        host_name: host.to_string(),
        network_zone: Some("dev".to_string()),
        capabilities: json!({"runtime": ["codex"]}),
    }
}

fn artifact(id: ExecutionArtifactId, fixture: &Fixture) -> ExecutionArtifactCreate {
    ExecutionArtifactCreate {
        id,
        kind: ExecutionArtifactKind::TestReport,
        content_sha256: "b".repeat(64),
        size_bytes: 42,
        media_type: "application/json".to_string(),
        storage_ref: "cas://sha256/test-report".to_string(),
        provenance: json!({"tool": "cargo-test", "version": 1}),
        execution_run_id: fixture.run_id.clone(),
        execution_task_id: Some(fixture.task_id.clone()),
        runtime_session_id: Some(fixture.session_id.clone()),
        runtime_attempt_id: Some(fixture.attempt_id.clone()),
        snapshot: fixture.snapshot.clone(),
        target_repository_id: "repo_01ARZ3NDEKTSV4RRFFQ69G5FAV".to_string(),
        target_base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        producer_worker_incarnation_id: Some(fixture.incarnation_id.clone()),
    }
}

async fn assert_certification_refs(fixture: &Fixture, artifact_id: &ExecutionArtifactId) {
    for (kind, external_ref) in [
        (CertificationRefKind::Request, "of-request-1"),
        (CertificationRefKind::Result, "of-result-1"),
        (CertificationRefKind::Signature, "of-signature-1"),
        (CertificationRefKind::Attestation, "of-attestation-1"),
    ] {
        let first = execution_artifact_repo::append_certification_ref(
            fixture.store.pool(),
            artifact_id,
            "openfab:prod",
            kind,
            external_ref,
        )
        .await
        .expect("append certification ref");
        let retry = execution_artifact_repo::append_certification_ref(
            fixture.store.pool(),
            artifact_id,
            "openfab:prod",
            kind,
            external_ref,
        )
        .await
        .expect("idempotent certification ref");
        assert_eq!(retry, first);
    }
    let changed = execution_artifact_repo::append_certification_ref(
        fixture.store.pool(),
        artifact_id,
        "openfab:prod",
        CertificationRefKind::Request,
        "of-request-changed",
    )
    .await
    .expect_err("request ref cannot change");
    assert!(
        matches!(changed, StoreError::Conflict(_)),
        "got {changed:?}"
    );

    let update = sqlx::query("UPDATE artifact_certification_refs SET external_ref='changed'")
        .execute(fixture.store.pool())
        .await;
    assert!(update.is_err());
    let delete = sqlx::query("DELETE FROM artifact_certification_refs")
        .execute(fixture.store.pool())
        .await;
    assert!(delete.is_err());
    let cert_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artifact_certification_refs")
        .fetch_one(fixture.store.pool())
        .await
        .expect("cert count");
    assert_eq!(cert_count, 4);
}

#[tokio::test]
async fn execution_artifact_persists_immutable_provenance_and_parent_graph() {
    let fixture = fixture().await;
    let id = ExecutionArtifactId::new();
    let expected = artifact(id.clone(), &fixture);
    let record = execution_artifact_repo::create_artifact(fixture.store.pool(), expected.clone())
        .await
        .expect("create artifact");
    assert_eq!(record.id, id);
    assert_eq!(record.kind, expected.kind);
    assert_eq!(record.content_sha256, expected.content_sha256);
    assert_eq!(record.size_bytes, expected.size_bytes);
    assert_eq!(record.media_type, expected.media_type);
    assert_eq!(record.storage_ref, expected.storage_ref);
    assert_eq!(record.provenance, expected.provenance);
    assert_eq!(record.execution_run_id, fixture.run_id);
    assert_eq!(record.execution_task_id, Some(fixture.task_id));
    assert_eq!(record.runtime_session_id, Some(fixture.session_id));
    assert_eq!(record.runtime_attempt_id, Some(fixture.attempt_id));
    assert_eq!(record.snapshot, fixture.snapshot);
    assert_eq!(
        record.producer_worker_incarnation_id,
        Some(fixture.incarnation_id)
    );

    let update = sqlx::query("UPDATE execution_artifacts SET media_type='text/plain' WHERE id=?")
        .bind(id.as_str())
        .execute(fixture.store.pool())
        .await;
    assert!(update.is_err(), "artifact update must be rejected");
    let delete = sqlx::query("DELETE FROM execution_artifacts WHERE id=?")
        .bind(id.as_str())
        .execute(fixture.store.pool())
        .await;
    assert!(delete.is_err(), "artifact delete must be rejected");
    assert_eq!(
        execution_artifact_repo::get_artifact(fixture.store.pool(), &id)
            .await
            .expect("get artifact"),
        Some(record)
    );
}

#[tokio::test]
async fn execution_artifact_rejects_invalid_metadata_or_parent_graph() {
    let fixture = fixture().await;
    let mut invalid = artifact(ExecutionArtifactId::from_string("ar_bad"), &fixture);
    invalid.content_sha256 = "not-a-sha".to_string();
    invalid.provenance = json!("not-an-object");
    let error = execution_artifact_repo::create_artifact(fixture.store.pool(), invalid)
        .await
        .expect_err("invalid metadata");
    assert!(matches!(error, StoreError::Invariant(_)), "got {error:?}");

    let other_run = RunId::new();
    run_repo::insert_run(fixture.store.pool(), &other_run, "other-workflow")
        .await
        .expect("other run");
    let other_task =
        task_repo::insert_task_run(fixture.store.pool(), &other_run, &NodeId::parsed("other"))
            .await
            .expect("other task");
    let mut wrong_task = artifact(ExecutionArtifactId::new(), &fixture);
    wrong_task.execution_task_id = Some(other_task);
    let error = execution_artifact_repo::create_artifact(fixture.store.pool(), wrong_task)
        .await
        .expect_err("task from another run");
    assert!(matches!(error, StoreError::Conflict(_)), "got {error:?}");

    let second_session = RuntimeSessionId::new();
    runtime_session_repo::create_session(
        fixture.store.pool(),
        RuntimeSessionCreate {
            id: second_session.clone(),
            execution_task_id: fixture.task_id.clone(),
            agent_profile_id: fixture.profile_id.clone(),
            snapshot: fixture.snapshot.clone(),
        },
    )
    .await
    .expect("second session");
    let second_attempt = RuntimeAttemptId::new();
    runtime_session_repo::start_attempt(
        fixture.store.pool(),
        &second_session,
        RuntimeAttemptCreate {
            id: second_attempt.clone(),
            worker_incarnation_id: fixture.incarnation_id.clone(),
            backend_target: Some("native://other-attempt".to_string()),
            session_name: None,
            pane_id: None,
            pid: Some(101),
            native_session_ref: None,
            workdir: Some("/tmp/other-worktree".to_string()),
        },
    )
    .await
    .expect("second attempt");
    let mut wrong_attempt = artifact(ExecutionArtifactId::new(), &fixture);
    wrong_attempt.runtime_attempt_id = Some(second_attempt);
    let error = execution_artifact_repo::create_artifact(fixture.store.pool(), wrong_attempt)
        .await
        .expect_err("attempt from another session");
    assert!(matches!(error, StoreError::Conflict(_)), "got {error:?}");

    let second_incarnation = WorkerIncarnationId::new();
    worker_repo::register_incarnation(
        fixture.store.pool(),
        &fixture.worker_id,
        registration(second_incarnation.clone(), "host-b"),
    )
    .await
    .expect("second incarnation");
    let mut wrong_worker = artifact(ExecutionArtifactId::new(), &fixture);
    wrong_worker.producer_worker_incarnation_id = Some(second_incarnation);
    let error = execution_artifact_repo::create_artifact(fixture.store.pool(), wrong_worker)
        .await
        .expect_err("producer differs from attempt worker");
    assert!(matches!(error, StoreError::Conflict(_)), "got {error:?}");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution_artifacts")
        .fetch_one(fixture.store.pool())
        .await
        .expect("artifact count");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn legacy_mapping_and_certification_refs_are_explicit_and_append_only() {
    let fixture = fixture().await;
    let sha = "c".repeat(64);
    let legacy = Artifact {
        kind: ArtifactKind::Transcript,
        path: PathBuf::from("legacy/transcript.log"),
        sha256: sha.clone(),
        bytes: 64,
    };
    artifact_repo::insert_artifact(
        fixture.store.pool(),
        &legacy,
        Some(&fixture.run_id),
        Some(&NodeId::parsed("impl")),
    )
    .await
    .expect("legacy artifact");

    let artifact_id = ExecutionArtifactId::new();
    let mut request = artifact(artifact_id.clone(), &fixture);
    request.content_sha256 = sha.clone();
    request.size_bytes = 64;
    execution_artifact_repo::create_artifact(fixture.store.pool(), request)
        .await
        .expect("enterprise artifact");
    let mapping =
        execution_artifact_repo::map_legacy_artifact(fixture.store.pool(), &sha, &artifact_id)
            .await
            .expect("mapping");
    assert_eq!(mapping.legacy_sha256, sha);
    assert_eq!(mapping.execution_artifact_id, artifact_id);
    assert_eq!(
        execution_artifact_repo::map_legacy_artifact(
            fixture.store.pool(),
            &mapping.legacy_sha256,
            &mapping.execution_artifact_id,
        )
        .await
        .expect("idempotent mapping"),
        mapping
    );

    let other_id = ExecutionArtifactId::new();
    let mut other = artifact(other_id.clone(), &fixture);
    other.content_sha256 = mapping.legacy_sha256.clone();
    other.size_bytes = 64;
    execution_artifact_repo::create_artifact(fixture.store.pool(), other)
        .await
        .expect("second enterprise artifact");
    let remap = execution_artifact_repo::map_legacy_artifact(
        fixture.store.pool(),
        &mapping.legacy_sha256,
        &other_id,
    )
    .await
    .expect_err("legacy row cannot remap");
    assert!(matches!(remap, StoreError::Conflict(_)), "got {remap:?}");

    assert_certification_refs(&fixture, &artifact_id).await;

    let legacy_after = artifact_repo::get_artifact(fixture.store.pool(), &mapping.legacy_sha256)
        .await
        .expect("legacy get")
        .expect("legacy exists");
    assert_eq!(legacy_after.kind, legacy.kind);
    assert_eq!(legacy_after.path, legacy.path);
    assert_eq!(legacy_after.bytes, legacy.bytes);
}
