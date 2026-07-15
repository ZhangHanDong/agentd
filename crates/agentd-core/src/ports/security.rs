//! Independent fail-closed ports for enterprise execution security.

use thiserror::Error;

use crate::types::{
    AttemptCapabilityId, AuthenticatedWorkload, CapabilityAdmission, CapabilityIssueRequest,
    CapabilityToken, CapabilityValidationRequest, PreparedSandbox, SandboxCleanupRequest,
    SandboxExecuteRequest, SandboxExecution, SandboxPrepareRequest, SecretCheckoutRequest,
    SecretLease, TenantAuthorization, TenantAuthorizationRequest, WorkloadIdentityRequest,
};

use crate::types::SecurityDenialReason;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecurityError {
    #[error("security request denied: {0}")]
    Denied(SecurityDenialReason),
    #[error("invalid security request: {0}")]
    Invalid(String),
    #[error("execution security unavailable: {0}")]
    Unavailable(String),
}

impl SecurityError {
    #[must_use]
    pub const fn denial_reason(&self) -> Option<SecurityDenialReason> {
        match self {
            Self::Denied(reason) => Some(*reason),
            Self::Invalid(_) | Self::Unavailable(_) => None,
        }
    }
}

#[async_trait::async_trait]
pub trait WorkloadIdentityPort: Send + Sync {
    async fn authenticate_workload(
        &self,
        request: &WorkloadIdentityRequest,
    ) -> Result<AuthenticatedWorkload, SecurityError>;
}

#[async_trait::async_trait]
pub trait TenantAuthorizationPort: Send + Sync {
    async fn authorize_tenant(
        &self,
        request: &TenantAuthorizationRequest,
    ) -> Result<TenantAuthorization, SecurityError>;
}

#[async_trait::async_trait]
pub trait AttemptCapabilityPort: Send + Sync {
    async fn issue_capability(
        &self,
        request: &CapabilityIssueRequest,
    ) -> Result<(CapabilityToken, CapabilityAdmission), SecurityError>;

    async fn validate_capability(
        &self,
        request: &CapabilityValidationRequest,
    ) -> Result<CapabilityAdmission, SecurityError>;

    async fn revoke_capability(
        &self,
        id: &AttemptCapabilityId,
        observed_at: i64,
    ) -> Result<(), SecurityError>;
}

#[async_trait::async_trait]
pub trait SecretBrokerPort: Send + Sync {
    async fn checkout_secret(
        &self,
        request: &SecretCheckoutRequest,
    ) -> Result<SecretLease, SecurityError>;
}

#[async_trait::async_trait]
pub trait ExecutionSandboxPort: Send + Sync {
    async fn prepare_sandbox(
        &self,
        request: &SandboxPrepareRequest,
    ) -> Result<PreparedSandbox, SecurityError>;

    async fn execute_sandbox(
        &self,
        request: &SandboxExecuteRequest,
    ) -> Result<SandboxExecution, SecurityError>;

    async fn cleanup_sandbox(&self, request: &SandboxCleanupRequest) -> Result<(), SecurityError>;
}
