//! Durable final-cutover contracts. Agent-chat appears only as an offline source.

use serde::{Deserialize, Serialize};

use crate::types::{
    BackupManifestId, CutoverId, CutoverReceiptId, CutoverSourceId, ServiceInstallationId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutoverState {
    Planned,
    Importing,
    Shadowing,
    Draining,
    HandoffReady,
    Active,
    Retired,
    RolledBack,
}

impl CutoverState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Importing => "importing",
            Self::Shadowing => "shadowing",
            Self::Draining => "draining",
            Self::HandoffReady => "handoff_ready",
            Self::Active => "active",
            Self::Retired => "retired",
            Self::RolledBack => "rolled_back",
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Retired | Self::RolledBack)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutoverSurface {
    Agent,
    Group,
    Message,
    Cursor,
    Task,
    TaskGraph,
    MatrixProject,
}

impl CutoverSurface {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Group => "group",
            Self::Message => "message",
            Self::Cursor => "cursor",
            Self::Task => "task",
            Self::TaskGraph => "task_graph",
            Self::MatrixProject => "matrix_project",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceModel {
    Local,
    Team,
    Fleet,
}

impl ServiceModel {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Team => "team",
            Self::Fleet => "fleet",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverPlan {
    pub id: CutoverId,
    pub source_root_sha256: String,
    pub target_database_sha256: Option<String>,
    pub rollback_window_expires_at: i64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverRun {
    pub plan: CutoverPlan,
    pub state: CutoverState,
    pub source_id: Option<CutoverSourceId>,
    pub authority_owner: String,
    pub record_version: u64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverTransition {
    pub cutover_id: CutoverId,
    pub expected_state: CutoverState,
    pub next_state: CutoverState,
    pub idempotency_key: String,
    pub input_sha256: String,
    pub authority_owner: String,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverSourceManifest {
    pub id: CutoverSourceId,
    pub cutover_id: CutoverId,
    pub source_sha256: String,
    pub file_count: u32,
    pub record_count: u64,
    pub captured_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyIdMapping {
    pub cutover_id: CutoverId,
    pub surface: CutoverSurface,
    pub legacy_id_sha256: String,
    pub native_id: String,
    pub native_record_sha256: String,
    pub mapped_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowDecision {
    pub cutover_id: CutoverId,
    pub surface: CutoverSurface,
    pub decision_key_sha256: String,
    pub legacy_decision_sha256: String,
    pub native_decision_sha256: String,
    pub matched: bool,
    pub reason_code: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverStepReceipt {
    pub id: CutoverReceiptId,
    pub cutover_id: CutoverId,
    pub step: String,
    pub idempotency_key: String,
    pub input_sha256: String,
    pub output_sha256: String,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorHandoff {
    pub cutover_id: CutoverId,
    pub project_ref_sha256: String,
    pub previous_cursor_sha256: String,
    pub next_cursor: String,
    pub authority_owner: String,
    pub acknowledged: bool,
    pub handed_off_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupManifest {
    pub id: BackupManifestId,
    pub cutover_id: CutoverId,
    pub database_sha256: String,
    pub schema_version: u32,
    pub size_bytes: u64,
    pub storage_ref: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInstallation {
    pub id: ServiceInstallationId,
    pub cutover_id: CutoverId,
    pub model: ServiceModel,
    pub manifest_sha256: String,
    pub target_ref_sha256: String,
    pub installed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CutoverError {
    #[error("invalid cutover request: {0}")]
    Invalid(String),
    #[error("cutover conflict: {0}")]
    Conflict(String),
    #[error("cutover resource not found: {0}")]
    NotFound(String),
    #[error("cutover operation unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait CutoverLedgerPort: Send + Sync {
    async fn create_cutover(&self, plan: &CutoverPlan) -> Result<CutoverRun, CutoverError>;
    async fn load_cutover(&self, id: &CutoverId) -> Result<Option<CutoverRun>, CutoverError>;
    async fn transition_cutover(
        &self,
        transition: &CutoverTransition,
    ) -> Result<CutoverRun, CutoverError>;
    async fn record_source(
        &self,
        manifest: &CutoverSourceManifest,
    ) -> Result<CutoverSourceManifest, CutoverError>;
    async fn record_mapping(
        &self,
        mapping: &LegacyIdMapping,
    ) -> Result<LegacyIdMapping, CutoverError>;
    async fn record_shadow(
        &self,
        decision: &ShadowDecision,
    ) -> Result<ShadowDecision, CutoverError>;
    async fn record_step(
        &self,
        receipt: &CutoverStepReceipt,
    ) -> Result<CutoverStepReceipt, CutoverError>;
    async fn record_cursor_handoff(
        &self,
        handoff: &CursorHandoff,
    ) -> Result<CursorHandoff, CutoverError>;
    async fn record_backup(
        &self,
        manifest: &BackupManifest,
    ) -> Result<BackupManifest, CutoverError>;
    async fn record_service_installation(
        &self,
        installation: &ServiceInstallation,
    ) -> Result<ServiceInstallation, CutoverError>;
    async fn mappings(&self, id: &CutoverId) -> Result<Vec<LegacyIdMapping>, CutoverError>;
    async fn shadows(&self, id: &CutoverId) -> Result<Vec<ShadowDecision>, CutoverError>;
    async fn cursor_handoffs(&self, id: &CutoverId) -> Result<Vec<CursorHandoff>, CutoverError>;
}
