use std::collections::BTreeSet;

use agentd_core::ports::{EnterprisePrincipalPort, PolicyRevocationPort, SecurityError};
use agentd_core::types::{
    AuthorityKey, DataClassification, EnterprisePrincipal, EnterprisePrincipalId,
    EnterpriseRequestIdentity, MatrixDeviceBinding, MatrixDeviceStatus,
    MatrixPrincipalResolveRequest, MatrixTrustPolicy, OidcPrincipalResolveRequest, OrganizationRef,
    PlacementCandidate, PlacementPolicy, PrincipalKind, PrincipalStatus,
    ProjectExecutionSnapshotRef, ProjectRef, SecurityCheckpoint, SecurityDenialReason,
    SecurityEpochRequest, SecurityEpochStatus,
};

fn authority_key() -> AuthorityKey {
    AuthorityKey::new("specify:principal-test").expect("authority key")
}

fn organization() -> OrganizationRef {
    OrganizationRef::new(authority_key(), "org-a", "4").expect("organization ref")
}

fn project() -> ProjectRef {
    ProjectRef::new(authority_key(), "project-a", "9").expect("project ref")
}

fn snapshot() -> ProjectExecutionSnapshotRef {
    ProjectExecutionSnapshotRef::new(authority_key(), "snapshot-a", "12").expect("snapshot ref")
}

fn principal(status: PrincipalStatus) -> EnterprisePrincipal {
    EnterprisePrincipal {
        id: EnterprisePrincipalId::from_string("ep_01ARZ3NDEKTSV4RRFFQ69G5FAA"),
        organization_ref: organization(),
        kind: PrincipalKind::Human,
        status,
        display_name: "Alex".to_string(),
        created_at: 100,
        updated_at: 120,
        disabled_at: (status == PrincipalStatus::Disabled).then_some(120),
    }
}

#[test]
fn disabled_enterprise_principal_fails_closed() {
    assert_eq!(
        principal(PrincipalStatus::Disabled).ensure_active(),
        Err(SecurityDenialReason::PrincipalDisabled)
    );
    assert_eq!(principal(PrincipalStatus::Active).ensure_active(), Ok(()));
}

#[test]
fn matrix_trust_and_device_revocation_fail_closed() {
    let policy = MatrixTrustPolicy {
        trusted_homeservers: BTreeSet::from(["matrix.example".to_string()]),
        trusted_appservices: BTreeSet::from(["agentd-gateway".to_string()]),
    };
    let foreign = MatrixPrincipalResolveRequest {
        user_id: "@alex:foreign.example".to_string(),
        homeserver: "foreign.example".to_string(),
        device_id: Some("DEVICE-A".to_string()),
        appservice_id: None,
        observed_at: 130,
    };
    assert_eq!(
        policy.authorize_source(&foreign),
        Err(SecurityDenialReason::MatrixHomeserverUntrusted)
    );

    let revoked = MatrixDeviceBinding {
        principal_id: EnterprisePrincipalId::from_string("ep_01ARZ3NDEKTSV4RRFFQ69G5FAA"),
        user_id: "@alex:matrix.example".to_string(),
        device_id: "DEVICE-A".to_string(),
        status: MatrixDeviceStatus::Revoked,
        bound_at: 100,
        revoked_at: Some(125),
    };
    assert_eq!(
        revoked.ensure_current(),
        Err(SecurityDenialReason::MatrixDeviceRevoked)
    );
}

#[test]
fn policy_epoch_and_placement_are_closed_contracts() {
    let epoch = SecurityEpochStatus {
        current_epoch: 9,
        observed_at: 140,
    };
    assert_eq!(
        epoch.validate_pinned_epoch(8),
        Err(SecurityDenialReason::PolicyEpochStale)
    );

    let policy = PlacementPolicy {
        data_classification: DataClassification::Restricted,
        allowed_regions: BTreeSet::from(["eu-west-1".to_string()]),
        allowed_worker_trust_domains: BTreeSet::from(["workers.example".to_string()]),
        require_signed_image: true,
        require_dedicated_pool: true,
        egress_profile_id: "restricted-egress-v1".to_string(),
        tenant_cache_namespace: "org-a/project-a".to_string(),
    };
    let candidate = PlacementCandidate {
        supported_data_classifications: BTreeSet::from([DataClassification::Restricted]),
        region: "us-east-1".to_string(),
        worker_trust_domain: "workers.example".to_string(),
        image_digest: "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .to_string(),
        image_signature_verified: true,
        dedicated_pool: true,
        egress_profile_id: "restricted-egress-v1".to_string(),
        tenant_cache_namespace: "org-a/project-a".to_string(),
    };
    assert_eq!(
        policy.evaluate(&candidate),
        Err(SecurityDenialReason::PlacementRegionDenied)
    );
}

#[derive(Debug)]
struct RecordingPrincipalPort;

#[async_trait::async_trait]
impl EnterprisePrincipalPort for RecordingPrincipalPort {
    async fn get_principal(
        &self,
        id: &EnterprisePrincipalId,
    ) -> Result<EnterprisePrincipal, SecurityError> {
        assert_eq!(id.as_str(), "ep_01ARZ3NDEKTSV4RRFFQ69G5FAA");
        Ok(principal(PrincipalStatus::Active))
    }

    async fn resolve_oidc(
        &self,
        request: &OidcPrincipalResolveRequest,
    ) -> Result<EnterpriseRequestIdentity, SecurityError> {
        assert_eq!(request.issuer, "https://identity.example");
        assert_eq!(request.subject, "subject-a");
        Ok(EnterpriseRequestIdentity::oidc(
            principal(PrincipalStatus::Active),
            request.clone(),
            300,
        ))
    }

    async fn resolve_matrix(
        &self,
        request: &MatrixPrincipalResolveRequest,
    ) -> Result<EnterpriseRequestIdentity, SecurityError> {
        Ok(EnterpriseRequestIdentity::matrix(
            principal(PrincipalStatus::Active),
            request.clone(),
            300,
        ))
    }
}

#[async_trait::async_trait]
impl PolicyRevocationPort for RecordingPrincipalPort {
    async fn check_security_epoch(
        &self,
        request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityError> {
        assert_eq!(request.checkpoint, SecurityCheckpoint::Dispatch);
        Ok(SecurityEpochStatus {
            current_epoch: request.pinned_epoch,
            observed_at: request.observed_at,
        })
    }
}

#[tokio::test]
async fn enterprise_principal_port_resolves_verified_request_identities() {
    let port = RecordingPrincipalPort;
    let oidc = port
        .resolve_oidc(&OidcPrincipalResolveRequest {
            issuer: "https://identity.example".to_string(),
            subject: "subject-a".to_string(),
            observed_at: 140,
        })
        .await
        .expect("OIDC identity");
    assert_eq!(oidc.principal.id.as_str(), "ep_01ARZ3NDEKTSV4RRFFQ69G5FAA");

    let epoch = port
        .check_security_epoch(&SecurityEpochRequest {
            checkpoint: SecurityCheckpoint::Dispatch,
            organization_ref: organization(),
            project_ref: project(),
            execution_snapshot_ref: snapshot(),
            pinned_epoch: 8,
            observed_at: 141,
        })
        .await
        .expect("security epoch");
    assert_eq!(epoch.validate_pinned_epoch(8), Ok(()));
}
