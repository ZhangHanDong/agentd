mod support;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use agentd_core::ports::{
    ProjectAuthorityAvailability, ProjectAuthorityError, ProjectAuthorityHealth,
    ProjectAuthorityMode, ProjectAuthorityPort, ProjectSnapshotResolveRequest,
};
use agentd_core::types::{
    OfflineRecoveryPolicy, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef,
};
use agentd_project_authority::{
    ProjectAuthorityControlPlane, RecoveryAuthorization, RecoveryInputs,
};

use support::{authority, resolve_request, snapshot};

#[derive(Debug, Clone)]
struct ScriptedPort {
    resolve_results: Arc<Mutex<VecDeque<Result<ProjectExecutionSnapshot, ProjectAuthorityError>>>>,
    refresh_results: Arc<Mutex<VecDeque<Result<ProjectExecutionSnapshot, ProjectAuthorityError>>>>,
    resolve_calls: Arc<Mutex<usize>>,
    refresh_calls: Arc<Mutex<usize>>,
}

impl ScriptedPort {
    fn new(
        resolve_results: Vec<Result<ProjectExecutionSnapshot, ProjectAuthorityError>>,
        refresh_results: Vec<Result<ProjectExecutionSnapshot, ProjectAuthorityError>>,
    ) -> Self {
        Self {
            resolve_results: Arc::new(Mutex::new(resolve_results.into())),
            refresh_results: Arc::new(Mutex::new(refresh_results.into())),
            resolve_calls: Arc::new(Mutex::new(0)),
            refresh_calls: Arc::new(Mutex::new(0)),
        }
    }

    fn resolve_count(&self) -> usize {
        *self.resolve_calls.lock().expect("resolve count")
    }

    fn refresh_count(&self) -> usize {
        *self.refresh_calls.lock().expect("refresh count")
    }
}

#[async_trait::async_trait]
impl ProjectAuthorityPort for ScriptedPort {
    async fn resolve(
        &self,
        _request: &ProjectSnapshotResolveRequest,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        *self.resolve_calls.lock().expect("resolve calls") += 1;
        self.resolve_results
            .lock()
            .expect("resolve results")
            .pop_front()
            .expect("scripted resolve result")
    }

    async fn refresh(
        &self,
        _snapshot_ref: &ProjectExecutionSnapshotRef,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        *self.refresh_calls.lock().expect("refresh calls") += 1;
        self.refresh_results
            .lock()
            .expect("refresh results")
            .pop_front()
            .expect("scripted refresh result")
    }

    async fn health(&self) -> Result<ProjectAuthorityHealth, ProjectAuthorityError> {
        Ok(ProjectAuthorityHealth {
            authority_key: authority("specify:corp"),
            mode: ProjectAuthorityMode::Specify,
            availability: ProjectAuthorityAvailability::Available,
            checked_at: 200,
            authority_revision: Some(9),
        })
    }
}

#[tokio::test]
async fn control_plane_new_execution_pins_validated_snapshot() {
    let expected = snapshot(
        authority("specify:corp"),
        "project-1",
        "snapshot-1",
        OfflineRecoveryPolicy::Deny,
    );
    let port = ScriptedPort::new(vec![Ok(expected.clone())], vec![]);
    let control_plane = ProjectAuthorityControlPlane::new(port.clone());
    let pinned = control_plane
        .authorize_new_execution(&resolve_request(&expected), 200)
        .await
        .expect("authorize new execution");
    assert_eq!(pinned.snapshot, expected);
    assert_eq!(pinned.target_repository_ref.resource_id(), "repo-1");
    assert_eq!(
        pinned.target_base_commit,
        "0123456789abcdef0123456789abcdef01234567"
    );
    assert_eq!(port.resolve_count(), 1);

    let mut expired = pinned.snapshot.clone();
    expired.valid_until = 200;
    let expired_port = ScriptedPort::new(vec![Ok(expired)], vec![]);
    let error = ProjectAuthorityControlPlane::new(expired_port)
        .authorize_new_execution(&resolve_request(&pinned.snapshot), 200)
        .await
        .expect_err("expired snapshot");
    assert!(matches!(error, ProjectAuthorityError::Unverifiable(_)));

    let foreign = snapshot(
        authority("specify:corp"),
        "project-2",
        "snapshot-2",
        OfflineRecoveryPolicy::Deny,
    );
    let foreign_port = ScriptedPort::new(vec![Ok(foreign)], vec![]);
    let error = ProjectAuthorityControlPlane::new(foreign_port)
        .authorize_new_execution(&resolve_request(&pinned.snapshot), 200)
        .await
        .expect_err("mismatched project");
    assert!(matches!(error, ProjectAuthorityError::Unverifiable(_)));
}

#[tokio::test]
async fn control_plane_recovery_enforces_live_or_bounded_offline_policy() {
    let expected = snapshot(
        authority("specify:corp"),
        "project-1",
        "snapshot-1",
        OfflineRecoveryPolicy::AllowPinnedUntilExpiry,
    );
    let live_port = ScriptedPort::new(vec![Ok(expected.clone())], vec![Ok(expected.clone())]);
    let live_control_plane = ProjectAuthorityControlPlane::new(live_port.clone());
    let live_pinned = live_control_plane
        .authorize_new_execution(&resolve_request(&expected), 200)
        .await
        .expect("pin live snapshot");
    let live_inputs = RecoveryInputs::from_pinned(&live_pinned);
    let live = live_control_plane
        .authorize_recovery(&live_pinned, &live_inputs, 300)
        .await
        .expect("live recovery");
    assert!(matches!(live, RecoveryAuthorization::LiveRevalidated));
    assert_eq!(live_port.refresh_count(), 1);

    assert_offline_policy_and_expiry(&expected).await;
    assert_changed_recovery_inputs(&expected).await;
}

async fn assert_offline_policy_and_expiry(expected: &ProjectExecutionSnapshot) {
    let unavailable = ProjectAuthorityError::Unavailable("Specify down".to_string());
    let offline_port = ScriptedPort::new(vec![Ok(expected.clone())], vec![Err(unavailable)]);
    let offline_control_plane = ProjectAuthorityControlPlane::new(offline_port);
    let offline_pinned = offline_control_plane
        .authorize_new_execution(&resolve_request(expected), 200)
        .await
        .expect("pin offline candidate");
    let offline_inputs = RecoveryInputs::from_pinned(&offline_pinned);
    let offline = offline_control_plane
        .authorize_recovery(&offline_pinned, &offline_inputs, 300)
        .await
        .expect("bounded offline recovery");
    assert!(matches!(offline, RecoveryAuthorization::OfflinePinned));

    let deny_snapshot = snapshot(
        authority("specify:corp"),
        "project-deny",
        "snapshot-deny",
        OfflineRecoveryPolicy::Deny,
    );
    let deny_port = ScriptedPort::new(
        vec![Ok(deny_snapshot.clone())],
        vec![Err(ProjectAuthorityError::Unavailable(
            "Specify down".to_string(),
        ))],
    );
    let deny_control_plane = ProjectAuthorityControlPlane::new(deny_port);
    let deny_pinned = deny_control_plane
        .authorize_new_execution(&resolve_request(&deny_snapshot), 200)
        .await
        .expect("pin deny snapshot");
    let deny_error = deny_control_plane
        .authorize_recovery(
            &deny_pinned,
            &RecoveryInputs::from_pinned(&deny_pinned),
            300,
        )
        .await
        .expect_err("deny policy blocks unavailable recovery");
    assert!(matches!(deny_error, ProjectAuthorityError::Unavailable(_)));

    let expired_port = ScriptedPort::new(
        vec![Ok(expected.clone())],
        vec![Err(ProjectAuthorityError::Unavailable(
            "Specify down".to_string(),
        ))],
    );
    let expired_control_plane = ProjectAuthorityControlPlane::new(expired_port);
    let expired_pinned = expired_control_plane
        .authorize_new_execution(&resolve_request(expected), 200)
        .await
        .expect("pin expiring snapshot");
    let expired_error = expired_control_plane
        .authorize_recovery(
            &expired_pinned,
            &RecoveryInputs::from_pinned(&expired_pinned),
            1_000,
        )
        .await
        .expect_err("expiry blocks offline recovery");
    assert!(matches!(
        expired_error,
        ProjectAuthorityError::Unverifiable(_)
    ));
}

async fn assert_changed_recovery_inputs(expected: &ProjectExecutionSnapshot) {
    let changed_input_port = ScriptedPort::new(vec![Ok(expected.clone())], vec![]);
    let changed_input_control_plane = ProjectAuthorityControlPlane::new(changed_input_port.clone());
    let changed_input_pinned = changed_input_control_plane
        .authorize_new_execution(&resolve_request(expected), 200)
        .await
        .expect("pin changed-input snapshot");
    let mut changed_inputs = RecoveryInputs::from_pinned(&changed_input_pinned);
    changed_inputs.target_base_commit = "f".repeat(40);
    let changed_error = changed_input_control_plane
        .authorize_recovery(&changed_input_pinned, &changed_inputs, 300)
        .await
        .expect_err("changed input blocks recovery before refresh");
    assert!(matches!(changed_error, ProjectAuthorityError::Conflict(_)));
    assert_eq!(changed_input_port.refresh_count(), 0);

    let mut changed_live = expected.clone();
    changed_live.content_sha256 = "b".repeat(64);
    let changed_live_port = ScriptedPort::new(vec![Ok(expected.clone())], vec![Ok(changed_live)]);
    let changed_live_control_plane = ProjectAuthorityControlPlane::new(changed_live_port);
    let changed_live_pinned = changed_live_control_plane
        .authorize_new_execution(&resolve_request(expected), 200)
        .await
        .expect("pin changed-live snapshot");
    let changed_live_error = changed_live_control_plane
        .authorize_recovery(
            &changed_live_pinned,
            &RecoveryInputs::from_pinned(&changed_live_pinned),
            300,
        )
        .await
        .expect_err("changed live snapshot blocks recovery");
    assert!(matches!(
        changed_live_error,
        ProjectAuthorityError::Unverifiable(_)
    ));
}
