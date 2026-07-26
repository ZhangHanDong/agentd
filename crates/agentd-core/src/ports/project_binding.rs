//! The durable project ↔ room ↔ repository binding boundary. This record is
//! agentd-owned: it is the answer to "which repository and which room does
//! this project execute against", independent of any external authority.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A stored binding. `record_version` increments on every accepted write, so
/// an operator can tell a re-binding from the original declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRoomRepoBinding {
    pub project_id: String,
    pub room_id: String,
    pub repository_id: String,
    pub repository_url: String,
    pub default_branch: String,
    pub record_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Operator-supplied binding declaration. Writing the same project twice
/// re-binds it; writing a room that another project already holds is a
/// conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRoomRepoBindingRequest {
    pub project_id: String,
    pub room_id: String,
    pub repository_id: String,
    pub repository_url: String,
    pub default_branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectBindingError {
    #[error("project binding input is invalid: {0}")]
    Invalid(String),
    #[error("project binding not found: {0}")]
    NotFound(String),
    #[error("project binding conflict: {0}")]
    Conflict(String),
    #[error("project binding store is unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait ProjectBindingPort: Send + Sync {
    /// Declare or re-declare the binding for one project.
    async fn put_binding(
        &self,
        request: &ProjectRoomRepoBindingRequest,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError>;

    /// Read the binding a project holds.
    async fn get_binding_for_project(
        &self,
        project_id: &str,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError>;

    /// Read the binding a Matrix room is covered by.
    async fn get_binding_for_room(
        &self,
        room_id: &str,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError>;
}
