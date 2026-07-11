mod support;

use agentd_core::ports::{
    ProjectAuthorityAvailability, ProjectAuthorityError, ProjectAuthorityMode, ProjectAuthorityPort,
};
use agentd_core::types::OfflineRecoveryPolicy;
use agentd_project_authority::LocalProjectAuthority;

use support::{authority, resolve_request, snapshot};

#[tokio::test]
async fn local_project_authority_resolves_refreshes_and_reports_health() {
    let authority_key = authority("local:standalone");
    let expected = snapshot(
        authority_key.clone(),
        "project-1",
        "snapshot-1",
        OfflineRecoveryPolicy::Deny,
    );
    let adapter = LocalProjectAuthority::new(authority_key.clone(), vec![expected.clone()], 200)
        .expect("local adapter");

    let resolved = adapter
        .resolve(&resolve_request(&expected))
        .await
        .expect("resolve");
    assert_eq!(resolved, expected);
    assert_eq!(
        adapter
            .refresh(&resolved.snapshot_ref)
            .await
            .expect("refresh"),
        resolved
    );
    let health = adapter.health().await.expect("health");
    assert_eq!(health.authority_key, authority_key);
    assert_eq!(health.mode, ProjectAuthorityMode::Local);
    assert_eq!(health.availability, ProjectAuthorityAvailability::Available);
    assert_eq!(health.checked_at, 200);
    assert_eq!(health.authority_revision, Some(9));
}

#[test]
fn local_project_authority_rejects_ambiguous_or_mismatched_configuration() {
    let local = authority("local:standalone");
    let first = snapshot(
        local.clone(),
        "project-1",
        "snapshot-1",
        OfflineRecoveryPolicy::Deny,
    );
    let second = snapshot(
        local.clone(),
        "project-1",
        "snapshot-2",
        OfflineRecoveryPolicy::Deny,
    );
    let ambiguous = LocalProjectAuthority::new(local.clone(), vec![first, second], 200)
        .expect_err("duplicate current project mapping");
    assert!(matches!(ambiguous, ProjectAuthorityError::Invalid(_)));

    let foreign = snapshot(
        authority("specify:corp"),
        "project-2",
        "snapshot-3",
        OfflineRecoveryPolicy::Deny,
    );
    let mismatched = LocalProjectAuthority::new(local, vec![foreign], 200)
        .expect_err("foreign authority snapshot");
    assert!(matches!(mismatched, ProjectAuthorityError::Invalid(_)));
}
