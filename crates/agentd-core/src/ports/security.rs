//! Closed execution-security admission contracts for enterprise workers.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{
    AuthorityKey, FencingToken, OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef,
    RbacPolicyVersionRef, TaskLeaseClaim, WorkerIncarnationId,
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
    pub spiffe_id: String,
    pub role: WorkloadRole,
    pub trust_domain: String,
    pub certificate_fingerprint: String,
    pub valid_from: i64,
    pub valid_until: i64,
    pub worker_incarnation_id: Option<WorkerIncarnationId>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "selector", rename_all = "snake_case")]
pub enum ProtectedResource {
    Sandbox(String),
    Secret(String),
    Artifact(String),
    Forge(String),
    Tool(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionSecurityScope {
    pub authority_key: AuthorityKey,
    pub organization_ref: OrganizationRef,
    pub project_ref: ProjectRef,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub rbac_policy_version_ref: RbacPolicyVersionRef,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub lease_claim: TaskLeaseClaim,
    pub sandbox_profile: String,
    pub egress_profile: String,
    pub policy_revocation_epoch: u64,
    pub valid_from: i64,
    pub valid_until: i64,
}

impl ExecutionSecurityScope {
    pub fn validate(&self) -> Result<(), SecurityDenial> {
        if self.valid_from >= self.valid_until {
            return Err(SecurityDenial::SnapshotMismatch);
        }
        if self.snapshot_ref.authority_key() != &self.authority_key
            || self.project_ref.authority_key() != &self.authority_key
            || self.organization_ref.authority_key() != &self.authority_key
        {
            return Err(SecurityDenial::TenantMismatch);
        }
        if self.lease_claim.worker_incarnation_id != self.worker_incarnation_id {
            return Err(SecurityDenial::IncarnationStale);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityAdmission {
    pub action: ProtectedAction,
    pub resource: ProtectedResource,
    pub scope: ExecutionSecurityScope,
    pub fencing_token: FencingToken,
}

impl CapabilityAdmission {
    /// Validate the capability shape without a control-plane lookup.
    /// Revocation and the current policy epoch remain authorizer concerns.
    pub fn validate_at(&self, observed_at: i64) -> Result<(), SecurityDenial> {
        self.scope.validate()?;
        if observed_at < self.scope.valid_from || observed_at >= self.scope.valid_until {
            return Err(SecurityDenial::CapabilityExpired);
        }
        if self.fencing_token != self.scope.lease_claim.fencing_token {
            return Err(SecurityDenial::LeaseRejected);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecurityDenial {
    #[error("identity is not trusted")]
    IdentityUntrusted,
    #[error("identity is expired")]
    IdentityExpired,
    #[error("identity is revoked")]
    IdentityRevoked,
    #[error("worker incarnation is stale")]
    IncarnationStale,
    #[error("tenant does not match")]
    TenantMismatch,
    #[error("project does not match")]
    ProjectMismatch,
    #[error("snapshot does not match")]
    SnapshotMismatch,
    #[error("protected action is denied")]
    ActionDenied,
    #[error("protected resource is denied")]
    ResourceDenied,
    #[error("capability is expired")]
    CapabilityExpired,
    #[error("capability is revoked")]
    CapabilityRevoked,
    #[error("capability scope does not match")]
    CapabilityScopeMismatch,
    #[error("task lease rejected")]
    LeaseRejected,
    #[error("sandbox cleanup failed")]
    SandboxCleanupFailed,
    #[error("sandbox profile denied")]
    SandboxProfileDenied,
}

#[async_trait::async_trait]
pub trait TenantAuthorizationPort: Send + Sync {
    async fn authorize(
        &self,
        workload: &AuthenticatedWorkload,
        action: ProtectedAction,
        resource: &ProtectedResource,
        scope: &ExecutionSecurityScope,
        observed_at: i64,
    ) -> Result<(), SecurityDenial>;
}

#[async_trait::async_trait]
pub trait MtlsWorkloadVerifier: Send + Sync {
    async fn verify_peer(
        &self,
        peer_certificate_der: &[u8],
        observed_at: i64,
    ) -> Result<AuthenticatedWorkload, SecurityDenial>;
}

#[derive(Clone, PartialEq, Eq)]
pub struct SecretMaterial(Vec<u8>);

impl SecretMaterial {
    #[must_use]
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl std::fmt::Debug for SecretMaterial {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SecretMaterial(REDACTED)")
    }
}

impl Drop for SecretMaterial {
    fn drop(&mut self) {
        for byte in &mut self.0 {
            *byte = 0;
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretLease {
    pub selector: String,
    pub material: SecretMaterial,
    pub expires_at: i64,
}

#[async_trait::async_trait]
pub trait SecretBrokerPort: Send + Sync {
    async fn checkout(
        &self,
        admission: &CapabilityAdmission,
        selector: &str,
        observed_at: i64,
    ) -> Result<SecretLease, SecurityDenial>;
}

#[cfg(test)]
mod tests {
    use super::SecretMaterial;

    #[test]
    fn secret_material_debug_is_redacted() {
        let material = SecretMaterial::new(b"never-log-me".to_vec());
        let debug = format!("{material:?}");
        assert_eq!(debug, "SecretMaterial(REDACTED)");
        assert!(!debug.contains("never-log-me"));
    }
}
