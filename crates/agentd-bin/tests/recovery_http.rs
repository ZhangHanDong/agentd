use agentd_bin::daemon::{WorkerFleetService, recovery_router};
use agentd_bin::native_worker::AgentdWorker;
use agentd_core::types::{RuntimeSessionId, WorkerIncarnationId};
use agentd_store::SqliteStore;
use agentd_store::worker_fleet::SqliteWorkerFleet;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn recovery_http_requires_operator_token_and_accepts_codex_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = Arc::new(SqliteWorkerFleet::new(store.pool().clone()));
    let service = Arc::new(WorkerFleetService::new(fleet, AgentdWorker::new(store)));
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
