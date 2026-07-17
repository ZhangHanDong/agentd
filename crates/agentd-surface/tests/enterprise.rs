use std::collections::BTreeMap;
use std::sync::Arc;

use agentd_core::ports::EnterpriseOperationalSnapshot;
use agentd_surface::http::{
    AgentTokenMode, AppState, AuthConfig, MediaConfig, SchedulerConfig, router,
};
use agentd_surface::test_support::FakeRunHost;
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

fn app(host: Arc<FakeRunHost>) -> Router {
    router(AppState {
        host,
        auth: AuthConfig {
            api_token: Some("operator-secret".to_string()),
            agent_token_mode: AgentTokenMode::Hard,
            agent_tokens: BTreeMap::new(),
        },
        media: MediaConfig::default_for_tests(),
        scheduler: SchedulerConfig::default(),
    })
}

fn snapshot() -> EnterpriseOperationalSnapshot {
    EnterpriseOperationalSnapshot {
        observed_at: 1_784_000_000,
        leadership: None,
        members: Vec::new(),
        zones: Vec::new(),
        queued_tasks: 23,
        acquired_tasks: 7,
        dead_letter_tasks: 1,
        active_rollouts: 1,
        degraded_rollouts: 0,
        pending_replica_regions: 2,
        active_legal_holds: 3,
        service_level_warnings: 1,
        service_level_breaches: 0,
        latest_dr_checkpoint: None,
        load_model: None,
    }
}

#[tokio::test]
async fn enterprise_status_requires_operator_and_returns_bounded_snapshot() {
    let host = Arc::new(FakeRunHost::new());
    host.set_enterprise_snapshot(snapshot());
    let unauthorized = app(Arc::clone(&host))
        .oneshot(
            Request::get("/api/enterprise/status")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let authorized = app(host)
        .oneshot(
            Request::get("/api/enterprise/status")
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(authorized.status(), StatusCode::OK);
}

#[tokio::test]
async fn enterprise_explain_returns_not_found_for_unknown_durable_task() {
    let response = app(Arc::new(FakeRunHost::new()))
        .oneshot(
            Request::get("/api/enterprise/tasks/tr_01J00000000000000000000000/explain")
                .header("authorization", "Bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn enterprise_mutation_is_authenticated_and_typed_before_host_dispatch() {
    let host = Arc::new(FakeRunHost::new());
    let body = serde_json::json!({
        "tenant_scope_sha256": "a".repeat(64),
        "policy_version_sha256": "b".repeat(64),
        "artifact_retention_seconds": 86400,
        "transcript_retention_seconds": 86400,
        "audit_retention_seconds": 86400,
        "minimum_replica_regions": 2,
        "updated_at": 1_784_000_000
    });
    let response = app(Arc::clone(&host))
        .oneshot(
            Request::post("/api/enterprise/mutations/set-retention-policy")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(host.enterprise_mutation_count(), 1);
}
