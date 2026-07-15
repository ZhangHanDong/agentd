use std::collections::HashMap;

use agentd_core::ports::{
    ProjectAuthorityAvailability, ProjectAuthorityError, ProjectAuthorityHealth,
    ProjectAuthorityMode, ProjectAuthorityPort, ProjectSnapshotResolveRequest,
};
use agentd_core::types::{
    AuthorityKey, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef,
};

#[derive(Debug, Clone)]
pub struct LocalProjectAuthority {
    authority_key: AuthorityKey,
    by_project: HashMap<ProjectRef, ProjectExecutionSnapshotRef>,
    by_snapshot: HashMap<ProjectExecutionSnapshotRef, ProjectExecutionSnapshot>,
    checked_at: i64,
    authority_revision: Option<u64>,
}

impl LocalProjectAuthority {
    pub fn new(
        authority_key: AuthorityKey,
        snapshots: Vec<ProjectExecutionSnapshot>,
        checked_at: i64,
    ) -> Result<Self, ProjectAuthorityError> {
        let mut by_project = HashMap::new();
        let mut by_snapshot = HashMap::new();
        let mut authority_revision = None;
        for snapshot in snapshots {
            snapshot
                .validate()
                .map_err(|error| ProjectAuthorityError::Invalid(error.to_string()))?;
            if snapshot.authority_key != authority_key {
                return Err(ProjectAuthorityError::Invalid(
                    "local snapshot authority does not match configured authority".to_string(),
                ));
            }
            if by_project
                .insert(snapshot.project_ref.clone(), snapshot.snapshot_ref.clone())
                .is_some()
            {
                return Err(ProjectAuthorityError::Invalid(
                    "local authority has multiple current snapshots for one project".to_string(),
                ));
            }
            authority_revision = Some(
                authority_revision
                    .unwrap_or(0)
                    .max(snapshot.authority_revision),
            );
            if by_snapshot
                .insert(snapshot.snapshot_ref.clone(), snapshot)
                .is_some()
            {
                return Err(ProjectAuthorityError::Invalid(
                    "local authority has duplicate snapshot references".to_string(),
                ));
            }
        }
        Ok(Self {
            authority_key,
            by_project,
            by_snapshot,
            checked_at,
            authority_revision,
        })
    }
}

#[async_trait::async_trait]
impl ProjectAuthorityPort for LocalProjectAuthority {
    async fn resolve(
        &self,
        request: &ProjectSnapshotResolveRequest,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        if request.expected_authority != self.authority_key
            || request.project_ref.authority_key() != &self.authority_key
        {
            return Err(ProjectAuthorityError::Invalid(
                "resolve request does not target the selected local authority".to_string(),
            ));
        }
        let snapshot_ref = self
            .by_project
            .get(&request.project_ref)
            .ok_or_else(|| ProjectAuthorityError::NotFound("local project snapshot".to_string()))?;
        if request
            .requested_snapshot_ref
            .as_ref()
            .is_some_and(|requested| requested != snapshot_ref)
        {
            return Err(ProjectAuthorityError::NotFound(
                "requested local snapshot is not current for the project".to_string(),
            ));
        }
        self.by_snapshot.get(snapshot_ref).cloned().ok_or_else(|| {
            ProjectAuthorityError::Unverifiable(
                "local current-project index references a missing snapshot".to_string(),
            )
        })
    }

    async fn refresh(
        &self,
        snapshot_ref: &ProjectExecutionSnapshotRef,
    ) -> Result<ProjectExecutionSnapshot, ProjectAuthorityError> {
        if snapshot_ref.authority_key() != &self.authority_key {
            return Err(ProjectAuthorityError::Invalid(
                "refresh request does not target the selected local authority".to_string(),
            ));
        }
        self.by_snapshot
            .get(snapshot_ref)
            .cloned()
            .ok_or_else(|| ProjectAuthorityError::NotFound("local snapshot".to_string()))
    }

    async fn health(&self) -> Result<ProjectAuthorityHealth, ProjectAuthorityError> {
        Ok(ProjectAuthorityHealth {
            authority_key: self.authority_key.clone(),
            mode: ProjectAuthorityMode::Local,
            availability: ProjectAuthorityAvailability::Available,
            checked_at: self.checked_at,
            authority_revision: self.authority_revision,
        })
    }
}
