use std::collections::BTreeSet;

use agentd_core::ports::{EnterprisePrincipalPort, SecurityError};
use agentd_core::types::{
    AuthorityKey, EnterprisePrincipalId, MatrixDeviceBinding, MatrixDeviceStatus,
    MatrixPrincipalResolveRequest, MatrixTrustPolicy, OidcPrincipalResolveRequest, OrganizationRef,
    PrincipalKind, PrincipalStatus, SecurityDenialReason,
};
use agentd_store::principal_repo::{
    MatrixAppserviceBinding, MatrixUserBinding, OidcSubjectBinding, PrincipalUpsert,
    SqliteEnterprisePrincipalRepository,
};
use agentd_store::{SqliteStore, StoreError};
use sqlx::Row;

async fn store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    (store, dir)
}

fn authority_key() -> AuthorityKey {
    AuthorityKey::new("specify:principal-store-test").expect("authority key")
}

fn organization(id: &str) -> OrganizationRef {
    OrganizationRef::new(authority_key(), id, "4").expect("organization ref")
}

fn trust_policy() -> MatrixTrustPolicy {
    MatrixTrustPolicy {
        trusted_homeservers: BTreeSet::from(["matrix.example".to_string()]),
        trusted_appservices: BTreeSet::from(["agentd-gateway".to_string()]),
    }
}

fn principal(id: EnterprisePrincipalId, name: &str, kind: PrincipalKind) -> PrincipalUpsert {
    PrincipalUpsert {
        id,
        organization_ref: organization("org-a"),
        kind,
        display_name: name.to_string(),
        observed_at: 100,
    }
}

fn repository(store: &SqliteStore) -> SqliteEnterprisePrincipalRepository {
    SqliteEnterprisePrincipalRepository::new(store.pool().clone(), trust_policy(), 300)
        .expect("principal repository")
}

#[tokio::test]
async fn oidc_principal_lifecycle_is_unique_and_fail_closed() {
    let (store, _dir) = store().await;
    let repo = repository(&store);
    let id = EnterprisePrincipalId::new();
    let created = repo
        .upsert_principal(principal(id.clone(), "Alex", PrincipalKind::Human))
        .await
        .expect("upsert principal");
    assert_eq!(created.status, PrincipalStatus::Active);

    repo.bind_oidc_subject(OidcSubjectBinding {
        issuer: "https://identity.example".to_string(),
        subject: "subject-a".to_string(),
        principal_id: id.clone(),
        bound_at: 110,
    })
    .await
    .expect("bind OIDC subject");

    let resolved = repo
        .resolve_oidc(&OidcPrincipalResolveRequest {
            issuer: "https://identity.example".to_string(),
            subject: "subject-a".to_string(),
            observed_at: 120,
        })
        .await
        .expect("resolve OIDC subject");
    assert_eq!(resolved.principal.id, id);
    assert_eq!(resolved.expires_at, 420);

    let other_id = EnterprisePrincipalId::new();
    repo.upsert_principal(principal(other_id.clone(), "Other", PrincipalKind::Human))
        .await
        .expect("upsert second principal");
    let conflict = repo
        .bind_oidc_subject(OidcSubjectBinding {
            issuer: "https://identity.example".to_string(),
            subject: "subject-a".to_string(),
            principal_id: other_id,
            bound_at: 121,
        })
        .await
        .expect_err("OIDC identity cannot map to two principals");
    assert!(matches!(conflict, StoreError::Conflict(_)), "{conflict:?}");

    repo.disable_principal(&id, 130)
        .await
        .expect("disable principal");
    let denied = repo
        .resolve_oidc(&OidcPrincipalResolveRequest {
            issuer: "https://identity.example".to_string(),
            subject: "subject-a".to_string(),
            observed_at: 131,
        })
        .await
        .expect_err("disabled principal must be denied");
    assert_eq!(
        denied,
        SecurityError::Denied(SecurityDenialReason::PrincipalDisabled)
    );
}

#[tokio::test]
async fn matrix_user_and_device_lifecycle_fail_closed() {
    let (store, _dir) = store().await;
    let repo = repository(&store);
    let id = EnterprisePrincipalId::new();
    repo.upsert_principal(principal(id.clone(), "Alex", PrincipalKind::Human))
        .await
        .expect("upsert principal");
    repo.bind_matrix_user(MatrixUserBinding {
        user_id: "@alex:matrix.example".to_string(),
        homeserver: "matrix.example".to_string(),
        principal_id: id.clone(),
        bound_at: 105,
    })
    .await
    .expect("bind Matrix user");
    repo.bind_matrix_device(MatrixDeviceBinding {
        principal_id: id.clone(),
        user_id: "@alex:matrix.example".to_string(),
        device_id: "DEVICE-A".to_string(),
        status: MatrixDeviceStatus::Current,
        bound_at: 106,
        revoked_at: None,
    })
    .await
    .expect("bind Matrix device");

    let request = MatrixPrincipalResolveRequest {
        user_id: "@alex:matrix.example".to_string(),
        homeserver: "matrix.example".to_string(),
        device_id: Some("DEVICE-A".to_string()),
        appservice_id: None,
        observed_at: 120,
    };
    assert_eq!(
        repo.resolve_matrix(&request)
            .await
            .expect("resolve Matrix user")
            .principal
            .id,
        id
    );

    repo.revoke_matrix_device("@alex:matrix.example", "DEVICE-A", 125)
        .await
        .expect("revoke device");
    assert_eq!(
        repo.resolve_matrix(&request)
            .await
            .expect_err("revoked device must be denied"),
        SecurityError::Denied(SecurityDenialReason::MatrixDeviceRevoked)
    );

    repo.disable_matrix_user("@alex:matrix.example", 126)
        .await
        .expect("disable Matrix user");
    let no_device = MatrixPrincipalResolveRequest {
        device_id: None,
        observed_at: 127,
        ..request.clone()
    };
    assert_eq!(
        repo.resolve_matrix(&no_device)
            .await
            .expect_err("disabled Matrix user must be denied"),
        SecurityError::Denied(SecurityDenialReason::MatrixUserDisabled)
    );

    let foreign = MatrixPrincipalResolveRequest {
        user_id: "@alex:foreign.example".to_string(),
        homeserver: "foreign.example".to_string(),
        device_id: None,
        appservice_id: None,
        observed_at: 128,
    };
    assert_eq!(
        repo.resolve_matrix(&foreign)
            .await
            .expect_err("foreign homeserver must be denied"),
        SecurityError::Denied(SecurityDenialReason::MatrixHomeserverUntrusted)
    );
}

#[tokio::test]
async fn matrix_appservice_requires_trusted_namespace() {
    let (store, _dir) = store().await;
    let repo = repository(&store);
    let id = EnterprisePrincipalId::new();
    repo.upsert_principal(principal(
        id.clone(),
        "Agentd Gateway",
        PrincipalKind::Service,
    ))
    .await
    .expect("upsert service principal");
    repo.bind_matrix_appservice(MatrixAppserviceBinding {
        appservice_id: "agentd-gateway".to_string(),
        homeserver: "matrix.example".to_string(),
        sender_localpart_prefix: "ac_".to_string(),
        principal_id: id.clone(),
        bound_at: 110,
    })
    .await
    .expect("bind appservice");

    let accepted = repo
        .resolve_matrix(&MatrixPrincipalResolveRequest {
            user_id: "@ac_worker:matrix.example".to_string(),
            homeserver: "matrix.example".to_string(),
            device_id: None,
            appservice_id: Some("agentd-gateway".to_string()),
            observed_at: 120,
        })
        .await
        .expect("trusted appservice sender");
    assert_eq!(accepted.principal.id, id);

    let denied = repo
        .resolve_matrix(&MatrixPrincipalResolveRequest {
            user_id: "@human:matrix.example".to_string(),
            homeserver: "matrix.example".to_string(),
            device_id: None,
            appservice_id: Some("agentd-gateway".to_string()),
            observed_at: 121,
        })
        .await
        .expect_err("foreign appservice namespace must be denied");
    assert_eq!(
        denied,
        SecurityError::Denied(SecurityDenialReason::MatrixAppserviceUntrusted)
    );

    repo.disable_matrix_appservice("agentd-gateway", "matrix.example", 130)
        .await
        .expect("disable appservice");
    let disabled = repo
        .resolve_matrix(&MatrixPrincipalResolveRequest {
            user_id: "@ac_worker:matrix.example".to_string(),
            homeserver: "matrix.example".to_string(),
            device_id: None,
            appservice_id: Some("agentd-gateway".to_string()),
            observed_at: 131,
        })
        .await
        .expect_err("disabled appservice must be denied");
    assert_eq!(
        disabled,
        SecurityError::Denied(SecurityDenialReason::MatrixAppserviceUntrusted)
    );
}

#[tokio::test]
async fn principal_schema_contains_no_credentials_or_policy_authority_tables() {
    let (store, _dir) = store().await;
    let table_names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'enterprise_%' \
         OR type = 'table' AND name LIKE 'oidc_%' \
         OR type = 'table' AND name LIKE 'matrix_principal_%' \
         ORDER BY name",
    )
    .fetch_all(store.pool())
    .await
    .expect("principal tables");
    assert_eq!(
        table_names,
        vec![
            "enterprise_principals",
            "matrix_principal_appservices",
            "matrix_principal_devices",
            "matrix_principal_users",
            "oidc_principal_bindings",
        ]
    );

    for table in table_names {
        let rows = sqlx::query(&format!("PRAGMA table_info({table})"))
            .fetch_all(store.pool())
            .await
            .expect("table columns");
        let columns = rows
            .iter()
            .map(|row| row.get::<String, _>("name"))
            .collect::<Vec<_>>();
        for forbidden in ["token", "jwt", "secret", "private_key", "device_key"] {
            assert!(
                columns.iter().all(|column| !column.contains(forbidden)),
                "{table} contains forbidden credential column matching {forbidden}: {columns:?}"
            );
        }
    }
}
