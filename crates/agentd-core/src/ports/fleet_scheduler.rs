//! Durable enterprise scheduler and authenticated worker-fleet contracts.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{
    ArtifactUploadId, AuthenticatedWorkload, DataClassification, ExecutionArtifactId, FencingToken,
    FleetOutboxId, LeaseId, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProtectedAction,
    SecurityCheckpoint, TaskLeaseClaim, TaskLeaseGrant, TaskRunId, WorkerId, WorkerIncarnationId,
    WorkerStatus,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetQueueStatus {
    Queued,
    Acquired,
    RetryWait,
    Completed,
    Cancelled,
    DeadLetter,
}

impl FleetQueueStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Acquired => "acquired",
            Self::RetryWait => "retry_wait",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::DeadLetter => "dead_letter",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::DeadLetter)
    }
}

impl TryFrom<&str> for FleetQueueStatus {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "queued" => Ok(Self::Queued),
            "acquired" => Ok(Self::Acquired),
            "retry_wait" => Ok(Self::RetryWait),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "dead_letter" => Ok(Self::DeadLetter),
            _ => Err("invalid fleet queue status"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetTaskRequirements {
    pub resource_class: String,
    pub required_capabilities: BTreeSet<String>,
    pub quota_max_active: u32,
    pub priority: i32,
    pub max_attempts: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetSubmitRequest {
    pub idempotency_key: String,
    pub execution_task_id: TaskRunId,
    pub snapshot: ProjectExecutionSnapshot,
    pub requirements: FleetTaskRequirements,
    pub submitted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetTaskRecord {
    pub execution_task_id: TaskRunId,
    pub idempotency_key: String,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub snapshot_content_sha256: String,
    pub policy_revocation_epoch: u64,
    pub requirements: FleetTaskRequirements,
    pub status: FleetQueueStatus,
    pub attempt_count: u32,
    pub current_claim: Option<TaskLeaseClaim>,
    pub next_eligible_at: Option<i64>,
    pub outcome_sha256: Option<String>,
    pub block_code: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerAvailability {
    pub worker_id: WorkerId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub heartbeat_sequence: u64,
    pub worker_status: WorkerStatus,
    pub daemon_version: String,
    pub protocol_min: u32,
    pub protocol_max: u32,
    pub region: String,
    pub zone: String,
    pub resource_class: String,
    pub capabilities: BTreeSet<String>,
    pub total_slots: u32,
    pub available_slots: u32,
    pub data_classifications: BTreeSet<DataClassification>,
    pub image_digest: String,
    pub image_signature_verified: bool,
    pub dedicated_pool: bool,
    pub egress_profile_ids: BTreeSet<String>,
    pub tenant_cache_namespaces: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetHeartbeatRequest {
    pub workload: AuthenticatedWorkload,
    pub availability: WorkerAvailability,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetPullRequest {
    pub workload: AuthenticatedWorkload,
    pub protocol_version: u32,
    pub observed_at: i64,
    pub heartbeat_max_age_seconds: u32,
    pub lease_expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetAssignment {
    pub task: FleetTaskRecord,
    pub lease: TaskLeaseGrant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetRenewRequest {
    pub workload: AuthenticatedWorkload,
    pub claim: TaskLeaseClaim,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub pinned_revocation_epoch: u64,
    pub observed_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetCompletionReport {
    pub workload: AuthenticatedWorkload,
    pub claim: TaskLeaseClaim,
    pub idempotency_key: String,
    pub outcome_sha256: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetFailureReport {
    pub workload: AuthenticatedWorkload,
    pub claim: TaskLeaseClaim,
    pub idempotency_key: String,
    pub failure_code: String,
    pub retryable: bool,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetCancelRequest {
    pub workload: AuthenticatedWorkload,
    pub claim: TaskLeaseClaim,
    pub idempotency_key: String,
    pub reason_code: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactUploadAckRequest {
    pub workload: AuthenticatedWorkload,
    pub claim: TaskLeaseClaim,
    pub upload_id: ArtifactUploadId,
    pub execution_artifact_id: ExecutionArtifactId,
    pub idempotency_key: String,
    pub artifact_sha256: String,
    pub upload_attempt: u32,
    pub part_count: u32,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactUploadAck {
    pub upload_id: ArtifactUploadId,
    pub execution_artifact_id: ExecutionArtifactId,
    pub claim: TaskLeaseClaim,
    pub artifact_sha256: String,
    pub upload_attempt: u32,
    pub part_count: u32,
    pub acknowledged_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetSideEffectRequest {
    pub workload: AuthenticatedWorkload,
    pub claim: TaskLeaseClaim,
    pub checkpoint: SecurityCheckpoint,
    pub action: ProtectedAction,
    pub idempotency_key: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetSideEffectAdmission {
    pub execution_task_id: TaskRunId,
    pub lease_id: LeaseId,
    pub fencing_token: FencingToken,
    pub checkpoint: SecurityCheckpoint,
    pub action: ProtectedAction,
    pub admitted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetReapRequest {
    pub observed_at: i64,
    pub heartbeat_stale_before: i64,
    pub lease_expired_before: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetReapSummary {
    pub workers_offlined: u64,
    pub leases_expired: u64,
    pub tasks_requeued: u64,
    pub tasks_dead_lettered: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetOutboxEvent {
    pub id: FleetOutboxId,
    pub event_type: String,
    pub execution_task_id: TaskRunId,
    pub claim: Option<TaskLeaseClaim>,
    pub payload_sha256: String,
    pub created_at: i64,
    pub delivered_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FleetExplain {
    pub execution_task_id: TaskRunId,
    pub status: FleetQueueStatus,
    pub attempt_count: u32,
    pub current_claim: Option<TaskLeaseClaim>,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub policy_revocation_epoch: u64,
    pub worker_incarnation_id: Option<WorkerIncarnationId>,
    pub block_code: Option<String>,
    pub next_eligible_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FleetDenialReason {
    IdentityMismatch,
    WorkerNotCurrent,
    WorkerNotOnline,
    WorkerDraining,
    HeartbeatStale,
    HeartbeatSequenceRegressed,
    ProtocolUnsupported,
    CapacityUnavailable,
    QuotaExceeded,
    SnapshotExpired,
    RevocationEpochStale,
    PlacementDenied,
    StaleFencingToken,
    TaskTerminal,
    DuplicateMismatch,
    ArtifactUploadRejected,
    SideEffectDenied,
}

impl FleetDenialReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdentityMismatch => "identity_mismatch",
            Self::WorkerNotCurrent => "worker_not_current",
            Self::WorkerNotOnline => "worker_not_online",
            Self::WorkerDraining => "worker_draining",
            Self::HeartbeatStale => "heartbeat_stale",
            Self::HeartbeatSequenceRegressed => "heartbeat_sequence_regressed",
            Self::ProtocolUnsupported => "protocol_unsupported",
            Self::CapacityUnavailable => "capacity_unavailable",
            Self::QuotaExceeded => "quota_exceeded",
            Self::SnapshotExpired => "snapshot_expired",
            Self::RevocationEpochStale => "revocation_epoch_stale",
            Self::PlacementDenied => "placement_denied",
            Self::StaleFencingToken => "stale_fencing_token",
            Self::TaskTerminal => "task_terminal",
            Self::DuplicateMismatch => "duplicate_mismatch",
            Self::ArtifactUploadRejected => "artifact_upload_rejected",
            Self::SideEffectDenied => "side_effect_denied",
        }
    }
}

impl std::fmt::Display for FleetDenialReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FleetSchedulerError {
    #[error("invalid fleet scheduler request: {0}")]
    Invalid(String),
    #[error("fleet scheduler resource not found: {0}")]
    NotFound(String),
    #[error("fleet scheduler conflict: {0}")]
    Conflict(String),
    #[error("fleet scheduler request denied ({0})")]
    Denied(FleetDenialReason),
    #[error("fleet scheduler unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait FleetSchedulerPort: Send + Sync {
    async fn submit_task(
        &self,
        request: &FleetSubmitRequest,
    ) -> Result<FleetTaskRecord, FleetSchedulerError>;

    async fn heartbeat(
        &self,
        request: &FleetHeartbeatRequest,
    ) -> Result<WorkerAvailability, FleetSchedulerError>;

    async fn pull(
        &self,
        request: &FleetPullRequest,
    ) -> Result<Option<FleetAssignment>, FleetSchedulerError>;

    async fn renew(
        &self,
        request: &FleetRenewRequest,
    ) -> Result<TaskLeaseGrant, FleetSchedulerError>;

    async fn complete(
        &self,
        request: &FleetCompletionReport,
    ) -> Result<FleetTaskRecord, FleetSchedulerError>;

    async fn fail(
        &self,
        request: &FleetFailureReport,
    ) -> Result<FleetTaskRecord, FleetSchedulerError>;

    async fn cancel(
        &self,
        request: &FleetCancelRequest,
    ) -> Result<FleetTaskRecord, FleetSchedulerError>;

    async fn acknowledge_artifact_upload(
        &self,
        request: &ArtifactUploadAckRequest,
    ) -> Result<ArtifactUploadAck, FleetSchedulerError>;

    async fn admit_side_effect(
        &self,
        request: &FleetSideEffectRequest,
    ) -> Result<FleetSideEffectAdmission, FleetSchedulerError>;

    async fn reap(
        &self,
        request: &FleetReapRequest,
    ) -> Result<FleetReapSummary, FleetSchedulerError>;

    async fn outbox_after(
        &self,
        after: Option<&FleetOutboxId>,
        limit: u32,
    ) -> Result<Vec<FleetOutboxEvent>, FleetSchedulerError>;

    async fn explain(
        &self,
        execution_task_id: &TaskRunId,
    ) -> Result<Option<FleetExplain>, FleetSchedulerError>;
}
