//! Enterprise Matrix/Robrix gateway, cutover, and projection contracts.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{
    AuditEventId, EnterpriseRequestIdentity, ExecutionArtifactId, MatrixCommandId,
    MatrixGatewayOutboxId, NodeId, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef,
    ProjectRef, ProjectRoomBindingRef, RunId, TaskRunId,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixTransportProvenance {
    pub event_id: String,
    pub room_id: String,
    pub sender_user_id: String,
    pub homeserver: String,
    pub device_id: Option<String>,
    pub appservice_id: Option<String>,
    pub authenticated_sender_user_id: String,
    pub authenticated_appservice_id: Option<String>,
    pub inviter_user_id: Option<String>,
    pub origin_server_ts: i64,
    pub transport_authenticated: bool,
    pub previous_sync_cursor: String,
    pub sync_cursor: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixAttachmentRef {
    pub content_sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixCommandClass {
    Execute,
    Status,
    Cancel,
}

impl MatrixCommandClass {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Execute => "execute",
            Self::Status => "status",
            Self::Cancel => "cancel",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedMatrixCommand {
    pub class: MatrixCommandClass,
    pub arguments: Vec<String>,
    pub attachments: Vec<MatrixAttachmentRef>,
    pub command_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixGatewayMode {
    Observe,
    ShadowReadOnly,
    Canary,
    Active,
    Draining,
    Retired,
    RolledBack,
}

impl MatrixGatewayMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::ShadowReadOnly => "shadow_read_only",
            Self::Canary => "canary",
            Self::Active => "active",
            Self::Draining => "draining",
            Self::Retired => "retired",
            Self::RolledBack => "rolled_back",
        }
    }

    #[must_use]
    pub const fn permits_execution(self) -> bool {
        matches!(self, Self::Canary | Self::Active)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixGatewayProjectConfig {
    pub binding_ref: ProjectRoomBindingRef,
    pub snapshot: ProjectExecutionSnapshot,
    pub room_id: String,
    pub mode: MatrixGatewayMode,
    pub trusted_inviters: Vec<String>,
    pub ignored_senders: Vec<String>,
    pub gateway_user_id: String,
    pub configured_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixGatewayCommandRequest {
    pub provenance: MatrixTransportProvenance,
    pub identity: EnterpriseRequestIdentity,
    pub binding_ref: ProjectRoomBindingRef,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub command: NormalizedMatrixCommand,
    pub observed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixCommandDisposition {
    Accepted,
    Replayed,
    Observed,
    Shadowed,
    Ignored,
    Denied,
}

impl MatrixCommandDisposition {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Replayed => "replayed",
            Self::Observed => "observed",
            Self::Shadowed => "shadowed",
            Self::Ignored => "ignored",
            Self::Denied => "denied",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixCommandReceipt {
    pub command_id: MatrixCommandId,
    pub event_id: String,
    pub disposition: MatrixCommandDisposition,
    pub run_id: Option<RunId>,
    pub outbox_id: Option<MatrixGatewayOutboxId>,
    pub mode: MatrixGatewayMode,
    pub reason_code: Option<String>,
    pub accepted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixGatewayCutoverRequest {
    pub binding_ref: ProjectRoomBindingRef,
    pub expected_mode: MatrixGatewayMode,
    pub next_mode: MatrixGatewayMode,
    pub cursor: String,
    pub reason_code: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixGatewayMappingKind {
    Project,
    Room,
    Principal,
    Task,
    Message,
    Cursor,
    Run,
}

impl MatrixGatewayMappingKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::Room => "room",
            Self::Principal => "principal",
            Self::Task => "task",
            Self::Message => "message",
            Self::Cursor => "cursor",
            Self::Run => "run",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixGatewayStateMappingRequest {
    pub binding_ref: ProjectRoomBindingRef,
    pub kind: MatrixGatewayMappingKind,
    pub legacy_ref_sha256: String,
    pub canonical_ref: String,
    pub in_flight: bool,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixGatewayStateMapping {
    pub kind: MatrixGatewayMappingKind,
    pub legacy_ref_sha256: String,
    pub canonical_ref: String,
    pub in_flight: bool,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixGatewayRollbackManifest {
    pub binding_ref: ProjectRoomBindingRef,
    pub mode: MatrixGatewayMode,
    pub current_cursor: String,
    pub previous_cursor: Option<String>,
    pub mappings: Vec<MatrixGatewayStateMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixExecutionSummaryStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    WaitingApproval,
}

impl MatrixExecutionSummaryStatus {
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::WaitingApproval => "waiting_approval",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixGatewaySummaryPublish {
    pub command_id: MatrixCommandId,
    pub status: MatrixExecutionSummaryStatus,
    pub reason_code: Option<String>,
    pub actionable_links: Vec<String>,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatrixGatewayOutboxRecord {
    pub sequence: u64,
    pub outbox_id: MatrixGatewayOutboxId,
    pub command_id: MatrixCommandId,
    pub room_id: String,
    pub event_kind: String,
    pub summary: String,
    pub actionable_links: Vec<String>,
    pub payload_sha256: String,
    pub created_at: i64,
    pub delivered_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobrixCommandView {
    pub command_id: MatrixCommandId,
    pub class: MatrixCommandClass,
    pub disposition: MatrixCommandDisposition,
    pub run_id: Option<RunId>,
    pub reason_code: Option<String>,
    pub accepted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobrixTaskView {
    pub task_id: TaskRunId,
    pub node_id: NodeId,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobrixArtifactView {
    pub artifact_id: ExecutionArtifactId,
    pub kind: String,
    pub content_sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub storage_ref: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobrixApprovalView {
    pub approval_ref: String,
    pub node_id: NodeId,
    pub status: String,
    pub opened_at: i64,
    pub timeout_at: Option<i64>,
    pub answered_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobrixEvidenceView {
    pub audit_event_id: AuditEventId,
    pub event_type: String,
    pub payload_sha256: String,
    pub occurred_at: i64,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobrixRunView {
    pub run_id: RunId,
    pub status: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub tasks: Vec<RobrixTaskView>,
    pub artifacts: Vec<RobrixArtifactView>,
    pub approvals: Vec<RobrixApprovalView>,
    pub evidence: Vec<RobrixEvidenceView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RobrixProjectView {
    pub project_ref: ProjectRef,
    pub binding_ref: ProjectRoomBindingRef,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub room_id: String,
    pub mode: MatrixGatewayMode,
    pub sync_cursor: String,
    pub recent_commands: Vec<RobrixCommandView>,
    pub recent_runs: Vec<RobrixRunView>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatrixGatewayDenialReason {
    TransportUnauthenticated,
    TransportIdentityMismatch,
    SenderIgnored,
    AppserviceLoop,
    InviterUntrusted,
    BindingMismatch,
    PrincipalUnauthorized,
    CommandNotAllowed,
    ProjectAuthorizationStale,
    SnapshotExpired,
    SideEffectsDisabled,
    DuplicateMismatch,
}

impl MatrixGatewayDenialReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TransportUnauthenticated => "transport_unauthenticated",
            Self::TransportIdentityMismatch => "transport_identity_mismatch",
            Self::SenderIgnored => "sender_ignored",
            Self::AppserviceLoop => "appservice_loop",
            Self::InviterUntrusted => "inviter_untrusted",
            Self::BindingMismatch => "binding_mismatch",
            Self::PrincipalUnauthorized => "principal_unauthorized",
            Self::CommandNotAllowed => "command_not_allowed",
            Self::ProjectAuthorizationStale => "project_authorization_stale",
            Self::SnapshotExpired => "snapshot_expired",
            Self::SideEffectsDisabled => "side_effects_disabled",
            Self::DuplicateMismatch => "duplicate_mismatch",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MatrixGatewayError {
    #[error("invalid Matrix gateway request: {0}")]
    Invalid(String),
    #[error("Matrix gateway request denied: {0:?}")]
    Denied(MatrixGatewayDenialReason),
    #[error("Matrix gateway resource not found: {0}")]
    NotFound(String),
    #[error("Matrix gateway conflict: {0}")]
    Conflict(String),
    #[error("Matrix gateway unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait MatrixGatewayIdentityPort: Send + Sync {
    async fn authenticate_matrix_source(
        &self,
        provenance: &MatrixTransportProvenance,
    ) -> Result<EnterpriseRequestIdentity, MatrixGatewayError>;
}

#[async_trait::async_trait]
pub trait MatrixGatewayDeliveryPort: Send + Sync {
    async fn deliver_summary(
        &self,
        record: &MatrixGatewayOutboxRecord,
    ) -> Result<(), MatrixGatewayError>;
}

#[async_trait::async_trait]
pub trait MatrixGatewayPort: Send + Sync {
    async fn configure_project(
        &self,
        config: &MatrixGatewayProjectConfig,
    ) -> Result<RobrixProjectView, MatrixGatewayError>;

    async fn accept_command(
        &self,
        request: &MatrixGatewayCommandRequest,
    ) -> Result<MatrixCommandReceipt, MatrixGatewayError>;

    async fn transition_cutover(
        &self,
        request: &MatrixGatewayCutoverRequest,
    ) -> Result<RobrixProjectView, MatrixGatewayError>;

    async fn record_state_mapping(
        &self,
        request: &MatrixGatewayStateMappingRequest,
    ) -> Result<MatrixGatewayStateMapping, MatrixGatewayError>;

    async fn rollback_manifest(
        &self,
        binding_ref: &ProjectRoomBindingRef,
    ) -> Result<MatrixGatewayRollbackManifest, MatrixGatewayError>;

    async fn publish_summary(
        &self,
        request: &MatrixGatewaySummaryPublish,
    ) -> Result<MatrixGatewayOutboxId, MatrixGatewayError>;

    async fn outbox_after(
        &self,
        after_sequence: Option<u64>,
        limit: u32,
    ) -> Result<Vec<MatrixGatewayOutboxRecord>, MatrixGatewayError>;

    async fn mark_outbox_delivered(
        &self,
        outbox_id: &MatrixGatewayOutboxId,
        delivered_at: i64,
    ) -> Result<MatrixGatewayOutboxRecord, MatrixGatewayError>;

    async fn project_view(
        &self,
        binding_ref: &ProjectRoomBindingRef,
        recent_limit: u32,
    ) -> Result<Option<RobrixProjectView>, MatrixGatewayError>;
}
