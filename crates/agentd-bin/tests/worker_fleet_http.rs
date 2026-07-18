use agentd_core::ports::WorkerFleetRegisterRequest;
use agentd_core::types::{WorkerId, WorkerIncarnationId};
use agentd_store::SqliteStore;
use agentd_store::worker_fleet::SqliteWorkerFleet;
use agentd_surface::worker_fleet_http::worker_fleet_router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

#[tokio::test]
async fn worker_fleet_http_registers_with_auth_and_pulls_empty_queue() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = Arc::new(SqliteWorkerFleet::new(store.pool().clone()).with_auth_proof("secret"));
    let app = worker_fleet_router(fleet);
    let request = WorkerFleetRegisterRequest {
        auth_proof: "secret".into(),
        worker_id: WorkerId::new(),
        trust_domain: "local".into(),
        labels: json!({}),
        incarnation_id: WorkerIncarnationId::new(),
        daemon_version: "test".into(),
        host_name: "host".into(),
        network_zone: None,
        capabilities: json!({"runtime": ["native"]}),
    };
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/worker-fleet/register")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let pull = json!({
        "auth_proof": "secret",
        "worker_incarnation_id": request.incarnation_id,
        "observed_at": 10,
        "expires_at": 20
    });
    let response = app
        .oneshot(
            Request::post("/api/worker-fleet/pull")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&pull).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).expect("value"),
        json!(null)
    );
}
