use agentd_bin::cli::DaemonConfig;
use agentd_bin::daemon::{DispatchRoute, dispatch_task_to_fleet, production_dispatch_route};
use agentd_core::ports::DurableSchedulerPort as _;
use agentd_core::types::{
    AgentProfileId, AuthorityKey, CertificationPolicyVersionRef, FrozenSpecVersionRef,
    MatrixRoomRef, NativeExecutionSpec, NodeId, OfflineRecoveryPolicy, OrganizationRef,
    ProductWorkflowRef, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef,
    ProjectRoomBindingRef, QuotaPolicyVersionRef, RbacPolicyVersionRef, RepositoryBinding,
    RepositoryRef, RepositoryRole, RequirementRef, RoomBinding, RoomBindingRole, RunId,
    RuntimeSessionId, TaskRunId, TeamRef, WorkerId, WorkerIncarnationId,
};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::runtime_session_repo::{self, ExecutionSnapshotRef, RuntimeSessionCreate};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
use serde_json::json;

fn config_with_native_dispatch(native: bool) -> DaemonConfig {
    let mut config = DaemonConfig::for_test();
    config.native_dispatch = native;
    config
}

#[tokio::test]
async fn default_route_is_tmux_and_switch_selects_native_queue() {
    assert_eq!(
        production_dispatch_route(&config_with_native_dispatch(false)),
        DispatchRoute::Tmux
    );
    assert_eq!(
        production_dispatch_route(&config_with_native_dispatch(true)),
        DispatchRoute::NativeQueue
    );
}

#[tokio::test]
async fn dispatch_task_to_fleet_enqueues_a_queued_row_with_spec() {
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
    let spec = NativeExecutionSpec {
        version: 1,
        provider: "codex".into(),
        program: "/usr/bin/codex".into(),
        args: vec![],
        cwd: None,
        env: vec![],
    };

    dispatch_task_to_fleet(&store, &task_id, &spec, 100)
        .await
        .expect("dispatch");

    let (status, provider): (String, Option<String>) = sqlx::query_as(
        "SELECT q.status, json_extract(t.execution_spec_json, '$.provider') \
         FROM execution_task_queue q JOIN task_runs t ON t.id = q.execution_task_id \
         WHERE q.execution_task_id = ?",
    )
    .bind(task_id.as_str())
    .fetch_one(store.pool())
    .await
    .expect("queue row");
    assert_eq!(status, "queued");
    assert_eq!(provider.as_deref(), Some("codex"));
}

/// M2 Plan B Task 6 exit-gate fixture: seeds a run + task + agent profile +
/// worker + runtime session so a native worker can pull and execute a task
/// that was queued via the production `dispatch_task_to_fleet` primitive
/// (not a hand-written `enqueue`). Mirrors
/// `crates/agentd-bin/tests/worker_main.rs::fixture()`.
#[allow(dead_code)]
struct DispatchFixture {
    store: SqliteStore,
    _dir: tempfile::TempDir,
    run_id: RunId,
    session_id: RuntimeSessionId,
    task_id: TaskRunId,
    incarnation_id: WorkerIncarnationId,
}

async fn dispatch_fixture() -> DispatchFixture {
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
            daemon_version: "0.0.0-m2-plan-b-t6".to_string(),
            host_name: "host-a".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex", "claude-code"]}),
            capacity: 1,
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
    DispatchFixture {
        store,
        _dir: dir,
        run_id,
        session_id,
        task_id,
        incarnation_id,
    }
}

async fn serve_dispatch_daemon(store: SqliteStore, token: &str) -> String {
    let fleet = std::sync::Arc::new(agentd_store::worker_fleet::SqliteWorkerFleet::new(
        store.pool().clone(),
    ));
    let artifacts = std::sync::Arc::new(
        agentd_store::content_store::LocalContentStore::new(
            std::env::temp_dir().join(format!("agentd-m2b-t6-artifacts-{}", std::process::id())),
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
/// security scope for the native grant. Mirrors
/// `crates/agentd-bin/tests/worker_main.rs::authority_snapshot()`.
fn dispatch_authority_snapshot() -> ProjectExecutionSnapshot {
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
async fn production_native_dispatch_is_executed_by_a_worker_without_tmux() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = dispatch_fixture().await;
    let store = fixture.store.clone();
    let task_id = fixture.task_id.clone();
    let session_id = fixture.session_id.clone();

    agentd_store::project_authority_repo::record_snapshot(
        store.pool(),
        &dispatch_authority_snapshot(),
    )
    .await
    .expect("record project authority snapshot");

    // A codex shim that exits immediately (basename must equal the provider).
    let shim_dir = tempfile::tempdir().expect("shim dir");
    let shim = shim_dir.path().join("codex");
    std::fs::write(&shim, "#!/bin/sh\nexit 0\n").expect("write shim");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let spec = NativeExecutionSpec {
        version: 1,
        provider: "codex".into(),
        program: shim.to_string_lossy().into_owned(),
        args: vec![],
        cwd: Some(shim_dir.path().to_string_lossy().into_owned()),
        env: vec![],
    };

    // Production dispatch decision: the switch routes to the native queue, and
    // the native launch primitive enqueues the task. No tmux is involved.
    assert_eq!(
        production_dispatch_route(&config_with_native_dispatch(true)),
        DispatchRoute::NativeQueue
    );
    dispatch_task_to_fleet(&store, &task_id, &spec, 100)
        .await
        .expect("native dispatch");

    // A real native worker pulls the dispatched task over HTTP and executes it.
    let base_url = serve_dispatch_daemon(store.clone(), "worker-secret").await;
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

    // The worker's release marks the lease `released`; the durable scheduler's
    // reconcile pass (normally run on the daemon's maintenance tick, see
    // `worker_fleet_tick`) is what threads that into the queue row's terminal
    // status. Run one pass here rather than waiting on the interval.
    let scheduler =
        agentd_store::durable_scheduler::SqliteDurableScheduler::new(store.pool().clone());
    scheduler.reconcile(200).await.expect("reconcile");

    // Daemon-side: the queue row completed and the session is Completed.
    let (status,): (String,) =
        sqlx::query_as("SELECT status FROM execution_task_queue WHERE execution_task_id = ?")
            .bind(task_id.as_str())
            .fetch_one(store.pool())
            .await
            .expect("queue row");
    assert_eq!(status, "completed");
    let session = runtime_session_repo::get_session(store.pool(), &session_id)
        .await
        .expect("session lookup")
        .expect("session");
    assert_eq!(
        session.status,
        agentd_core::types::RuntimeSessionStatus::Completed
    );
}
