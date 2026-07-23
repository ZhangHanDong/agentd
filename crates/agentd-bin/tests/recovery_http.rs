use agentd_bin::daemon::{WorkerFleetService, recovery_router};
use agentd_bin::native_worker::AgentdWorker;
use agentd_core::ports::{TaskLeaseDispatchRequest, TaskLeasePort};
use agentd_core::types::{NodeId, RunId, RuntimeSessionId, WorkerId, WorkerIncarnationId};
use agentd_store::SqliteStore;
use agentd_store::content_store::LocalContentStore;
use agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane;
use agentd_store::worker_fleet::SqliteWorkerFleet;
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{run_repo, task_repo};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn recovery_http_requires_operator_token_and_accepts_codex_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = Arc::new(SqliteWorkerFleet::new(store.pool().clone()));
    let artifacts =
        Arc::new(LocalContentStore::new(dir.path().join("artifacts")).expect("content store"));
    let service = Arc::new(WorkerFleetService::new(
        fleet,
        AgentdWorker::new(store),
        artifacts,
    ));
    let app = recovery_router(service, "operator-secret".into());
    let body = json!({
        "session_id": RuntimeSessionId::new(),
        "worker_incarnation_id": WorkerIncarnationId::new(),
        "cwd": "/tmp",
        "env": []
    });

    let unauthorized = app
        .clone()
        .oneshot(
            Request::post("/api/runtime/recover")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let capabilities = app
        .clone()
        .oneshot(
            Request::get("/api/runtime/capabilities")
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(capabilities.status(), StatusCode::OK);
    let capabilities_body = capabilities
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let capabilities_json: serde_json::Value =
        serde_json::from_slice(&capabilities_body).expect("capabilities json");
    assert_eq!(capabilities_json["runtime"], "native");
    assert_eq!(capabilities_json["runtimeApiVersion"], 1);
    assert_eq!(capabilities_json["workerProtocol"], "http-or-mtls");
    assert_eq!(capabilities_json["sessionResume"], true);

    let inventory = app
        .clone()
        .oneshot(
            Request::get("/api/cutover/inventory")
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(inventory.status(), StatusCode::OK);
    let inventory_body = inventory
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let inventory_json: serde_json::Value =
        serde_json::from_slice(&inventory_body).expect("inventory json");
    assert!(inventory_json["captured_at"].is_number());
    assert!(inventory_json["workers"].is_object());
    assert!(inventory_json["ready_for_cutover"].is_boolean());
    assert_eq!(inventory_json["rollback_requires_new_lease_epoch"], true);

    let artifact_unauthorized = app
        .clone()
        .oneshot(
            Request::get("/api/runtime/runs/run_missing/artifacts")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(artifact_unauthorized.status(), StatusCode::UNAUTHORIZED);

    let artifact_invalid = app
        .clone()
        .oneshot(
            Request::get("/api/runtime/runs/not-a-valid-run/artifacts")
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(artifact_invalid.status(), StatusCode::OK);
    let artifact_body = artifact_invalid
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let artifact_json: serde_json::Value =
        serde_json::from_slice(&artifact_body).expect("artifact page json");
    assert_eq!(artifact_json["records"], serde_json::json!([]));

    let missing_cutover = app
        .clone()
        .oneshot(
            Request::get("/api/cutover/projects/project-alpha")
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(missing_cutover.status(), StatusCode::NOT_FOUND);

    let transition_body = json!({
        "phase": "observe",
        "authority_revision": "authority-r1",
        "matrix_cursor": 3,
        "lease_epoch": 1
    });
    let created_cutover = app
        .clone()
        .oneshot(
            Request::post("/api/cutover/projects/project-alpha/transition")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&transition_body).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(created_cutover.status(), StatusCode::OK);

    let shadow_body = json!({
        "phase": "shadow",
        "authority_revision": "authority-r1",
        "matrix_cursor": 4,
        "lease_epoch": 1
    });
    let shadow = app
        .clone()
        .oneshot(
            Request::post("/api/cutover/projects/project-alpha/transition")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&shadow_body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(shadow.status(), StatusCode::OK);

    let canary_body = json!({
        "phase": "canary",
        "authority_revision": "authority-r1",
        "matrix_cursor": 5,
        "lease_epoch": 1
    });
    let canary = app
        .clone()
        .oneshot(
            Request::post("/api/cutover/projects/project-alpha/transition")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&canary_body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(canary.status(), StatusCode::OK);

    let promote_body = json!({
        "phase": "cutover",
        "authority_revision": "authority-r1",
        "matrix_cursor": 5,
        "lease_epoch": 1
    });
    let promoted = app
        .clone()
        .oneshot(
            Request::post("/api/cutover/projects/project-alpha/promote")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&promote_body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(promoted.status(), StatusCode::OK);

    let invalid_backtrack = app
        .clone()
        .oneshot(
            Request::post("/api/cutover/projects/project-alpha/transition")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&transition_body).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(invalid_backtrack.status(), StatusCode::CONFLICT);

    let accepted = app
        .oneshot(
            Request::post("/api/runtime/recover")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(accepted.status(), StatusCode::ACCEPTED);
    let bytes = accepted
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).expect("value"),
        json!({"accepted": true})
    );
}

#[tokio::test]
async fn recovery_http_uploads_artifact_bytes_content_addressed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = Arc::new(SqliteWorkerFleet::new(store.pool().clone()));
    let artifacts =
        Arc::new(LocalContentStore::new(dir.path().join("artifacts")).expect("content store"));
    let service = Arc::new(WorkerFleetService::new(
        fleet,
        AgentdWorker::new(store),
        artifacts,
    ));
    let app = recovery_router(service, "operator-secret".into());

    let unauthorized = app
        .clone()
        .oneshot(
            Request::post("/api/runtime/artifacts/upload")
                .header("content-type", "application/octet-stream")
                .body(Body::from("transcript bytes"))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let uploaded = app
        .clone()
        .oneshot(
            Request::post("/api/runtime/artifacts/upload")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/octet-stream")
                .body(Body::from("transcript bytes"))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(uploaded.status(), StatusCode::OK);
    let body = uploaded
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let stored: serde_json::Value = serde_json::from_slice(&body).expect("json");
    // sha256 of "transcript bytes"
    assert_eq!(
        stored["content_sha256"],
        "0b16479830f2fa8ca03e4152b626647d04f18007eeb93fa65b812d4cb035524f"
    );
    assert_eq!(stored["size_bytes"], 16);
    assert!(
        stored["storage_ref"]
            .as_str()
            .is_some_and(|r| !r.is_empty()),
        "{stored}"
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn recovery_http_acknowledges_worker_artifact_under_fenced_lease() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");

    let run_id = RunId::new();
    run_repo::insert_run(store.pool(), &run_id, "workflow-sha")
        .await
        .expect("run");
    let task_id = task_repo::insert_task_run(store.pool(), &run_id, &NodeId::parsed("impl"))
        .await
        .expect("task");
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
        WorkerRegistration {
            id: incarnation_id.clone(),
            daemon_version: "0.0.0-p272".to_string(),
            host_name: "host-a".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
        },
    )
    .await
    .expect("incarnation");

    let lease_port = SqliteTaskLeaseControlPlane::new(store.pool().clone());
    let grant = lease_port
        .dispatch(&TaskLeaseDispatchRequest {
            execution_task_id: task_id.clone(),
            worker_incarnation_id: incarnation_id.clone(),
            observed_at: 100,
            expires_at: 10_000,
        })
        .await
        .expect("dispatch");

    let fleet = Arc::new(SqliteWorkerFleet::new(store.pool().clone()));
    let artifacts =
        Arc::new(LocalContentStore::new(dir.path().join("artifacts")).expect("content store"));
    let service = Arc::new(WorkerFleetService::new(
        fleet,
        AgentdWorker::new(store),
        artifacts,
    ));
    let app = recovery_router(service, "operator-secret".into());

    let report = agentd_core::ports::WorkerArtifactReport {
        claim: grant.claim(),
        observed_at: grant.acquired_at + 1,
        artifact: agentd_core::ports::ExecutionArtifactPublish {
            id: agentd_core::types::ExecutionArtifactId::new(),
            kind: agentd_core::ports::ExecutionArtifactKind::Transcript,
            content_sha256: "a".repeat(64),
            size_bytes: 16,
            media_type: "text/plain".to_string(),
            storage_ref: "local://test".to_string(),
            provenance: serde_json::json!({"source": "test"}),
            links: agentd_core::ports::ExecutionEvidenceLinks {
                execution_run_id: run_id.clone(),
                execution_task_id: Some(task_id.clone()),
                runtime_session_id: None,
                runtime_attempt_id: None,
                worker_incarnation_id: Some(incarnation_id.clone()),
                snapshot: agentd_core::ports::ExecutionSnapshotLink {
                    authority_key: "specify:corp".to_string(),
                    resource_kind: "execution_snapshot".to_string(),
                    resource_id: "snapshot-1".to_string(),
                    resource_version: "1".to_string(),
                    content_sha256: "a".repeat(64),
                },
                target_repository_id: "repo_test".to_string(),
                target_base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
            },
        },
    };

    let acked = app
        .clone()
        .oneshot(
            Request::post("/api/runtime/artifacts/acknowledge")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&report).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(acked.status(), StatusCode::OK);

    // Duplicate acknowledge with the identical artifact id replays idempotently.
    let replay = app
        .clone()
        .oneshot(
            Request::post("/api/runtime/artifacts/acknowledge")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&report).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(replay.status(), StatusCode::OK);

    // A stale claim (wrong fencing token) is a terminal 409.
    let mut stale = report.clone();
    stale.claim.fencing_token = agentd_core::types::FencingToken::new(999).expect("token");
    let rejected = app
        .oneshot(
            Request::post("/api/runtime/artifacts/acknowledge")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&stale).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
}
