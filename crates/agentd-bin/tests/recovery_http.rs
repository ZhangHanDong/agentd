use agentd_bin::daemon::{WorkerFleetService, recovery_router};
use agentd_bin::native_worker::AgentdWorker;
use agentd_core::types::{RuntimeSessionId, WorkerIncarnationId};
use agentd_store::SqliteStore;
use agentd_store::content_store::LocalContentStore;
use agentd_store::worker_fleet::SqliteWorkerFleet;
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
