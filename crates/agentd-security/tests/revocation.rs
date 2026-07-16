use std::sync::Mutex;

use agentd_core::ports::{PolicyRevocationPort, SecurityError};
use agentd_core::types::{
    AuthorityKey, OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef, SecurityCheckpoint,
    SecurityDenialReason, SecurityEpochRequest, SecurityEpochStatus,
};
use agentd_security::revocation::{
    AuthorityRevocationChecker, SecurityEpochAuthority, SecurityEpochAuthorityError,
};

#[derive(Debug)]
struct FakeAuthority {
    result: Mutex<Option<Result<SecurityEpochStatus, SecurityEpochAuthorityError>>>,
    requests: Mutex<Vec<SecurityEpochRequest>>,
}

#[async_trait::async_trait]
impl SecurityEpochAuthority for FakeAuthority {
    async fn current_epoch(
        &self,
        request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityEpochAuthorityError> {
        self.requests
            .lock()
            .expect("requests")
            .push(request.clone());
        self.result
            .lock()
            .expect("result")
            .take()
            .expect("one scripted result")
    }
}

fn request(checkpoint: SecurityCheckpoint) -> SecurityEpochRequest {
    let authority = AuthorityKey::new("specify:revocation-test").expect("authority");
    SecurityEpochRequest {
        checkpoint,
        organization_ref: OrganizationRef::new(authority.clone(), "org-a", "1")
            .expect("organization"),
        project_ref: ProjectRef::new(authority.clone(), "project-a", "2").expect("project"),
        execution_snapshot_ref: ProjectExecutionSnapshotRef::new(authority, "snapshot-a", "3")
            .expect("snapshot"),
        pinned_epoch: 9,
        observed_at: 200,
    }
}

#[tokio::test]
async fn checker_enforces_every_closed_checkpoint_at_equal_epoch() {
    for checkpoint in [
        SecurityCheckpoint::Dispatch,
        SecurityCheckpoint::LeaseRenewal,
        SecurityCheckpoint::ArtifactAcceptance,
        SecurityCheckpoint::Delivery,
        SecurityCheckpoint::Release,
    ] {
        let authority = FakeAuthority {
            result: Mutex::new(Some(Ok(SecurityEpochStatus {
                current_epoch: 9,
                observed_at: 199,
            }))),
            requests: Mutex::new(Vec::new()),
        };
        let checker = AuthorityRevocationChecker::new(authority, 30).expect("checker");
        let expected = request(checkpoint);
        let status = checker
            .check_security_epoch(&expected)
            .await
            .expect("current epoch");
        assert_eq!(status.current_epoch, 9);
        assert_eq!(
            checker
                .authority()
                .requests
                .lock()
                .expect("requests")
                .as_slice(),
            &[expected]
        );
    }
}

#[tokio::test]
async fn checker_denies_advanced_regressed_unavailable_and_malformed_epochs() {
    let cases = [
        (
            Ok(SecurityEpochStatus {
                current_epoch: 10,
                observed_at: 199,
            }),
            SecurityError::Denied(SecurityDenialReason::PolicyEpochStale),
        ),
        (
            Ok(SecurityEpochStatus {
                current_epoch: 8,
                observed_at: 199,
            }),
            SecurityError::Denied(SecurityDenialReason::PolicyEpochRegressed),
        ),
        (
            Ok(SecurityEpochStatus {
                current_epoch: 9,
                observed_at: 201,
            }),
            SecurityError::Unavailable(
                "policy revocation authority returned invalid state".to_string(),
            ),
        ),
        (
            Err(SecurityEpochAuthorityError::Unavailable),
            SecurityError::Unavailable("policy revocation authority unavailable".to_string()),
        ),
        (
            Err(SecurityEpochAuthorityError::Malformed),
            SecurityError::Unavailable(
                "policy revocation authority returned invalid state".to_string(),
            ),
        ),
    ];

    for (result, expected) in cases {
        let checker = AuthorityRevocationChecker::new(
            FakeAuthority {
                result: Mutex::new(Some(result)),
                requests: Mutex::new(Vec::new()),
            },
            30,
        )
        .expect("checker");
        assert_eq!(
            checker
                .check_security_epoch(&request(SecurityCheckpoint::Release))
                .await
                .expect_err("epoch must fail closed"),
            expected
        );
    }
}
