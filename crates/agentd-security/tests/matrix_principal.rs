use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use agentd_core::ports::Clock;
use agentd_core::types::{
    AuthorityKey, EnterprisePrincipalId, MatrixDeviceBinding, MatrixDeviceStatus,
    MatrixPrincipalResolveRequest, MatrixTrustPolicy, OrganizationRef, PrincipalKind,
    SecurityDenialReason,
};
use agentd_security::matrix_principal::{MatrixPrincipalResolver, MatrixPrincipalResolverConfig};
use agentd_store::SqliteStore;
use agentd_store::principal_repo::{
    MatrixAppserviceBinding, MatrixUserBinding, PrincipalUpsert,
    SqliteEnterprisePrincipalRepository,
};

#[derive(Debug)]
struct TestClock(AtomicI64);

impl TestClock {
    fn new(now: i64) -> Self {
        Self(AtomicI64::new(now))
    }

    fn set(&self, now: i64) {
        self.0.store(now, Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now_unix(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

async fn fixture() -> (
    tempfile::TempDir,
    Arc<SqliteEnterprisePrincipalRepository>,
    MatrixTrustPolicy,
    EnterprisePrincipalId,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let policy = MatrixTrustPolicy {
        trusted_homeservers: BTreeSet::from(["matrix.example".to_string()]),
        trusted_appservices: BTreeSet::from(["agentd-gateway".to_string()]),
    };
    let repo = Arc::new(
        SqliteEnterprisePrincipalRepository::new(store.pool().clone(), policy.clone(), 300)
            .expect("repository"),
    );
    let id = EnterprisePrincipalId::new();
    let authority = AuthorityKey::new("specify:matrix-principal-test").expect("authority");
    repo.upsert_principal(PrincipalUpsert {
        id: id.clone(),
        organization_ref: OrganizationRef::new(authority, "org-a", "4").expect("organization"),
        kind: PrincipalKind::Human,
        display_name: "Alex".to_string(),
        observed_at: 100,
    })
    .await
    .expect("principal");
    repo.bind_matrix_user(MatrixUserBinding {
        user_id: "@alex:matrix.example".to_string(),
        homeserver: "matrix.example".to_string(),
        principal_id: id.clone(),
        bound_at: 101,
    })
    .await
    .expect("user binding");
    repo.bind_matrix_device(MatrixDeviceBinding {
        principal_id: id.clone(),
        user_id: "@alex:matrix.example".to_string(),
        device_id: "DEVICE-A".to_string(),
        status: MatrixDeviceStatus::Current,
        bound_at: 102,
        revoked_at: None,
    })
    .await
    .expect("device binding");
    (dir, repo, policy, id)
}

#[tokio::test]
async fn matrix_resolver_requires_trusted_homeserver_and_human_device() {
    let (_dir, repo, policy, id) = fixture().await;
    let resolver = MatrixPrincipalResolver::new(
        Arc::clone(&repo),
        Arc::new(TestClock::new(120)),
        MatrixPrincipalResolverConfig {
            trust_policy: policy,
        },
    );
    let accepted = resolver
        .resolve(&MatrixPrincipalResolveRequest {
            user_id: "@alex:matrix.example".to_string(),
            homeserver: "matrix.example".to_string(),
            device_id: Some("DEVICE-A".to_string()),
            appservice_id: None,
            observed_at: -1,
        })
        .await
        .expect("trusted human identity");
    assert_eq!(accepted.principal.id, id);

    let missing_device = resolver
        .resolve(&MatrixPrincipalResolveRequest {
            user_id: "@alex:matrix.example".to_string(),
            homeserver: "matrix.example".to_string(),
            device_id: None,
            appservice_id: None,
            observed_at: 121,
        })
        .await
        .expect_err("human device is required");
    assert_eq!(
        missing_device.denial_reason(),
        Some(SecurityDenialReason::MatrixDeviceRequired)
    );

    let foreign = resolver
        .resolve(&MatrixPrincipalResolveRequest {
            user_id: "@alex:foreign.example".to_string(),
            homeserver: "foreign.example".to_string(),
            device_id: Some("DEVICE-A".to_string()),
            appservice_id: None,
            observed_at: 122,
        })
        .await
        .expect_err("foreign homeserver");
    assert_eq!(
        foreign.denial_reason(),
        Some(SecurityDenialReason::MatrixHomeserverUntrusted)
    );
}

#[tokio::test]
async fn matrix_resolver_allows_trusted_appservice_without_device_and_propagates_disablement() {
    let (_dir, repo, policy, _human_id) = fixture().await;
    let clock = Arc::new(TestClock::new(120));
    let id = EnterprisePrincipalId::new();
    let authority = AuthorityKey::new("specify:matrix-principal-test").expect("authority");
    repo.upsert_principal(PrincipalUpsert {
        id: id.clone(),
        organization_ref: OrganizationRef::new(authority, "org-a", "4").expect("organization"),
        kind: PrincipalKind::Service,
        display_name: "Agentd Gateway".to_string(),
        observed_at: 103,
    })
    .await
    .expect("service principal");
    repo.bind_matrix_appservice(MatrixAppserviceBinding {
        appservice_id: "agentd-gateway".to_string(),
        homeserver: "matrix.example".to_string(),
        sender_localpart_prefix: "ac_".to_string(),
        principal_id: id.clone(),
        bound_at: 104,
    })
    .await
    .expect("appservice binding");
    let resolver = MatrixPrincipalResolver::new(
        Arc::clone(&repo),
        Arc::clone(&clock),
        MatrixPrincipalResolverConfig {
            trust_policy: policy,
        },
    );
    let accepted = resolver
        .resolve(&MatrixPrincipalResolveRequest {
            user_id: "@ac_worker:matrix.example".to_string(),
            homeserver: "matrix.example".to_string(),
            device_id: None,
            appservice_id: Some("agentd-gateway".to_string()),
            observed_at: 120,
        })
        .await
        .expect("trusted appservice");
    assert_eq!(accepted.principal.id, id);

    repo.disable_principal(&id, 130)
        .await
        .expect("disable principal");
    clock.set(131);
    let denied = resolver
        .resolve(&MatrixPrincipalResolveRequest {
            user_id: "@ac_worker:matrix.example".to_string(),
            homeserver: "matrix.example".to_string(),
            device_id: None,
            appservice_id: Some("agentd-gateway".to_string()),
            observed_at: 131,
        })
        .await
        .expect_err("disabled principal");
    assert_eq!(
        denied.denial_reason(),
        Some(SecurityDenialReason::PrincipalDisabled)
    );
}
