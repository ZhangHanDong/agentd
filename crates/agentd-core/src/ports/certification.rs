//! Signed execution evidence, OpenFab certification, forge, and Skill Hub contracts.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::types::{
    AgentProfileId, CertificationGate, CertificationPolicyVersionRef, CertificationRequestId,
    CertificationResultId, EvidenceEnvelopeId, ExecutionArtifactId, FencingToken, ForgeAdmissionId,
    LeaseId, OrganizationRef, ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef,
    RepositoryRef, RunId, RuntimeAttemptId, RuntimeSessionId, SkillInstallationId,
    SkillPackageBinding, SkillPackageVersionRef, TaskRunId, WorkerIncarnationId,
};

pub const EXECUTION_EVIDENCE_ENVELOPE_SCHEMA_VERSION: u16 = 1;
pub const OPENFAB_CERTIFICATION_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceSignerRole {
    Builder,
    Worker,
    OpenFab,
}

impl EvidenceSignerRole {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Builder => "builder",
            Self::Worker => "worker",
            Self::OpenFab => "openfab",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedSigningKey {
    pub key_id: String,
    pub signer_did: String,
    pub role: EvidenceSignerRole,
    pub not_before: i64,
    pub not_after: i64,
    pub revoked_at: Option<i64>,
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImmutableEvidenceRef {
    pub authority_key: String,
    pub resource_kind: String,
    pub resource_id: String,
    pub resource_version: String,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceArtifactSubject {
    pub artifact_id: ExecutionArtifactId,
    pub kind: String,
    pub content_sha256: String,
    pub size_bytes: u64,
    pub storage_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackageEvidenceRef {
    pub package_ref: SkillPackageVersionRef,
    pub archive_sha256: String,
    pub manifest_sha256: String,
    pub dependency_lock_sha256: String,
    pub permissions_sha256: String,
}

impl From<&SkillPackageBinding> for SkillPackageEvidenceRef {
    fn from(value: &SkillPackageBinding) -> Self {
        Self {
            package_ref: value.package_ref.clone(),
            archive_sha256: value.archive_sha256.clone(),
            manifest_sha256: value.manifest_sha256.clone(),
            dependency_lock_sha256: value.dependency_lock_sha256.clone(),
            permissions_sha256: value.permissions_sha256.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionEvidenceEnvelopePayload {
    pub schema_version: u16,
    pub envelope_id: EvidenceEnvelopeId,
    pub organization_ref: OrganizationRef,
    pub project_ref: ProjectRef,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub snapshot_content_sha256: String,
    pub execution_run_id: RunId,
    pub execution_task_id: Option<TaskRunId>,
    pub runtime_session_id: Option<RuntimeSessionId>,
    pub runtime_attempt_id: Option<RuntimeAttemptId>,
    pub worker_incarnation_id: Option<WorkerIncarnationId>,
    pub agent_profile_id: Option<AgentProfileId>,
    pub lease_id: Option<LeaseId>,
    pub fencing_token: Option<FencingToken>,
    pub runtime_name: String,
    pub model: Option<String>,
    pub sandbox_profile_sha256: String,
    pub requirements: Vec<ImmutableEvidenceRef>,
    pub frozen_spec: ImmutableEvidenceRef,
    pub prompt_sha256: String,
    pub plan_sha256: Option<String>,
    pub target_repository_ref: RepositoryRef,
    pub target_base_commit: String,
    pub produced_commit: Option<String>,
    pub produced_diff_sha256: Option<String>,
    pub artifacts: Vec<EvidenceArtifactSubject>,
    pub review_evidence: Vec<ImmutableEvidenceRef>,
    pub human_decisions: Vec<ImmutableEvidenceRef>,
    pub policy_refs: Vec<ImmutableEvidenceRef>,
    pub skill_packages: Vec<SkillPackageEvidenceRef>,
    pub usage_ledger_sha256: String,
    pub recovery_events_sha256: String,
    pub issued_at: i64,
    pub completed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedExecutionEvidenceEnvelope {
    pub schema_version: u16,
    pub payload: ExecutionEvidenceEnvelopePayload,
    pub payload_sha256: String,
    pub signer_key_id: String,
    pub signer_did: String,
    pub signer_role: EvidenceSignerRole,
    pub signature_algorithm: String,
    pub signature_b64: String,
    pub signed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CertificationVerdict {
    Pass,
    Fail,
    Revoked,
}

impl CertificationVerdict {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Revoked => "revoked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificationRequest {
    pub schema_version: u16,
    pub request_id: CertificationRequestId,
    pub idempotency_key: String,
    pub openfab_authority_key: String,
    pub envelope_id: EvidenceEnvelopeId,
    pub evidence_payload_sha256: String,
    pub evidence_storage_ref: String,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub snapshot_content_sha256: String,
    pub source_commit: String,
    pub subject_sha256: String,
    pub spec_sha256: String,
    pub certification_policy_ref: CertificationPolicyVersionRef,
    pub certification_policy_sha256: String,
    pub gate: CertificationGate,
    pub skill_packages: Vec<SkillPackageEvidenceRef>,
    pub skill_packages_sha256: String,
    pub requested_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificationResultPayload {
    pub schema_version: u16,
    pub result_id: CertificationResultId,
    pub request_id: CertificationRequestId,
    pub openfab_authority_key: String,
    pub evidence_payload_sha256: String,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub snapshot_content_sha256: String,
    pub source_commit: String,
    pub subject_sha256: String,
    pub spec_sha256: String,
    pub certification_policy_ref: CertificationPolicyVersionRef,
    pub certification_policy_sha256: String,
    pub skill_packages_sha256: String,
    pub verdict: CertificationVerdict,
    pub machine_attested: bool,
    pub required_human_signoffs: u16,
    pub eligible_human_signoffs: u16,
    pub accepted_human_signoffs: u16,
    pub reason_code: Option<String>,
    pub signed_ref: String,
    pub published_at: i64,
    pub revoked_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedCertificationResult {
    pub schema_version: u16,
    pub payload: CertificationResultPayload,
    pub payload_sha256: String,
    pub signer_key_id: String,
    pub signer_did: String,
    pub signature_algorithm: String,
    pub signature_b64: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryCertificationState {
    Produced,
    Delivered,
    MachineAttested,
    HumanCertified,
    Released,
    Revoked,
}

impl DeliveryCertificationState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Produced => "produced",
            Self::Delivered => "delivered",
            Self::MachineAttested => "machine_attested",
            Self::HumanCertified => "human_certified",
            Self::Released => "released",
            Self::Revoked => "revoked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificationStateTransition {
    pub idempotency_key: String,
    pub execution_artifact_id: ExecutionArtifactId,
    pub previous_state: Option<DeliveryCertificationState>,
    pub next_state: DeliveryCertificationState,
    pub certification_result_id: Option<CertificationResultId>,
    pub reason_code: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeOperation {
    Merge,
    Release,
}

impl ForgeOperation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Release => "release",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeAdmissionRequest {
    pub idempotency_key: String,
    pub operation: ForgeOperation,
    pub snapshot: ProjectExecutionSnapshot,
    pub execution_artifact_id: ExecutionArtifactId,
    pub source_commit: String,
    pub subject_sha256: String,
    pub certification_policy_sha256: Option<String>,
    pub certification_result: Option<SignedCertificationResult>,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeAdmission {
    pub id: ForgeAdmissionId,
    pub operation: ForgeOperation,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub snapshot_content_sha256: String,
    pub execution_artifact_id: ExecutionArtifactId,
    pub source_commit: String,
    pub subject_sha256: String,
    pub certification_policy_ref: Option<CertificationPolicyVersionRef>,
    pub certification_policy_sha256: Option<String>,
    pub certification_result_id: Option<CertificationResultId>,
    pub certification_result_payload_sha256: Option<String>,
    pub admitted_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillPackageTrustStatus {
    Draft,
    InReview,
    Approved,
    Signed,
    Yanked,
    Revoked,
    Deprecated,
}

impl SkillPackageTrustStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::InReview => "in_review",
            Self::Approved => "approved",
            Self::Signed => "signed",
            Self::Yanked => "yanked",
            Self::Revoked => "revoked",
            Self::Deprecated => "deprecated",
        }
    }

    #[must_use]
    pub const fn permits_new_install(self) -> bool {
        matches!(self, Self::Approved | Self::Signed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackageTrustPayload {
    pub package: SkillPackageEvidenceRef,
    pub status: SkillPackageTrustStatus,
    pub published_at: i64,
    pub status_changed_at: i64,
    pub valid_until: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillPackageTrustRecord {
    pub payload: SkillPackageTrustPayload,
    pub trust_payload_sha256: String,
    pub signer_key_id: String,
    pub signer_did: String,
    pub signature_b64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInstallRequest {
    pub snapshot: ProjectExecutionSnapshot,
    pub package: SkillPackageBinding,
    pub install_root_ref: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillInstallAdmission {
    pub id: SkillInstallationId,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub snapshot_content_sha256: String,
    pub package_ref: SkillPackageVersionRef,
    pub archive_sha256: String,
    pub manifest_sha256: String,
    pub dependency_lock_sha256: String,
    pub permissions_sha256: String,
    pub install_root_ref: String,
    pub trust_status_at_install: SkillPackageTrustStatus,
    pub trust_record: SkillPackageTrustRecord,
    pub admitted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CertificationError {
    #[error("invalid certification input: {0}")]
    Invalid(String),
    #[error("certification denied: {0}")]
    Denied(String),
    #[error("certification resource not found: {0}")]
    NotFound(String),
    #[error("certification conflict: {0}")]
    Conflict(String),
    #[error("certification unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait SigningKeyTrustPort: Send + Sync {
    async fn resolve_signing_key(
        &self,
        key_id: &str,
        role: EvidenceSignerRole,
        signed_at: i64,
    ) -> Result<TrustedSigningKey, CertificationError>;
}

#[async_trait::async_trait]
pub trait EvidenceSigningPort: Send + Sync {
    async fn sign_evidence(
        &self,
        payload: &ExecutionEvidenceEnvelopePayload,
        signed_at: i64,
    ) -> Result<SignedExecutionEvidenceEnvelope, CertificationError>;
}

#[async_trait::async_trait]
pub trait EvidenceVerificationPort: Send + Sync {
    async fn verify_evidence(
        &self,
        envelope: &SignedExecutionEvidenceEnvelope,
        observed_at: i64,
    ) -> Result<(), CertificationError>;

    async fn verify_certification_result(
        &self,
        result: &SignedCertificationResult,
        observed_at: i64,
    ) -> Result<(), CertificationError>;

    async fn verify_skill_trust(
        &self,
        record: &SkillPackageTrustRecord,
        observed_at: i64,
    ) -> Result<(), CertificationError>;
}

#[async_trait::async_trait]
pub trait EvidenceEnvelopeStorePort: Send + Sync {
    async fn store_evidence_envelope(
        &self,
        envelope: &SignedExecutionEvidenceEnvelope,
    ) -> Result<SignedExecutionEvidenceEnvelope, CertificationError>;

    async fn load_evidence_envelope(
        &self,
        envelope_id: &EvidenceEnvelopeId,
    ) -> Result<Option<SignedExecutionEvidenceEnvelope>, CertificationError>;
}

#[async_trait::async_trait]
pub trait CertificationPort: Send + Sync {
    async fn request_certification(
        &self,
        request: &CertificationRequest,
    ) -> Result<(), CertificationError>;

    async fn certification_result(
        &self,
        request_id: &CertificationRequestId,
    ) -> Result<Option<SignedCertificationResult>, CertificationError>;
}

#[async_trait::async_trait]
pub trait CertificationStatePort: Send + Sync {
    async fn record_certification_request(
        &self,
        request: &CertificationRequest,
    ) -> Result<CertificationRequest, CertificationError>;

    async fn record_certification_result(
        &self,
        result: &SignedCertificationResult,
    ) -> Result<SignedCertificationResult, CertificationError>;

    async fn transition_certification_state(
        &self,
        transition: &CertificationStateTransition,
    ) -> Result<CertificationStateTransition, CertificationError>;

    async fn admit_forge_operation(
        &self,
        request: &ForgeAdmissionRequest,
    ) -> Result<ForgeAdmission, CertificationError>;

    async fn record_skill_installation(
        &self,
        admission: &SkillInstallAdmission,
    ) -> Result<SkillInstallAdmission, CertificationError>;
}

#[async_trait::async_trait]
pub trait SkillHubPort: Send + Sync {
    async fn resolve_package_trust(
        &self,
        package_ref: &SkillPackageVersionRef,
        observed_at: i64,
    ) -> Result<SkillPackageTrustRecord, CertificationError>;
}

pub fn canonical_json<T: Serialize>(value: &T) -> Result<String, CertificationError> {
    let value = serde_json::to_value(value)
        .map_err(|error| CertificationError::Invalid(error.to_string()))?;
    let mut output = String::new();
    write_canonical(&value, &mut output)?;
    Ok(output)
}

pub fn canonical_sha256<T: Serialize>(value: &T) -> Result<String, CertificationError> {
    Ok(hex::encode(Sha256::digest(
        canonical_json(value)?.as_bytes(),
    )))
}

fn write_canonical(
    value: &serde_json::Value,
    output: &mut String,
) -> Result<(), CertificationError> {
    match value {
        serde_json::Value::Null => output.push_str("null"),
        serde_json::Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        serde_json::Value::Number(value) => output.push_str(&value.to_string()),
        serde_json::Value::String(value) => output.push_str(
            &serde_json::to_string(value)
                .map_err(|error| CertificationError::Invalid(error.to_string()))?,
        ),
        serde_json::Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_canonical(value, output)?;
            }
            output.push(']');
        }
        serde_json::Value::Object(values) => {
            let mut keys: Vec<_> = values.keys().collect();
            keys.sort_unstable();
            output.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(
                    &serde_json::to_string(key)
                        .map_err(|error| CertificationError::Invalid(error.to_string()))?,
                );
                output.push(':');
                write_canonical(&values[*key], output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}
