//! M3 exit gate: a project's agents register, message each other, and run a
//! task graph whose native node is executed by a real worker — with no
//! agent-chat process and no tmux anywhere in the path.

use std::sync::Arc;

use agentd_bin::{ProductionRunHost, SystemClock, daemon};
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::{
    AgentProfileId, AuthorityKey, CertificationPolicyVersionRef, FrozenSpecVersionRef,
    MatrixRoomRef, OfflineRecoveryPolicy, OrganizationRef, ProductWorkflowRef,
    ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef, ProjectRoomBindingRef,
    QuotaPolicyVersionRef, RbacPolicyVersionRef, RepositoryBinding, RepositoryRef, RepositoryRole,
    RequirementRef, RoomBinding, RoomBindingRole, TeamRef, WorkerId, WorkerIncarnationId,
};
use agentd_store::SqliteStore;
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

fn workflows_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
}

async fn send(app: Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

async fn get(app: Router, path: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// Mirrors `crates/agentd-bin/tests/native_dispatch.rs::dispatch_authority_snapshot`:
/// a minimal internally-consistent snapshot whose ref matches the runtime
/// session created for the native node, so `pull` can resolve a security scope.
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

/// Serve the worker-facing fleet/runtime routes on a real socket so a real
/// `agentd worker` loop can pull over HTTP. Mirrors
/// `crates/agentd-bin/tests/native_dispatch.rs::serve_dispatch_daemon`.
async fn serve_fleet(store: SqliteStore, token: &str) -> String {
    let fleet = Arc::new(agentd_store::worker_fleet::SqliteWorkerFleet::new(
        store.pool().clone(),
    ));
    let artifacts = Arc::new(
        agentd_store::content_store::LocalContentStore::new(
            std::env::temp_dir().join(format!("agentd-m3-e2e-artifacts-{}", std::process::id())),
        )
        .expect("content store"),
    );
    let service = Arc::new(daemon::WorkerFleetService::new(
        fleet,
        agentd_bin::native_worker::AgentdWorker::new(store.clone()),
        artifacts,
    ));
    let auth = agentd_surface::http::AuthConfig {
        api_token: Some(token.to_string()),
        ..agentd_surface::http::AuthConfig::default()
    };
    let fleet_router = agentd_surface::worker_fleet_http::worker_fleet_router(
        Arc::new(
            agentd_store::worker_fleet::SqliteWorkerFleet::new(store.pool().clone())
                .with_auth_proof(token.to_string()),
        ),
        auth,
    );
    let app = daemon::daemon_native_runtime_router(&store, Some(token.to_string()))
        .merge(daemon::recovery_router(service, token.to_string()))
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

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn agents_register_message_and_run_a_task_graph_with_no_agent_chat_process() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    // `FakeBackend`, not `TmuxBackend`: nothing in this test can launch tmux.
    let host = ProductionRunHost::new(
        store.clone(),
        Box::new(FakeBackend::new()),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    let app = daemon::build_router(Arc::new(host));

    // 1. Register — M3 Plan A. `AgentRegistration` requires only `name`; every
    //    other field is an `Option` and defaults to `None`.
    for name in ["orchestrator", "codex-a"] {
        let (status, body) = send(
            app.clone(),
            "POST",
            "/api/agents",
            json!({
                "name": name,
                "role": "coding",
                "capability": "medium",
                "runtime": "codex",
                "workdir": format!("/tmp/agentd/{name}")
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "register {name}: {body}");
        assert_eq!(body["ok"], true, "register {name}: {body}");
    }
    let (agents_status, agents) = get(app.clone(), "/api/agents").await;
    assert_eq!(agents_status, StatusCode::OK);
    assert!(
        agents.to_string().contains("codex-a"),
        "registry lists the agent: {agents}"
    );

    // 2. Message — M3 Plan B.
    let (message_status, _) = send(
        app.clone(),
        "POST",
        "/api/messages",
        json!({
            "from": "orchestrator",
            "to": "codex-a",
            "summary": "kickoff",
            "full": "starting the graph"
        }),
    )
    .await;
    assert_eq!(message_status, StatusCode::CREATED);
    let (inbox_status, inbox) = get(app.clone(), "/api/inbox/codex-a?drain=false").await;
    assert_eq!(inbox_status, StatusCode::OK);
    assert_eq!(inbox["dm"][0]["summary"], "kickoff");

    // 3. Run a task graph: a messaged node followed by a native node.
    let shim_dir = tempfile::tempdir().expect("shim dir");
    let shim = shim_dir.path().join("codex");
    std::fs::write(&shim, "#!/bin/sh\nexit 0\n").expect("write shim");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let (create_status, created) = send(
        app.clone(),
        "POST",
        "/api/task-graphs",
        json!({
            "id": "graph_m3",
            "owner": "orchestrator",
            "label": "M3 exit gate",
            "nodes": {
                "plan": {"assignee": "codex-a", "description": "Plan the work"},
                "build": {
                    "description": "Build it natively",
                    "depends_on": ["plan"],
                    "execution": {
                        "version": 1,
                        "provider": "codex",
                        "program": shim.to_string_lossy(),
                        "args": [],
                        "cwd": shim_dir.path().to_string_lossy(),
                        "env": []
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(create_status, StatusCode::OK, "body: {created}");
    assert_eq!(created["graph"]["nodes"]["plan"]["status"], "dispatched");
    assert_eq!(created["graph"]["nodes"]["build"]["status"], "pending");
    let plan_message_id = created["graph"]["nodes"]["plan"]["message_id"]
        .as_str()
        .expect("plan dispatch message id")
        .to_string();

    // The assignee reports the first node's result as a normal message.
    let (result_status, result) = send(
        app.clone(),
        "POST",
        "/api/messages",
        json!({
            "from": "codex-a",
            "to": "orchestrator",
            "summary": "planned",
            "full": "planned",
            "reply_to": plan_message_id,
            "schema": {
                "kind": "task_graph_result",
                "version": 1,
                "payload": {"graphId": "graph_m3", "nodeId": "plan", "result": {"ok": true}}
            }
        }),
    )
    .await;
    assert_eq!(result_status, StatusCode::CREATED, "body: {result}");
    assert_eq!(result["taskGraph"]["status"], "complete");
    let (graph_status, graph) = get(app.clone(), "/api/task-graphs/graph_m3").await;
    assert_eq!(graph_status, StatusCode::OK);
    assert_eq!(graph["nodes"]["build"]["status"], "dispatched");
    let execution_task_id = graph["nodes"]["build"]["executionTaskId"]
        .as_str()
        .expect("native node carries its execution task id")
        .to_string();

    // 4. A real native worker pulls and executes the queued node.
    agentd_store::project_authority_repo::record_snapshot(store.pool(), &authority_snapshot())
        .await
        .expect("record project authority snapshot");
    let profile_id = AgentProfileId::new();
    agentd_store::agent_profile_repo::create_profile(
        store.pool(),
        agentd_store::agent_profile_repo::AgentProfileCreate {
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
    worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        WorkerRegistration {
            id: WorkerIncarnationId::new(),
            daemon_version: "0.0.0-m3-plan-c".to_string(),
            host_name: "host-a".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
            capacity: 1,
        },
    )
    .await
    .expect("incarnation");
    let session_id = agentd_core::types::RuntimeSessionId::new();
    agentd_store::runtime_session_repo::create_session(
        store.pool(),
        agentd_store::runtime_session_repo::RuntimeSessionCreate {
            id: session_id.clone(),
            execution_task_id: agentd_core::types::TaskRunId::from_string(
                execution_task_id.clone(),
            ),
            agent_profile_id: profile_id,
            snapshot: agentd_store::runtime_session_repo::ExecutionSnapshotRef {
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

    let base_url = serve_fleet(store.clone(), "worker-secret").await;
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

    // 5. The daemon's maintenance order — reconcile, then settle — closes the
    //    node and therefore the graph.
    let scheduler =
        agentd_store::durable_scheduler::SqliteDurableScheduler::new(store.pool().clone());
    agentd_core::ports::DurableSchedulerPort::reconcile(&scheduler, 1_000)
        .await
        .expect("reconcile");
    let settled =
        agentd_store::agent_chat_task_graph_repo::settle_node_executions(store.pool(), 1_000)
            .await
            .expect("settle");
    assert_eq!(settled, 1);

    let (final_status, final_graph) = get(app, "/api/task-graphs/graph_m3").await;
    assert_eq!(final_status, StatusCode::OK);
    assert_eq!(final_graph["nodes"]["plan"]["status"], "complete");
    assert_eq!(final_graph["nodes"]["build"]["status"], "complete");
    assert_eq!(final_graph["status"], "complete");
}
