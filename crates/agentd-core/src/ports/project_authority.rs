//! Port for resolving immutable project-authority execution snapshots.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{
    AuthorityKey, ProjectAuthorityValidationError, ProjectExecutionSnapshot,
    ProjectExecutionSnapshotRef, ProjectRef,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSnapshotResolveRequest {
    pub expected_authority: AuthorityKey,
    pub project_ref: ProjectRef,
    pub requested_snapshot_ref: Option<ProjectExecutionSnapshotRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectAuthorityMode {
    Local,
    Specify,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectAuthorityAvailability {
    Available,
    Unavailable,
    Degraded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectAuthorityHealth {
    pub authority_key: AuthorityKey,
    pub mode: ProjectAuthorityMode,
    pub availability: ProjectAuthorityAvailability,
    pub checked_at: i64,
    pub authority_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectAuthorityError {
    #[error("invalid project authority input: {0}")]
    Invalid(String),
    #[error("project authority unavailable: {0}")]
    Unavailable(String),
    #[error("project authority resource not found: {0}")]
    NotFound(String),
    #[error("project authority response is unverifiable: {0}")]
    Unverifiable(String),
    #[error("project authority conflict: {0}")]
    Conflict(String),
}

impl From<ProjectAuthorityValidationError> for ProjectAuthorityError {
    fn from(value: ProjectAuthorityValidationError) -> Self {
        Self::Invalid(value.to_string())
    }
}

#[async_trait::async_trait]
pub trait ProjectAuthorityPort: Send + Sync {
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
