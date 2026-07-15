//! Control-plane port for bounded task dispatch and fencing.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{TaskLeaseClaim, TaskLeaseGrant, TaskRunId, WorkerIncarnationId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLeaseDispatchRequest {
    pub execution_task_id: TaskRunId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub observed_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLeaseRenewRequest {
    pub claim: TaskLeaseClaim,
    pub observed_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskLeaseCloseRequest {
    pub claim: TaskLeaseClaim,
    pub observed_at: i64,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskLeaseRejectionReason {
    ClaimMismatch,
    NotCurrentLease,
    StaleFencingToken,
    TerminalLease,
    LeaseExpired,
    StaleWorkerIncarnation,
}

impl TaskLeaseRejectionReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClaimMismatch => "claim_mismatch",
            Self::NotCurrentLease => "not_current_lease",
            Self::StaleFencingToken => "stale_fencing_token",
            Self::TerminalLease => "terminal_lease",
            Self::LeaseExpired => "lease_expired",
            Self::StaleWorkerIncarnation => "stale_worker_incarnation",
        }
    }
}

impl std::fmt::Display for TaskLeaseRejectionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum TaskLeaseError {
    #[error("invalid task lease input: {0}")]
    Invalid(String),
    #[error("task lease resource not found: {0}")]
    NotFound(String),
    #[error("task lease conflict: {0}")]
    Conflict(String),
    #[error("task lease claim rejected ({reason}): {message}")]
    Rejected {
        reason: TaskLeaseRejectionReason,
        message: String,
    },
    #[error("task lease control plane unavailable: {0}")]
    Unavailable(String),
}

impl TaskLeaseError {
    #[must_use]
    pub const fn rejection_reason(&self) -> Option<TaskLeaseRejectionReason> {
        match self {
            Self::Rejected { reason, .. } => Some(*reason),
            _ => None,
        }
    }
}

#[async_trait::async_trait]
pub trait TaskLeasePort: Send + Sync {
    async fn dispatch(
        &self,
        request: &TaskLeaseDispatchRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError>;

    async fn renew(
        &self,
        request: &TaskLeaseRenewRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError>;

    async fn release(
        &self,
        request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError>;

    async fn cancel(
        &self,
        request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError>;

    async fn validate_claim(
        &self,
        claim: &TaskLeaseClaim,
        observed_at: i64,
    ) -> Result<TaskLeaseGrant, TaskLeaseError>;

    async fn expire_due(&self, observed_at: i64) -> Result<u64, TaskLeaseError>;
}
