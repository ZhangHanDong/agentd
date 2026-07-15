use agentd_core::ports::{
    ProjectAuthorityError, ProjectAuthorityPort, ProjectSnapshotResolveRequest,
};
use agentd_core::types::{
    OfflineRecoveryPolicy, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef,
    RepositoryRef,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedProjectSnapshot {
    pub snapshot: ProjectExecutionSnapshot,
    pub target_repository_ref: RepositoryRef,
    pub target_base_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryInputs {
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub content_sha256: String,
    pub project_ref: ProjectRef,
    pub target_repository_ref: RepositoryRef,
    pub target_base_commit: String,
}

impl RecoveryInputs {
    #[must_use]
    pub fn from_pinned(pinned: &PinnedProjectSnapshot) -> Self {
        Self {
            snapshot_ref: pinned.snapshot.snapshot_ref.clone(),
            content_sha256: pinned.snapshot.content_sha256.clone(),
            project_ref: pinned.snapshot.project_ref.clone(),
            target_repository_ref: pinned.target_repository_ref.clone(),
            target_base_commit: pinned.target_base_commit.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAuthorization {
    LiveRevalidated,
    OfflinePinned,
}

#[derive(Debug, Clone)]
pub struct ProjectAuthorityControlPlane<P> {
    port: P,
}

impl<P> ProjectAuthorityControlPlane<P> {
    #[must_use]
    pub const fn new(port: P) -> Self {
        Self { port }
    }
}

impl<P> ProjectAuthorityControlPlane<P>
where
    P: ProjectAuthorityPort,
{
    pub async fn authorize_new_execution(
        &self,
        request: &ProjectSnapshotResolveRequest,
        now: i64,
    ) -> Result<PinnedProjectSnapshot, ProjectAuthorityError> {
        let snapshot = self.port.resolve(request).await?;
        validate_resolved_snapshot(&snapshot, request, now)?;
        pin_snapshot(snapshot)
    }

    pub async fn authorize_recovery(
        &self,
        pinned: &PinnedProjectSnapshot,
        inputs: &RecoveryInputs,
        now: i64,
    ) -> Result<RecoveryAuthorization, ProjectAuthorityError> {
        validate_recovery_inputs(pinned, inputs)?;
        validate_snapshot_time(&pinned.snapshot, now)?;

        match self.port.refresh(&pinned.snapshot.snapshot_ref).await {
            Ok(refreshed) => {
                refreshed
                    .validate()
                    .map_err(|error| ProjectAuthorityError::Unverifiable(error.to_string()))?;
                if refreshed != pinned.snapshot {
                    return Err(ProjectAuthorityError::Unverifiable(
                        "live authority returned changed pinned snapshot content".to_string(),
                    ));
                }
                Ok(RecoveryAuthorization::LiveRevalidated)
            }
            Err(ProjectAuthorityError::Unavailable(_))
                if pinned.snapshot.offline_recovery_policy
                    == OfflineRecoveryPolicy::AllowPinnedUntilExpiry =>
            {
                Ok(RecoveryAuthorization::OfflinePinned)
            }
            Err(error) => Err(error),
        }
    }
}

fn validate_resolved_snapshot(
    snapshot: &ProjectExecutionSnapshot,
    request: &ProjectSnapshotResolveRequest,
    now: i64,
) -> Result<(), ProjectAuthorityError> {
    snapshot
        .validate()
        .map_err(|error| ProjectAuthorityError::Unverifiable(error.to_string()))?;
    if snapshot.authority_key != request.expected_authority
        || snapshot.project_ref != request.project_ref
    {
        return Err(ProjectAuthorityError::Unverifiable(
            "resolved snapshot does not match requested authority and project".to_string(),
        ));
    }
    if request
        .requested_snapshot_ref
        .as_ref()
        .is_some_and(|requested| requested != &snapshot.snapshot_ref)
    {
        return Err(ProjectAuthorityError::Unverifiable(
            "resolved snapshot does not match requested immutable snapshot".to_string(),
        ));
    }
    validate_snapshot_time(snapshot, now)
}

fn validate_snapshot_time(
    snapshot: &ProjectExecutionSnapshot,
    now: i64,
) -> Result<(), ProjectAuthorityError> {
    if now < snapshot.issued_at || now >= snapshot.valid_until {
        return Err(ProjectAuthorityError::Unverifiable(
            "project execution snapshot is not valid at the decision time".to_string(),
        ));
    }
    Ok(())
}

fn pin_snapshot(
    snapshot: ProjectExecutionSnapshot,
) -> Result<PinnedProjectSnapshot, ProjectAuthorityError> {
    let target = snapshot
        .target_repository()
        .map_err(|error| ProjectAuthorityError::Unverifiable(error.to_string()))?;
    let target_repository_ref = target.repository_ref.clone();
    let target_base_commit = target.base_commit.clone();
    Ok(PinnedProjectSnapshot {
        snapshot,
        target_repository_ref,
        target_base_commit,
    })
}

fn validate_recovery_inputs(
    pinned: &PinnedProjectSnapshot,
    inputs: &RecoveryInputs,
) -> Result<(), ProjectAuthorityError> {
    if inputs.snapshot_ref != pinned.snapshot.snapshot_ref
        || inputs.content_sha256 != pinned.snapshot.content_sha256
        || inputs.project_ref != pinned.snapshot.project_ref
        || inputs.target_repository_ref != pinned.target_repository_ref
        || inputs.target_base_commit != pinned.target_base_commit
    {
        return Err(ProjectAuthorityError::Conflict(
            "existing recovery inputs differ from the pinned execution snapshot".to_string(),
        ));
    }
    Ok(())
}
