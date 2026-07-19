use agentd_core::ports::WorkloadRole;
use agentd_core::ports::{AuthenticatedWorkload, MtlsWorkloadVerifier, WorkerFleetRegisterRequest};
use agentd_core::types::{WorkerId, WorkerIncarnationId};
use agentd_store::SqliteStore;
use agentd_store::worker_fleet::SqliteWorkerFleet;
use agentd_surface::http::AuthConfig;
use agentd_surface::worker_fleet_http::worker_fleet_router;
use agentd_surface::worker_fleet_mtls_http::worker_fleet_mtls_router;
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use http_body_util::BodyExt;
use serde_json::json;
use std::sync::Arc;
use tower::ServiceExt;

struct FixedCertificateVerifier {
    incarnation_id: WorkerIncarnationId,
}

#[async_trait]
impl MtlsWorkloadVerifier for FixedCertificateVerifier {
    async fn verify_peer(
        &self,
        peer_certificate_der: &[u8],
        _observed_at: i64,
    ) -> Result<AuthenticatedWorkload, agentd_core::ports::SecurityDenial> {
        if peer_certificate_der != b"certificate" {
            return Err(agentd_core::ports::SecurityDenial::IdentityRevoked);
        }
        Ok(AuthenticatedWorkload {
            spiffe_id: "spiffe://test/worker/fixture".into(),
            role: WorkloadRole::Worker,
            trust_domain: "test".into(),
            certificate_fingerprint: "fixture".into(),
            valid_from: 0,
            valid_until: i64::MAX,
            worker_incarnation_id: Some(self.incarnation_id.clone()),
        })
    }
}

#[tokio::test]
async fn worker_fleet_http_registers_with_auth_and_pulls_empty_queue() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = Arc::new(SqliteWorkerFleet::new(store.pool().clone()).with_auth_proof("secret"));
    let mut auth = AuthConfig::open();
    auth.api_token = Some("operator-secret".into());
    let app = worker_fleet_router(fleet, auth);
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
    let unauthorized_response = app
        .clone()
        .oneshot(
            Request::post("/api/worker-fleet/register")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthorized_response.status(), StatusCode::UNAUTHORIZED);
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/worker-fleet/register")
                .header("content-type", "application/json")
                .header("authorization", "Bearer operator-secret")
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
                .header("authorization", "Bearer operator-secret")
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

#[tokio::test]
async fn worker_fleet_mtls_binds_certificate_identity_to_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = Arc::new(SqliteWorkerFleet::new(store.pool().clone()).with_auth_proof("secret"));
    let incarnation_id = WorkerIncarnationId::new();
    let verifier = Arc::new(FixedCertificateVerifier {
        incarnation_id: incarnation_id.clone(),
    });
    let app = worker_fleet_mtls_router(fleet, verifier);
    let request = WorkerFleetRegisterRequest {
        auth_proof: "secret".into(),
        worker_id: WorkerId::new(),
        trust_domain: "test".into(),
        labels: json!({}),
        incarnation_id: incarnation_id.clone(),
        daemon_version: "test".into(),
        host_name: "host".into(),
        network_zone: None,
        capabilities: json!({"runtime": ["native"]}),
    };
    let certificate = STANDARD.encode(b"certificate");
    let mut mismatched_request = request.clone();
    mismatched_request.incarnation_id = WorkerIncarnationId::new();
    let response = app
        .clone()
        .oneshot(
            Request::post("/api/worker-fleet/mtls/register")
                .header("content-type", "application/json")
                .header("x-client-certificate-der", certificate.clone())
                .body(Body::from(
                    serde_json::to_vec(&mismatched_request).expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = app
        .oneshot(
            Request::post("/api/worker-fleet/mtls/register")
                .header("content-type", "application/json")
                .header("x-client-certificate-der", certificate)
                .body(Body::from(serde_json::to_vec(&request).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
}
