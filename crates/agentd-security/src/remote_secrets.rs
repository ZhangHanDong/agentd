//! Authenticated remote secret-broker transport with immutable scope validation.

use std::fmt;

use agentd_core::ports::{SecretBrokerPort, SecurityError};
use agentd_core::types::{
    AttemptCapabilityId, OrganizationRef, ProjectExecutionSnapshotRef, ProjectRef, ProtectedAction,
    ProtectedResourceKind, SecretCheckoutRequest, SecretLease, SecretMaterial, SecretSelector,
    SecurityDenialReason, TaskLeaseClaim, WorkerIncarnationId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSecretScope {
    pub selector: SecretSelector,
    pub organization_ref: OrganizationRef,
    pub project_ref: ProjectRef,
    pub execution_snapshot_ref: ProjectExecutionSnapshotRef,
    pub capability_id: AttemptCapabilityId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub task_lease_claim: TaskLeaseClaim,
    pub requested_expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSecretRequest {
    pub scope: RemoteSecretScope,
    pub observed_at: i64,
}

impl RemoteSecretRequest {
    pub fn from_checkout(request: &SecretCheckoutRequest) -> Result<Self, SecurityError> {
        let admission = &request.admission;
        let valid = admission.action == ProtectedAction::SecretCheckout
            && request.observed_at >= admission.issued_at
            && request.observed_at < admission.expires_at
            && request.observed_at < admission.scope.valid_until
            && request.observed_at < admission.workload.not_after
            && admission.workload.worker_incarnation_id.as_ref()
                == Some(&admission.scope.worker_incarnation_id)
            && matches!(
                &admission.resource.kind,
                ProtectedResourceKind::Secret(selector) if selector == &request.selector
            )
            && admission
                .scope
                .authorize_resource(&admission.resource)
                .is_ok();
        if !valid {
            return Err(secret_unavailable());
        }
        let requested_expires_at = admission
            .expires_at
            .min(admission.scope.valid_until)
            .min(admission.workload.not_after);
        Ok(Self {
            scope: RemoteSecretScope {
                selector: request.selector.clone(),
                organization_ref: admission.scope.organization_ref.clone(),
                project_ref: admission.scope.project_ref.clone(),
                execution_snapshot_ref: admission.scope.execution_snapshot_ref.clone(),
                capability_id: admission.id.clone(),
                worker_incarnation_id: admission.scope.worker_incarnation_id.clone(),
                task_lease_claim: admission.scope.task_lease_claim.clone(),
                requested_expires_at,
            },
            observed_at: request.observed_at,
        })
    }
}

pub struct RemoteSecretResponse {
    pub scope: RemoteSecretScope,
    pub material: SecretMaterial,
    pub expires_at: i64,
}

impl fmt::Debug for RemoteSecretResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteSecretResponse")
            .field("scope", &self.scope)
            .field("material", &"[REDACTED]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretBrokerTransportError {
    TimedOut,
    Unavailable,
    Rejected,
    Malformed,
}

#[async_trait::async_trait]
pub trait SecretBrokerTransport: Send + Sync {
    async fn checkout(
        &self,
        request: &RemoteSecretRequest,
    ) -> Result<RemoteSecretResponse, SecretBrokerTransportError>;
}

pub struct RemoteSecretBroker<T> {
    transport: T,
}

impl<T> RemoteSecretBroker<T> {
    #[must_use]
    pub const fn new(transport: T) -> Self {
        Self { transport }
    }

    #[must_use]
    pub const fn transport(&self) -> &T {
        &self.transport
    }
}

impl<T> fmt::Debug for RemoteSecretBroker<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteSecretBroker")
            .field("transport", &std::any::type_name::<T>())
            .finish()
    }
}

#[async_trait::async_trait]
impl<T> SecretBrokerPort for RemoteSecretBroker<T>
where
    T: SecretBrokerTransport,
{
    async fn checkout_secret(
        &self,
        request: &SecretCheckoutRequest,
    ) -> Result<SecretLease, SecurityError> {
        let remote_request = RemoteSecretRequest::from_checkout(request)?;
        let response = self
            .transport
            .checkout(&remote_request)
            .await
            .map_err(|_| secret_unavailable())?;
        if response.scope != remote_request.scope
            || response.expires_at <= request.observed_at
            || response.expires_at > remote_request.scope.requested_expires_at
            || response.material.expose_secret().is_empty()
        {
            return Err(secret_unavailable());
        }
        Ok(SecretLease {
            selector: request.selector.clone(),
            material: response.material,
            expires_at: response.expires_at,
        })
    }
}

fn secret_unavailable() -> SecurityError {
    SecurityError::Denied(SecurityDenialReason::SecretUnavailable)
}
