//! Typed control-plane APIs for immutable execution evidence.

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};
use serde_json::Value;
use thiserror::Error;

use crate::ports::TaskLeaseRejectionReason;
use crate::types::{
    AuditEventId, ExecutionArtifactId, RunId, RuntimeAttemptId, RuntimeSessionId, TaskLeaseClaim,
    TaskRunId, WorkerIncarnationId,
};

macro_rules! evidence_kind {
    (
        $name:ident {
            $($variant:ident => $value:literal),+ $(,)?
        }
    ) => {
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $value),+
                }
            }
        }

        impl TryFrom<&str> for $name {
            type Error = &'static str;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                match value {
                    $($value => Ok(Self::$variant),)+
                    _ => Err(concat!("invalid ", stringify!($name))),
                }
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

evidence_kind!(
    ExecutionArtifactKind {
        Requirements => "requirements",
        Spec => "spec",
        Plan => "plan",
        Review => "review",
        RuntimeSummary => "runtime_summary",
        Transcript => "transcript",
        Log => "log",
        Patch => "patch",
        Commit => "commit",
        TestReport => "test_report",
    }
);

evidence_kind!(
    AuditActorKind {
        ControlPlane => "control_plane",
        Worker => "worker",
        AgentProfile => "agent_profile",
        Operator => "operator",
        ProjectAuthority => "project_authority",
        CertificationAuthority => "certification_authority",
        System => "system",
        Import => "import",
    }
);

evidence_kind!(
    UsageMetric {
        InputTokens => "input_tokens",
        CachedInputTokens => "cached_input_tokens",
        OutputTokens => "output_tokens",
        ReasoningTokens => "reasoning_tokens",
        ToolCalls => "tool_calls",
        RuntimeMilliseconds => "runtime_milliseconds",
        ArtifactBytes => "artifact_bytes",
    }
);

evidence_kind!(
    CertificationReferenceKind {
        Request => "request",
        Result => "result",
        Signature => "signature",
        Attestation => "attestation",
    }
);

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSnapshotLink {
    pub authority_key: String,
    pub resource_kind: String,
    pub resource_id: String,
    pub resource_version: String,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEvidenceLinks {
    pub execution_run_id: RunId,
    pub execution_task_id: Option<TaskRunId>,
    pub runtime_session_id: Option<RuntimeSessionId>,
    pub runtime_attempt_id: Option<RuntimeAttemptId>,
    pub worker_incarnation_id: Option<WorkerIncarnationId>,
    pub snapshot: ExecutionSnapshotLink,
    pub target_repository_id: String,
    pub target_base_commit: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionArtifactPublish {
    pub id: ExecutionArtifactId,
    pub kind: ExecutionArtifactKind,
    pub content_sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub storage_ref: String,
    pub provenance: Value,
    pub links: ExecutionEvidenceLinks,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionArtifactRecord {
    pub publish: ExecutionArtifactPublish,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactCursor {
    pub created_at: i64,
    pub id: ExecutionArtifactId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactListRequest {
    pub execution_run_id: RunId,
    pub cursor: Option<ArtifactCursor>,
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactPage {
    pub records: Vec<ExecutionArtifactRecord>,
    pub next_cursor: Option<ArtifactCursor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionAuditAppend {
    pub id: AuditEventId,
    pub idempotency_scope: String,
    pub idempotency_key: String,
    pub event_type: String,
    pub actor_kind: AuditActorKind,
    pub actor_ref: String,
    pub payload_sha256: String,
    pub payload: Value,
    pub links: ExecutionEvidenceLinks,
    pub execution_artifact_id: Option<ExecutionArtifactId>,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionAuditRecord {
    pub append: ExecutionAuditAppend,
    pub sequence: u64,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditReadRequest {
    pub execution_run_id: RunId,
    pub after_sequence: u64,
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuditPage {
    pub records: Vec<ExecutionAuditRecord>,
    pub next_after_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageMeasurement {
    pub id: AuditEventId,
    pub idempotency_scope: String,
    pub idempotency_key: String,
    pub actor_kind: AuditActorKind,
    pub actor_ref: String,
    pub metric: UsageMetric,
    pub quantity: u64,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub links: ExecutionEvidenceLinks,
    pub measured_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub measurement: UsageMeasurement,
    pub sequence: u64,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageReadRequest {
    pub execution_run_id: RunId,
    pub after_sequence: u64,
    pub limit: PageLimit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsagePage {
    pub records: Vec<UsageRecord>,
    pub next_after_sequence: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotal {
    pub metric: UsageMetric,
    pub quantity: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTotals {
    pub execution_run_id: RunId,
    pub totals: Vec<UsageTotal>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificationReferenceAppend {
    pub execution_artifact_id: ExecutionArtifactId,
    pub authority_key: String,
    pub kind: CertificationReferenceKind,
    pub external_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificationReferenceRecord {
    pub id: u64,
    pub append: CertificationReferenceAppend,
    pub recorded_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerArtifactReport {
    pub claim: TaskLeaseClaim,
    pub observed_at: i64,
    pub artifact: ExecutionArtifactPublish,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerArtifactAcknowledgement {
    pub artifact: ExecutionArtifactRecord,
    pub accepted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerUsageReport {
    pub claim: TaskLeaseClaim,
    pub observed_at: i64,
    pub measurement: UsageMeasurement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct PageLimit(u16);

impl PageLimit {
    pub const MAX: u16 = 200;

    /// Construct a bounded evidence page size.
    ///
    /// # Errors
    /// Returns [`ExecutionEvidenceValidationError`] unless `value` is in
    /// `1..=200`.
    pub const fn new(value: u16) -> Result<Self, ExecutionEvidenceValidationError> {
        if value == 0 || value > Self::MAX {
            Err(ExecutionEvidenceValidationError::InvalidPageLimit(value))
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn value(self) -> u16 {
        self.0
    }
}

impl<'de> Deserialize<'de> for PageLimit {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = u16::deserialize(deserializer)?;
        Self::new(value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ExecutionEvidenceValidationError {
    #[error("page limit {0} is outside 1..=200")]
    InvalidPageLimit(u16),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExecutionEvidenceError {
    #[error("invalid execution evidence input: {0}")]
    Invalid(String),
    #[error("execution evidence not found: {0}")]
    NotFound(String),
    #[error("execution evidence conflict: {0}")]
    Conflict(String),
    #[error("worker evidence rejected ({reason}): {message}")]
    LeaseRejected {
        reason: TaskLeaseRejectionReason,
        message: String,
    },
    #[error("execution evidence unavailable: {0}")]
    Unavailable(String),
}

impl ExecutionEvidenceError {
    #[must_use]
    pub const fn lease_rejection_reason(&self) -> Option<TaskLeaseRejectionReason> {
        match self {
            Self::LeaseRejected { reason, .. } => Some(*reason),
            _ => None,
        }
    }
}

#[async_trait::async_trait]
pub trait ArtifactIndexPort: Send + Sync {
    async fn publish_artifact(
        &self,
        request: &ExecutionArtifactPublish,
    ) -> Result<ExecutionArtifactRecord, ExecutionEvidenceError>;

    async fn publish_worker_artifact(
        &self,
        request: &WorkerArtifactReport,
    ) -> Result<ExecutionArtifactRecord, ExecutionEvidenceError>;

    async fn acknowledge_worker_artifact(
        &self,
        request: &WorkerArtifactReport,
    ) -> Result<WorkerArtifactAcknowledgement, ExecutionEvidenceError>;

    async fn get_artifact(
        &self,
        id: &ExecutionArtifactId,
    ) -> Result<Option<ExecutionArtifactRecord>, ExecutionEvidenceError>;

    async fn list_artifacts(
        &self,
        request: &ArtifactListRequest,
    ) -> Result<ArtifactPage, ExecutionEvidenceError>;
}

#[async_trait::async_trait]
pub trait ExecutionAuditPort: Send + Sync {
    async fn append_audit(
        &self,
        request: &ExecutionAuditAppend,
    ) -> Result<ExecutionAuditRecord, ExecutionEvidenceError>;

    async fn read_audit(
        &self,
        request: &AuditReadRequest,
    ) -> Result<AuditPage, ExecutionEvidenceError>;
}

#[async_trait::async_trait]
pub trait UsageLedgerPort: Send + Sync {
    async fn record_usage(
        &self,
        request: &UsageMeasurement,
    ) -> Result<UsageRecord, ExecutionEvidenceError>;

    async fn record_worker_usage(
        &self,
        request: &WorkerUsageReport,
    ) -> Result<UsageRecord, ExecutionEvidenceError>;

    async fn read_usage(
        &self,
        request: &UsageReadRequest,
    ) -> Result<UsagePage, ExecutionEvidenceError>;

    async fn usage_totals(
        &self,
        execution_run_id: &RunId,
    ) -> Result<UsageTotals, ExecutionEvidenceError>;
}

#[async_trait::async_trait]
pub trait CertificationReferencePort: Send + Sync {
    async fn append_certification_reference(
        &self,
        request: &CertificationReferenceAppend,
    ) -> Result<CertificationReferenceRecord, ExecutionEvidenceError>;

    async fn list_certification_references(
        &self,
        artifact_id: &ExecutionArtifactId,
    ) -> Result<Vec<CertificationReferenceRecord>, ExecutionEvidenceError>;
}
