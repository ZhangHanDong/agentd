use agentd_core::ports::{
    ProjectAuthorityError, ProjectAuthorityHealth, ProjectAuthorityMode, ProjectAuthorityPort,
    ProjectSnapshotResolveRequest,
};
use agentd_core::types::{
    AuthorityKey, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef,
};

#[async_trait::async_trait]
pub trait SpecifyAuthorityTransport: Send + Sync {
    async fn resolve(
        &self,
        request: &ProjectSnapshotResolveRequest,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError>;

    async fn refresh(
        &self,
        snapshot_ref: &ProjectExecutionSnapshotRef,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError>;

    async fn health(&self) -> Result<ProjectAuthorityHealth, ProjectAuthorityError>;
}

#[derive(Debug, Clone)]
pub struct SpecifyProjectAuthority<T> {
    authority_key: AuthorityKey,
    transport: T,
}

impl<T> SpecifyProjectAuthority<T> {
    #[must_use]
    pub const fn new(authority_key: AuthorityKey, transport: T) -> Self {
        Self {
            authority_key,
            transport,
        }
    }
}

#[async_trait::async_trait]
impl<T> ProjectAuthorityPort for SpecifyProjectAuthority<T>
where
    T: SpecifyAuthorityTransport,
{
    async fn resolve(
        &self,
        request: &ProjectSnapshotResolveRequest,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        if request.expected_authority != self.authority_key
            || request.project_ref.authority_key() != &self.authority_key
        {
            return Err(ProjectAuthorityError::Invalid(
                "resolve request does not target the configured Specify authority".to_string(),
            ));
        }
        let snapshot = self.transport.resolve(request).await?;
        validate_snapshot_envelope(
            &snapshot,
            &self.authority_key,
            &request.project_ref,
            request.requested_snapshot_ref.as_ref(),
        )?;
        Ok(snapshot)
    }

    async fn refresh(
        &self,
        snapshot_ref: &ProjectExecutionSnapshotRef,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        if snapshot_ref.authority_key() != &self.authority_key {
            return Err(ProjectAuthorityError::Invalid(
                "refresh request does not target the configured Specify authority".to_string(),
            ));
        }
        let snapshot = self.transport.refresh(snapshot_ref).await?;
        snapshot
            .validate()
            .map_err(|error| ProjectAuthorityError::Unverifiable(error.to_string()))?;
        if snapshot.authority_key != self.authority_key || &snapshot.snapshot_ref != snapshot_ref {
            return Err(ProjectAuthorityError::Unverifiable(
                "Specify refresh returned a different authority or snapshot".to_string(),
            ));
        }
        Ok(snapshot)
    }

    async fn health(&self) -> Result<ProjectAuthorityHealth, ProjectAuthorityError> {
        let health = self.transport.health().await?;
        if health.authority_key != self.authority_key
            || health.mode != ProjectAuthorityMode::Specify
        {
            return Err(ProjectAuthorityError::Unverifiable(
                "Specify health returned a different authority or adapter mode".to_string(),
            ));
        }
        Ok(health)
    }
}

fn validate_snapshot_envelope(
    snapshot: &ProjectExecutionSnapshot,
    authority_key: &AuthorityKey,
    project_ref: &ProjectRef,
    requested_snapshot_ref: Option<&ProjectExecutionSnapshotRef>,
) -> Result<(), ProjectAuthorityError> {
    snapshot
        .validate()
        .map_err(|error| ProjectAuthorityError::Unverifiable(error.to_string()))?;
    if &snapshot.authority_key != authority_key || &snapshot.project_ref != project_ref {
        return Err(ProjectAuthorityError::Unverifiable(
            "Specify resolve returned a different authority or project".to_string(),
        ));
    }
    if requested_snapshot_ref.is_some_and(|requested| requested != &snapshot.snapshot_ref) {
        return Err(ProjectAuthorityError::Unverifiable(
            "Specify resolve returned a different requested snapshot".to_string(),
        ));
    }
    Ok(())
}
