//! Durable scheduler port (M2): queue selection, lease grant, and outbox
//! event commit as one authority. Implementations own the transaction
//! boundary; callers never compose queue and lease writes separately.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{
    LeaseId, SchedulerQueueId, SchedulerQueueStatus, TaskLeaseGrant, TaskRunId, WorkerIncarnationId,
};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DurableSchedulerError {
    #[error("invalid scheduler input: {0}")]
    Invalid(String),
    #[error("scheduler resource not found: {0}")]
    NotFound(String),
    #[error("scheduler conflict: {0}")]
    Conflict(String),
    #[error("scheduler unavailable: {0}")]
    Unavailable(String),
}

/// Idempotent enqueue: the same `request_id` for the same task replays the
/// original row; a different payload under the same `request_id` conflicts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerEnqueueRequest {
    pub request_id: String,
    pub execution_task_id: TaskRunId,
    pub max_attempts: u32,
    pub available_at: i64,
    pub enqueued_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerQueueRecord {
    pub id: SchedulerQueueId,
    pub execution_task_id: TaskRunId,
    pub status: SchedulerQueueStatus,
    pub attempts: u32,
    pub max_attempts: u32,
    pub available_at: i64,
    pub current_lease_id: Option<LeaseId>,
    pub last_reason: Option<String>,
    pub enqueued_at: i64,
    pub updated_at: i64,
}

/// Idempotent acquire: the same `request_id` replays the original grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerAcquireRequest {
    pub request_id: String,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub observed_at: i64,
    pub expires_at: i64,
}

/// One task's full scheduling explanation for operators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchedulerTaskExplanation {
    pub queue: SchedulerQueueRecord,
    pub active_lease: Option<TaskLeaseGrant>,
}

#[async_trait::async_trait]
pub trait DurableSchedulerPort: Send + Sync {
    async fn enqueue(
        &self,
        request: &SchedulerEnqueueRequest,
    ) -> Result<SchedulerQueueRecord, DurableSchedulerError>;

    /// Select eligible work, verify the online incarnation, grant the lease,
    /// transition the queue row, and append the outbox event — atomically.
    /// Returns `None` when no work is eligible.
    async fn acquire(
        &self,
        request: &SchedulerAcquireRequest,
    ) -> Result<Option<TaskLeaseGrant>, DurableSchedulerError>;

    /// Reconcile terminal/expired lease state back into the queue: released
    /// leases complete their row; expired leases requeue with attempts+1 or
    /// dead-letter at the limit. Returns rows changed.
    async fn reconcile(&self, observed_at: i64) -> Result<u64, DurableSchedulerError>;

    async fn explain_task(
        &self,
        task_id: &TaskRunId,
    ) -> Result<Option<SchedulerTaskExplanation>, DurableSchedulerError>;
}
