mod support;

use std::time::Duration;

use agentd_core::ports::{
    ProjectAuthorityAvailability, ProjectAuthorityError, ProjectAuthorityHealth,
    ProjectAuthorityMode,
};
use agentd_core::types::OfflineRecoveryPolicy;
use agentd_project_authority::{HttpSpecifyAuthorityTransport, SpecifyAuthorityTransport};
use support::{authority, resolve_request, snapshot};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn http_specify_transport_authenticates_and_bounds_all_operations() {
    let server = MockServer::start().await;
    let authority_key = authority("specify:corp");
    let expected = snapshot(
        authority_key.clone(),
        "project-1",
        "snapshot-1",
        OfflineRecoveryPolicy::Deny,
    );
    Mock::given(method("POST"))
        .and(path("/v1/project-authority/resolve"))
        .and(header("authorization", "Bearer workload-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&expected))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(
            "/v1/project-authority/snapshots/specify:corp/snapshot-1/9",
        ))
        .and(header("authorization", "Bearer workload-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&expected))
        .mount(&server)
        .await;
    let health = ProjectAuthorityHealth {
        authority_key,
        mode: ProjectAuthorityMode::Specify,
        availability: ProjectAuthorityAvailability::Available,
        checked_at: 200,
        authority_revision: Some(9),
    };
    Mock::given(method("GET"))
        .and(path("/v1/project-authority/health"))
        .and(header("authorization", "Bearer workload-token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&health))
        .mount(&server)
        .await;
    let transport = HttpSpecifyAuthorityTransport::new(
        &server.uri(),
        "Bearer workload-token",
        Duration::from_secs(2),
        true,
    )
    .unwrap();
    assert_eq!(
        expected,
        transport
            .resolve(&resolve_request(&expected))
            .await
            .unwrap()
    );
    assert_eq!(
        expected,
        transport.refresh(&expected.snapshot_ref).await.unwrap()
    );
    assert_eq!(health, transport.health().await.unwrap());
}

#[tokio::test]
async fn http_specify_transport_rejects_insecure_remote_and_oversized_response() {
    assert!(matches!(
        HttpSpecifyAuthorityTransport::new(
            "http://specify.example/v1",
            "Bearer token",
            Duration::from_secs(1),
            true,
        ),
        Err(ProjectAuthorityError::Invalid(_))
    ));
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/project-authority/health"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'x'; 2 * 1024 * 1024 + 1]))
        .mount(&server)
        .await;
    let transport = HttpSpecifyAuthorityTransport::new(
        &server.uri(),
        "Bearer workload-token",
        Duration::from_secs(2),
        true,
    )
    .unwrap();
    assert!(matches!(
        transport.health().await,
        Err(ProjectAuthorityError::Unverifiable(_))
    ));
}
