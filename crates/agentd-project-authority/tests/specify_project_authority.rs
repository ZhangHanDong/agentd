mod support;

use std::sync::{Arc, Mutex};

use agentd_core::ports::{
    ProjectAuthorityAvailability, ProjectAuthorityError, ProjectAuthorityHealth,
    ProjectAuthorityMode, ProjectAuthorityPort, ProjectSnapshotResolveRequest,
};
use agentd_core::types::{
    OfflineRecoveryPolicy, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef,
};
use agentd_project_authority::{SpecifyAuthorityTransport, SpecifyProjectAuthority};

use support::{authority, resolve_request, snapshot};

#[derive(Debug, Clone)]
struct RecordingTransport {
    calls: Arc<Mutex<Vec<String>>>,
    resolved: ProjectExecutionSnapshot,
    refreshed: ProjectExecutionSnapshot,
    health: ProjectAuthorityHealth,
}

#[async_trait::async_trait]
impl SpecifyAuthorityTransport for RecordingTransport {
    async fn resolve(
        &self,
        request: &ProjectSnapshotResolveRequest,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        self.calls
            .lock()
            .expect("calls")
            .push(format!("resolve:{}", request.project_ref.resource_id()));
        Ok(self.resolved.clone())
    }

    async fn refresh(
        &self,
        snapshot_ref: &ProjectExecutionSnapshotRef,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        self.calls
            .lock()
            .expect("calls")
            .push(format!("refresh:{}", snapshot_ref.resource_id()));
        Ok(self.refreshed.clone())
    }

    async fn health(&self) -> Result<ProjectAuthorityHealth, ProjectAuthorityError> {
        self.calls.lock().expect("calls").push("health".to_string());
        Ok(self.health.clone())
    }
}

#[tokio::test]
async fn specify_project_authority_forwards_contract_and_validates_envelopes() {
    let authority_key = authority("specify:corp");
    let expected = snapshot(
        authority_key.clone(),
        "project-1",
        "snapshot-1",
        OfflineRecoveryPolicy::Deny,
    );
    let calls = Arc::new(Mutex::new(Vec::new()));
    let transport = RecordingTransport {
        calls: Arc::clone(&calls),
        resolved: expected.clone(),
        refreshed: expected.clone(),
        health: ProjectAuthorityHealth {
            authority_key: authority_key.clone(),
            mode: ProjectAuthorityMode::Specify,
            availability: ProjectAuthorityAvailability::Available,
            checked_at: 200,
            authority_revision: Some(9),
        },
    };
    let adapter = SpecifyProjectAuthority::new(authority_key.clone(), transport);
    assert_eq!(
        adapter
            .resolve(&resolve_request(&expected))
            .await
            .expect("resolve"),
        expected
    );
    assert_eq!(
        adapter
            .refresh(&expected.snapshot_ref)
            .await
            .expect("refresh"),
        expected
    );
    assert_eq!(
        adapter.health().await.expect("health").authority_key,
        authority_key
    );
    assert_eq!(
        calls.lock().expect("calls").as_slice(),
        ["resolve:project-1", "refresh:snapshot-1", "health"]
    );

    let foreign = snapshot(
        authority("specify:other"),
        "project-1",
        "snapshot-1",
        OfflineRecoveryPolicy::Deny,
    );
    let invalid_adapter = SpecifyProjectAuthority::new(
        authority("specify:corp"),
        RecordingTransport {
            calls: Arc::new(Mutex::new(Vec::new())),
            resolved: foreign.clone(),
            refreshed: foreign,
            health: ProjectAuthorityHealth {
                authority_key: authority("specify:other"),
                mode: ProjectAuthorityMode::Local,
                availability: ProjectAuthorityAvailability::Available,
                checked_at: 200,
                authority_revision: Some(9),
            },
        },
    );
    let resolve_error = invalid_adapter
        .resolve(&resolve_request(&expected))
        .await
        .expect_err("foreign resolve envelope");
    assert!(matches!(
        resolve_error,
        ProjectAuthorityError::Unverifiable(_)
    ));
    let refresh_error = invalid_adapter
        .refresh(&expected.snapshot_ref)
        .await
        .expect_err("foreign refresh envelope");
    assert!(matches!(
        refresh_error,
        ProjectAuthorityError::Unverifiable(_)
    ));
    let health_error = invalid_adapter
        .health()
        .await
        .expect_err("foreign health envelope");
    assert!(matches!(
        health_error,
        ProjectAuthorityError::Unverifiable(_)
    ));
}

#[derive(Debug, Clone, Copy)]
struct UnavailableTransport;

#[async_trait::async_trait]
impl SpecifyAuthorityTransport for UnavailableTransport {
    async fn resolve(
        &self,
        _request: &ProjectSnapshotResolveRequest,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        Err(ProjectAuthorityError::Unavailable(
            "Specify transport down".to_string(),
        ))
    }

    async fn refresh(
        &self,
        _snapshot_ref: &ProjectExecutionSnapshotRef,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        Err(ProjectAuthorityError::Unavailable(
            "Specify transport down".to_string(),
        ))
    }

    async fn health(&self) -> Result<ProjectAuthorityHealth, ProjectAuthorityError> {
        Err(ProjectAuthorityError::Unavailable(
            "Specify transport down".to_string(),
        ))
    }
}

#[tokio::test]
async fn configured_specify_failure_is_fail_closed_without_local_fallback() {
    let expected = snapshot(
        authority("specify:corp"),
        "project-1",
        "snapshot-1",
        OfflineRecoveryPolicy::Deny,
    );
    let adapter = SpecifyProjectAuthority::new(authority("specify:corp"), UnavailableTransport);

    let error = adapter
        .resolve(&resolve_request(&expected))
        .await
        .expect_err("Specify outage must deny new resolve");
    assert_eq!(
        error,
        ProjectAuthorityError::Unavailable("Specify transport down".to_string())
    );
}
