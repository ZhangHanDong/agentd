//! Closed values shared by enterprise execution-security ports and adapters.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroizing;

use super::{
    AuthorityKey, OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef, RbacPolicyVersionRef,
    RepositoryRef, TaskLeaseClaim, WorkerId, WorkerIncarnationId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkloadRole {
    ControlPlane,
    Gateway,
    Worker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthenticatedWorkload {
    pub spiffe_uri: String,
    pub role: WorkloadRole,
    pub trust_domain: String,
    pub certificate_sha256: String,
    pub not_before: i64,
    pub not_after: i64,
    pub worker_id: Option<WorkerId>,
    pub worker_incarnation_id: Option<WorkerIncarnationId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadIdentityRequest {
    pub peer_certificates_der: Vec<Vec<u8>>,
    pub observed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectedAction {
    SandboxPrepare,
    SandboxExecute,
    SecretCheckout,
    ArtifactRead,
    ArtifactWrite,
    ForgeRead,
    ForgeWrite,
    ToolHighRisk,
}

impl ProtectedAction {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SandboxPrepare => "sandbox.prepare",
            Self::SandboxExecute => "sandbox.execute",
            Self::SecretCheckout => "secret.checkout",
            Self::ArtifactRead => "artifact.read",
            Self::ArtifactWrite => "artifact.write",
            Self::ForgeRead => "forge.read",
            Self::ForgeWrite => "forge.write",
            Self::ToolHighRisk => "tool.high_risk",
        }
    }
}

impl fmt::Display for ProtectedAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "selector", rename_all = "snake_case")]
pub enum ProtectedResourceKind {
    Execution,
    Repository(RepositoryRef),
    Secret(SecretSelector),
    Artifact(String),
    Forge(RepositoryRef),
    Tool(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProtectedResource {
    pub organization_ref: OrganizationRef,
    pub project_ref: ProjectRef,
    pub execution_snapshot_ref: ProjectExecutionSnapshotRef,
    pub kind: ProtectedResourceKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSecurityScope {
    pub authority_key: AuthorityKey,
    pub organization_ref: OrganizationRef,
    pub project_ref: ProjectRef,
    pub execution_snapshot_ref: ProjectExecutionSnapshotRef,
    pub rbac_policy_version_ref: RbacPolicyVersionRef,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub task_lease_claim: TaskLeaseClaim,
    pub sandbox_profile_id: String,
    pub egress_profile_id: String,
    pub policy_revocation_epoch: u64,
    pub valid_until: i64,
}

impl ExecutionSecurityScope {
    /// Bind a protected resource to this immutable Specify-owned scope.
    pub fn authorize_resource(
        &self,
        resource: &ProtectedResource,
    ) -> Result<AuthorizedResourceScope, SecurityDenialReason> {
        if resource.organization_ref != self.organization_ref {
            return Err(SecurityDenialReason::TenantMismatch);
        }
        if resource.project_ref != self.project_ref {
            return Err(SecurityDenialReason::ProjectMismatch);
        }
        if resource.execution_snapshot_ref != self.execution_snapshot_ref {
            return Err(SecurityDenialReason::SnapshotMismatch);
        }
        Ok(AuthorizedResourceScope {
            scope: self.clone(),
            resource: resource.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthorizedResourceScope {
    pub scope: ExecutionSecurityScope,
    pub resource: ProtectedResource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantAuthorizationRequest {
    pub workload: AuthenticatedWorkload,
    pub scope: ExecutionSecurityScope,
    pub action: ProtectedAction,
    pub resource: ProtectedResource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TenantAuthorization {
    pub workload: AuthenticatedWorkload,
    pub scope: ExecutionSecurityScope,
    pub action: ProtectedAction,
    pub resource: ProtectedResource,
    pub authorized_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AttemptCapabilityId(String);

impl AttemptCapabilityId {
    #[must_use]
    pub fn new() -> Self {
        Self(format!("cp_{}", ulid::Ulid::new()))
    }

    pub fn from_string(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for AttemptCapabilityId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AttemptCapabilityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct CapabilityToken(Zeroizing<[u8; 32]>);

impl CapabilityToken {
    #[must_use]
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    #[must_use]
    pub fn expose_secret(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for CapabilityToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CapabilityToken([REDACTED])")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityAdmission {
    pub id: AttemptCapabilityId,
    pub workload: AuthenticatedWorkload,
    pub scope: ExecutionSecurityScope,
    pub action: ProtectedAction,
    pub resource: ProtectedResource,
    pub issued_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityIssueRequest {
    pub authorization: TenantAuthorization,
    pub requested_expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityValidationRequest {
    pub token: CapabilityToken,
    pub scope: ExecutionSecurityScope,
    pub action: ProtectedAction,
    pub resource: ProtectedResource,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretSelector(String);

impl SecretSelector {
    pub fn new(value: impl Into<String>) -> Result<Self, SecurityValueError> {
        let value = value.into();
        if value.trim().is_empty() || value.contains('\0') {
            return Err(SecurityValueError::InvalidSecretSelector);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct SecretMaterial(Zeroizing<Vec<u8>>);

impl SecretMaterial {
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }

    #[must_use]
    pub fn expose_secret(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretMaterial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretMaterial([REDACTED])")
    }
}

pub struct SecretLease {
    pub selector: SecretSelector,
    pub material: SecretMaterial,
    pub expires_at: i64,
}

impl fmt::Debug for SecretLease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SecretLease")
            .field("selector", &self.selector)
            .field("material", &self.material)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretCheckoutRequest {
    pub admission: CapabilityAdmission,
    pub selector: SecretSelector,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OciSandboxRuntime {
    Docker,
    Podman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMountAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxMount {
    pub source_id: String,
    pub target: String,
    pub access: SandboxMountAccess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxLimits {
    pub pids: u32,
    pub memory_bytes: u64,
    pub cpu_millis: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", content = "allowlist", rename_all = "snake_case")]
pub enum EgressPolicy {
    DenyAll,
    Allow(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSandboxProfile {
    pub profile_id: String,
    pub runtime: OciSandboxRuntime,
    pub image_digest: String,
    pub read_only_root: bool,
    pub ephemeral_workspace: bool,
    pub mounts: Vec<SandboxMount>,
    pub drop_all_capabilities: bool,
    pub no_new_privileges: bool,
    pub seccomp_profile: String,
    pub limits: SandboxLimits,
    pub tenant_cache_namespace: String,
    pub shared_cache: bool,
    pub egress: EgressPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxPrepareRequest {
    pub admission: CapabilityAdmission,
    pub profile: ExecutionSandboxProfile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedSandbox {
    pub sandbox_id: String,
    pub profile: ExecutionSandboxProfile,
    pub created_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxExecuteRequest {
    pub sandbox: PreparedSandbox,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxExecution {
    pub exit_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxCleanupRequest {
    pub sandbox_id: String,
    pub observed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityDenialReason {
    IdentityUntrusted,
    IdentityExpired,
    IdentityRevoked,
    IncarnationStale,
    TenantMismatch,
    ProjectMismatch,
    SnapshotMismatch,
    ActionDenied,
    ResourceDenied,
    CapabilityExpired,
    CapabilityRevoked,
    CapabilityScopeMismatch,
    LeaseRejected,
    SecretUnavailable,
    SandboxProfileDenied,
    SandboxStartFailed,
    SandboxCleanupFailed,
}

impl SecurityDenialReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::IdentityUntrusted => "identity_untrusted",
            Self::IdentityExpired => "identity_expired",
            Self::IdentityRevoked => "identity_revoked",
            Self::IncarnationStale => "incarnation_stale",
            Self::TenantMismatch => "tenant_mismatch",
            Self::ProjectMismatch => "project_mismatch",
            Self::SnapshotMismatch => "snapshot_mismatch",
            Self::ActionDenied => "action_denied",
            Self::ResourceDenied => "resource_denied",
            Self::CapabilityExpired => "capability_expired",
            Self::CapabilityRevoked => "capability_revoked",
            Self::CapabilityScopeMismatch => "capability_scope_mismatch",
            Self::LeaseRejected => "lease_rejected",
            Self::SecretUnavailable => "secret_unavailable",
            Self::SandboxProfileDenied => "sandbox_profile_denied",
            Self::SandboxStartFailed => "sandbox_start_failed",
            Self::SandboxCleanupFailed => "sandbox_cleanup_failed",
        }
    }
}

impl fmt::Display for SecurityDenialReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SecurityValueError {
    #[error("secret selector must be non-empty and contain no NUL bytes")]
    InvalidSecretSelector,
}
