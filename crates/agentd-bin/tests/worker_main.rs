//! M1 Task 5: the `agentd worker` remote execution loop end-to-end, exercised
//! entirely over the authenticated daemon HTTP control plane (worker-fleet +
//! native runtime + artifact upload/acknowledge).

use std::os::unix::fs::PermissionsExt as _;

use agentd_core::ports::Store as _;
use agentd_core::types::{
    AgentProfileId, AuthorityKey, CertificationPolicyVersionRef, FrozenSpecVersionRef,
    MatrixRoomRef, NodeId, OfflineRecoveryPolicy, OrganizationRef, ProductWorkflowRef,
    ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef, ProjectRoomBindingRef,
    QuotaPolicyVersionRef, RbacPolicyVersionRef, RepositoryBinding, RepositoryRef, RepositoryRole,
    RequirementRef, RoomBinding, RoomBindingRole, RunId, RuntimeSessionId, TaskRunId, TeamRef,
    WorkerId, WorkerIncarnationId,
};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::runtime_session_repo::{self, ExecutionSnapshotRef, RuntimeSessionCreate};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
use serde_json::json;

#[allow(dead_code)]
struct Fixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    run_id: RunId,
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
            daemon_version: "0.0.0-m1-t5".to_string(),
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
        run_id,
        session_id,
        task_id,
        incarnation_id,
    }
}

async fn serve_daemon(store: SqliteStore, token: &str) -> String {
    let fleet = std::sync::Arc::new(agentd_store::worker_fleet::SqliteWorkerFleet::new(
        store.pool().clone(),
    ));
    let artifacts = std::sync::Arc::new(
        agentd_store::content_store::LocalContentStore::new(
            std::env::temp_dir().join(format!("agentd-m1-t5-artifacts-{}", std::process::id())),
        )
        .expect("content store"),
    );
    let service = std::sync::Arc::new(agentd_bin::daemon::WorkerFleetService::new(
        fleet,
        agentd_bin::native_worker::AgentdWorker::new(store.clone()),
        artifacts,
    ));
    let auth = agentd_surface::http::AuthConfig {
        api_token: Some(token.to_string()),
        ..agentd_surface::http::AuthConfig::default()
    };
    let fleet_router = agentd_surface::worker_fleet_http::worker_fleet_router(
        std::sync::Arc::new(
            agentd_store::worker_fleet::SqliteWorkerFleet::new(store.pool().clone())
                .with_auth_proof(token.to_string()),
        ),
        auth,
    );
    let app = agentd_bin::daemon::daemon_native_runtime_router(&store, Some(token.to_string()))
        .merge(agentd_bin::daemon::recovery_router(
            service,
            token.to_string(),
        ))
        .merge(fleet_router);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}

/// Build a minimal, internally-consistent project execution snapshot whose
/// `snapshot_ref` matches the fixture session's `specify:execution_snapshot:
/// spec-1:v1` authority reference, so `SqliteWorkerFleet::pull` can resolve a
/// security scope for the native grant.
fn authority_snapshot() -> ProjectExecutionSnapshot {
    let authority_key = AuthorityKey::new("specify").expect("authority key");
    let project_ref =
        ProjectRef::new(authority_key.clone(), "project-1", "7").expect("project ref");
    let rbac_ref =
        RbacPolicyVersionRef::new(authority_key.clone(), "rbac-1", "4").expect("rbac ref");
    ProjectExecutionSnapshot {
        snapshot_ref: ProjectExecutionSnapshotRef::new(authority_key.clone(), "spec-1", "v1")
            .expect("snapshot ref"),
        authority_key: authority_key.clone(),
        authority_revision: 9,
        organization_ref: OrganizationRef::new(authority_key.clone(), "org-1", "2")
            .expect("organization ref"),
        team_refs: vec![
            TeamRef::new(authority_key.clone(), "team-runtime", "3").expect("team ref"),
        ],
        project_ref: project_ref.clone(),
        repository_bindings: vec![RepositoryBinding {
            repository_ref: RepositoryRef::new(authority_key.clone(), "repo-1", "5")
                .expect("repository ref"),
            role: RepositoryRole::Target,
            forge_locator: Some("github:corp/repo".to_string()),
            base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        }],
        room_bindings: vec![RoomBinding {
            binding_ref: ProjectRoomBindingRef::new(authority_key.clone(), "binding-1", "6")
                .expect("binding ref"),
            project_ref,
            matrix_room_ref: MatrixRoomRef::new(
                AuthorityKey::new("matrix:corp").expect("matrix authority"),
                "!room:corp",
                "11",
            )
            .expect("matrix room ref"),
            roles: vec![RoomBindingRole::Command],
            allowed_command_classes: vec!["execute".to_string()],
            rbac_policy_version_ref: rbac_ref.clone(),
        }],
        issue_ref: None,
        requirement_refs: vec![
            RequirementRef::new(authority_key.clone(), "req-1", "8").expect("requirement ref"),
        ],
        frozen_spec_version_ref: FrozenSpecVersionRef::new(
            authority_key.clone(),
            "spec-doc-1",
            "12",
        )
        .expect("spec ref"),
        product_workflow_ref: ProductWorkflowRef::new(authority_key.clone(), "workflow-1", "13")
            .expect("workflow ref"),
        rbac_policy_version_ref: rbac_ref,
        quota_policy_version_ref: QuotaPolicyVersionRef::new(
            authority_key.clone(),
            "quota-1",
            "14",
        )
        .expect("quota ref"),
        certification_policy_version_ref: Some(
            CertificationPolicyVersionRef::new(authority_key.clone(), "cert-policy-1", "15")
                .expect("certification policy ref"),
        ),
        issued_at: 100,
        valid_until: 4_102_444_800,
        content_sha256: "a".repeat(64),
        offline_recovery_policy: OfflineRecoveryPolicy::Deny,
    }
}

#[tokio::test]
async fn worker_once_executes_a_dispatched_task_end_to_end() {
    let fixture = fixture().await;

    // A "codex" provider shim that exits immediately; the execution spec's
    // program must satisfy `provider_matches_program` (basename == provider).
    let shim_dir = tempfile::tempdir().expect("shim dir");
    let shim = shim_dir.path().join("codex");
    std::fs::write(&shim, "#!/bin/sh\nexit 0\n").expect("write shim");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let spec = agentd_core::types::NativeExecutionSpec {
        version: 1,
        provider: "codex".into(),
        program: shim.to_string_lossy().into_owned(),
        args: vec![],
        cwd: Some(shim_dir.path().to_string_lossy().into_owned()),
        env: vec![],
    };
    fixture
        .store
        .set_task_execution_spec(&fixture.task_id, &spec)
        .await
        .expect("attach spec");

    // A native grant carries a security scope only once the daemon can
    // resolve a project-authority snapshot for the session's snapshot ref.
    agentd_store::project_authority_repo::record_snapshot(
        fixture.store.pool(),
        &authority_snapshot(),
    )
    .await
    .expect("record project authority snapshot");

    let base_url = serve_daemon(fixture.store.clone(), "worker-secret").await;
    let worker_state = tempfile::tempdir().expect("worker state");

    let report = agentd_bin::worker_main::run_worker_once(
        &base_url,
        "worker-secret",
        worker_state.path(),
        std::time::Duration::from_millis(100),
        std::time::Duration::from_secs(30),
    )
    .await
    .expect("worker run");

    assert_eq!(report.executed, 1);
    assert_eq!(report.released, 1);

    // The daemon-side session completed and an artifact was acknowledged.
    let session = runtime_session_repo::get_session(fixture.store.pool(), &fixture.session_id)
        .await
        .expect("session lookup")
        .expect("session");
    assert_eq!(
        session.status,
        agentd_core::types::RuntimeSessionStatus::Completed
    );
    let worker = agentd_bin::native_worker::AgentdWorker::new(fixture.store.clone());
    let artifacts = worker
        .list_artifacts_for_run(fixture.run_id.as_str())
        .await
        .expect("artifact listing");
    assert!(
        !artifacts.records.is_empty(),
        "worker must acknowledge at least the transcript artifact"
    );
}
