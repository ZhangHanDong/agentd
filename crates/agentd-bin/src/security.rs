//! Fail-closed enterprise execution-security composition.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use agentd_core::ports::{
    AttemptCapabilityPort, AuditActorKind, Clock, ExecutionAuditAppend, ExecutionAuditPort,
    ExecutionEvidenceLinks, ExecutionSandboxPort, ExecutionSnapshotLink, PolicyRevocationPort,
    SecretBrokerPort, SecurityError, TaskLeasePort, TenantAuthorizationPort, WorkloadIdentityPort,
};
use agentd_core::types::{
    AuthenticatedWorkload, CapabilityAdmission, CapabilityToken, CapabilityValidationRequest,
    ExecutionSandboxProfile, ExecutionSecurityScope, PlacementCandidate, PlacementPolicy,
    ProtectedAction, ProtectedResource, ProtectedResourceKind, SandboxCleanupRequest,
    SandboxExecuteRequest, SandboxExecution, SandboxPrepareRequest, SandboxTerminalReason,
    SecretCheckoutRequest, SecretLease, SecretSelector, SecurityAuditContext, SecurityCheckpoint,
    SecurityDenialReason, SecurityEpochRequest, SecurityEpochStatus, TaskRunId,
    TenantAuthorization, TenantAuthorizationRequest, WorkloadIdentityRequest,
};
use agentd_surface::http::AuthConfig;
use clap::ValueEnum;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub enum SecurityRuntimeMode {
    #[default]
    Standalone,
    Enterprise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityProviderKind {
    WorkloadIdentity,
    ExecutionScope,
    TenantAuthorization,
    TaskLease,
    AttemptCapability,
    SecretBroker,
    ExecutionSandbox,
    ExecutionAudit,
    PolicyRevocation,
    TrustedClock,
}

impl SecurityProviderKind {
    pub const ALL: [Self; 10] = [
        Self::WorkloadIdentity,
        Self::ExecutionScope,
        Self::TenantAuthorization,
        Self::TaskLease,
        Self::AttemptCapability,
        Self::SecretBroker,
        Self::ExecutionSandbox,
        Self::ExecutionAudit,
        Self::PolicyRevocation,
        Self::TrustedClock,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::WorkloadIdentity => "workload_identity",
            Self::ExecutionScope => "execution_scope",
            Self::TenantAuthorization => "tenant_authorization",
            Self::TaskLease => "task_lease",
            Self::AttemptCapability => "attempt_capability",
            Self::SecretBroker => "secret_broker",
            Self::ExecutionSandbox => "execution_sandbox",
            Self::ExecutionAudit => "execution_audit",
            Self::PolicyRevocation => "policy_revocation",
            Self::TrustedClock => "trusted_clock",
        }
    }
}

impl fmt::Display for SecurityProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionScopeResolveRequest {
    pub execution_task_id: TaskRunId,
    pub resource: ProtectedResource,
    pub audit_context: SecurityAuditContext,
    pub observed_at: i64,
}

#[async_trait::async_trait]
pub trait ExecutionSecurityScopePort: Send + Sync {
    async fn resolve_execution_scope(
        &self,
        workload: &AuthenticatedWorkload,
        request: &ExecutionScopeResolveRequest,
    ) -> Result<ExecutionSecurityScope, SecurityError>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseSecretRequest {
    pub selector: SecretSelector,
    pub capability_token: CapabilityToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnterpriseWorkerOperation {
    pub identity_request: WorkloadIdentityRequest,
    pub scope_request: ExecutionScopeResolveRequest,
    pub sandbox_prepare_token: CapabilityToken,
    pub sandbox_execute_token: CapabilityToken,
    pub secret: Option<EnterpriseSecretRequest>,
    pub placement_policy: PlacementPolicy,
    pub placement_candidate: PlacementCandidate,
    pub profile: ExecutionSandboxProfile,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub observed_at: i64,
}

#[derive(Default)]
pub struct EnterpriseSecurityProviders {
    workload_identity: Option<Arc<dyn WorkloadIdentityPort>>,
    execution_scope: Option<Arc<dyn ExecutionSecurityScopePort>>,
    tenant_authorization: Option<Arc<dyn TenantAuthorizationPort>>,
    task_lease: Option<Arc<dyn TaskLeasePort>>,
    attempt_capability: Option<Arc<dyn AttemptCapabilityPort>>,
    secret_broker: Option<Arc<dyn SecretBrokerPort>>,
    execution_sandbox: Option<Arc<dyn ExecutionSandboxPort>>,
    execution_audit: Option<Arc<dyn ExecutionAuditPort>>,
    policy_revocation: Option<Arc<dyn PolicyRevocationPort>>,
    trusted_clock: Option<Arc<dyn Clock>>,
}

impl fmt::Debug for EnterpriseSecurityProviders {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnterpriseSecurityProviders")
            .field(
                "configured",
                &SecurityProviderKind::ALL
                    .into_iter()
                    .filter(|kind| self.has(*kind))
                    .map(SecurityProviderKind::as_str)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl EnterpriseSecurityProviders {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        workload_identity: Arc<dyn WorkloadIdentityPort>,
        execution_scope: Arc<dyn ExecutionSecurityScopePort>,
        tenant_authorization: Arc<dyn TenantAuthorizationPort>,
        task_lease: Arc<dyn TaskLeasePort>,
        attempt_capability: Arc<dyn AttemptCapabilityPort>,
        secret_broker: Arc<dyn SecretBrokerPort>,
        execution_sandbox: Arc<dyn ExecutionSandboxPort>,
        execution_audit: Arc<dyn ExecutionAuditPort>,
        policy_revocation: Arc<dyn PolicyRevocationPort>,
        trusted_clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            workload_identity: Some(workload_identity),
            execution_scope: Some(execution_scope),
            tenant_authorization: Some(tenant_authorization),
            task_lease: Some(task_lease),
            attempt_capability: Some(attempt_capability),
            secret_broker: Some(secret_broker),
            execution_sandbox: Some(execution_sandbox),
            execution_audit: Some(execution_audit),
            policy_revocation: Some(policy_revocation),
            trusted_clock: Some(trusted_clock),
        }
    }

    #[must_use]
    pub fn without(mut self, kind: SecurityProviderKind) -> Self {
        match kind {
            SecurityProviderKind::WorkloadIdentity => self.workload_identity = None,
            SecurityProviderKind::ExecutionScope => self.execution_scope = None,
            SecurityProviderKind::TenantAuthorization => self.tenant_authorization = None,
            SecurityProviderKind::TaskLease => self.task_lease = None,
            SecurityProviderKind::AttemptCapability => self.attempt_capability = None,
            SecurityProviderKind::SecretBroker => self.secret_broker = None,
            SecurityProviderKind::ExecutionSandbox => self.execution_sandbox = None,
            SecurityProviderKind::ExecutionAudit => self.execution_audit = None,
            SecurityProviderKind::PolicyRevocation => self.policy_revocation = None,
            SecurityProviderKind::TrustedClock => self.trusted_clock = None,
        }
        self
    }

    fn has(&self, kind: SecurityProviderKind) -> bool {
        match kind {
            SecurityProviderKind::WorkloadIdentity => self.workload_identity.is_some(),
            SecurityProviderKind::ExecutionScope => self.execution_scope.is_some(),
            SecurityProviderKind::TenantAuthorization => self.tenant_authorization.is_some(),
            SecurityProviderKind::TaskLease => self.task_lease.is_some(),
            SecurityProviderKind::AttemptCapability => self.attempt_capability.is_some(),
            SecurityProviderKind::SecretBroker => self.secret_broker.is_some(),
            SecurityProviderKind::ExecutionSandbox => self.execution_sandbox.is_some(),
            SecurityProviderKind::ExecutionAudit => self.execution_audit.is_some(),
            SecurityProviderKind::PolicyRevocation => self.policy_revocation.is_some(),
            SecurityProviderKind::TrustedClock => self.trusted_clock.is_some(),
        }
    }

    fn require_all(mut self) -> Result<EnterpriseSecurityPipeline, SecurityStartupError> {
        if let Some(missing) = SecurityProviderKind::ALL
            .into_iter()
            .find(|kind| !self.has(*kind))
        {
            return Err(SecurityStartupError::MissingProvider(missing));
        }
        Ok(EnterpriseSecurityPipeline {
            workload_identity: take_provider(&mut self.workload_identity),
            execution_scope: take_provider(&mut self.execution_scope),
            tenant_authorization: take_provider(&mut self.tenant_authorization),
            task_lease: take_provider(&mut self.task_lease),
            attempt_capability: take_provider(&mut self.attempt_capability),
            secret_broker: take_provider(&mut self.secret_broker),
            execution_sandbox: take_provider(&mut self.execution_sandbox),
            execution_audit: take_provider(&mut self.execution_audit),
            policy_revocation: take_provider(&mut self.policy_revocation),
            trusted_clock: take_provider(&mut self.trusted_clock),
        })
    }
}

fn take_provider<T: ?Sized>(provider: &mut Option<Arc<T>>) -> Arc<T> {
    provider
        .take()
        .expect("provider presence was checked before composition")
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SecurityStartupError {
    #[error("enterprise security startup rejected open_auth compatibility listener")]
    OpenAuth,
    #[error("enterprise security startup rejected audit_only_auth compatibility listener")]
    AuditOnlyAuth,
    #[error("enterprise security startup missing closed provider: {0}")]
    MissingProvider(SecurityProviderKind),
}

pub enum SecurityRuntime {
    Standalone,
    Enterprise(EnterpriseSecurityPipeline),
}

impl fmt::Debug for SecurityRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Standalone => f.write_str("SecurityRuntime::Standalone"),
            Self::Enterprise(_) => f.write_str("SecurityRuntime::Enterprise([PROVIDERS])"),
        }
    }
}

pub fn build_security_runtime(
    mode: SecurityRuntimeMode,
    auth: &AuthConfig,
    providers: Option<EnterpriseSecurityProviders>,
) -> Result<SecurityRuntime, SecurityStartupError> {
    match mode {
        SecurityRuntimeMode::Standalone => Ok(SecurityRuntime::Standalone),
        SecurityRuntimeMode::Enterprise => {
            if auth_is_open(auth) {
                return Err(SecurityStartupError::OpenAuth);
            }
            if auth.agent_token_mode == agentd_surface::http::AgentTokenMode::Audit {
                return Err(SecurityStartupError::AuditOnlyAuth);
            }
            let pipeline = providers.unwrap_or_default().require_all()?;
            Ok(SecurityRuntime::Enterprise(pipeline))
        }
    }
}

fn auth_is_open(auth: &AuthConfig) -> bool {
    auth.api_token
        .as_deref()
        .is_none_or(|token| token.trim().is_empty())
        && auth.agent_tokens.is_empty()
}

pub struct EnterpriseSecurityPipeline {
    workload_identity: Arc<dyn WorkloadIdentityPort>,
    execution_scope: Arc<dyn ExecutionSecurityScopePort>,
    tenant_authorization: Arc<dyn TenantAuthorizationPort>,
    task_lease: Arc<dyn TaskLeasePort>,
    attempt_capability: Arc<dyn AttemptCapabilityPort>,
    secret_broker: Arc<dyn SecretBrokerPort>,
    execution_sandbox: Arc<dyn ExecutionSandboxPort>,
    execution_audit: Arc<dyn ExecutionAuditPort>,
    policy_revocation: Arc<dyn PolicyRevocationPort>,
    trusted_clock: Arc<dyn Clock>,
}

struct SandboxTerminalGuard {
    execution_sandbox: Arc<dyn ExecutionSandboxPort>,
    execution_audit: Arc<dyn ExecutionAuditPort>,
    trusted_clock: Arc<dyn Clock>,
    workload: AuthenticatedWorkload,
    scope: ExecutionSecurityScope,
    sandbox_id: Option<String>,
}

impl SandboxTerminalGuard {
    fn new(
        execution_sandbox: Arc<dyn ExecutionSandboxPort>,
        execution_audit: Arc<dyn ExecutionAuditPort>,
        trusted_clock: Arc<dyn Clock>,
        workload: AuthenticatedWorkload,
        scope: ExecutionSecurityScope,
        sandbox_id: String,
    ) -> Self {
        Self {
            execution_sandbox,
            execution_audit,
            trusted_clock,
            workload,
            scope,
            sandbox_id: Some(sandbox_id),
        }
    }

    fn sandbox_id(&self) -> &str {
        self.sandbox_id
            .as_deref()
            .expect("terminal guard is armed until explicit cleanup completes")
    }

    async fn cleanup(
        &mut self,
        observed_at: i64,
        terminal_reason: SandboxTerminalReason,
    ) -> Result<(), SecurityError> {
        let request = SandboxCleanupRequest {
            sandbox_id: self.sandbox_id().to_string(),
            observed_at,
            terminal_reason,
        };
        let result = self.execution_sandbox.cleanup_sandbox(&request).await;
        self.sandbox_id = None;
        result
    }
}

impl Drop for SandboxTerminalGuard {
    fn drop(&mut self) {
        let Some(sandbox_id) = self.sandbox_id.take() else {
            return;
        };
        let execution_sandbox = Arc::clone(&self.execution_sandbox);
        let execution_audit = Arc::clone(&self.execution_audit);
        let workload = self.workload.clone();
        let scope = self.scope.clone();
        let observed_at = self.trusted_clock.now_unix().max(0);
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::error!(%sandbox_id, "cancelled sandbox requires recovery cleanup");
            return;
        };
        runtime.spawn(async move {
            let audit_result = append_security_decision(
                execution_audit.as_ref(),
                &workload,
                &scope,
                "cancelled",
                "sandbox",
                Some("operation_cancelled"),
                observed_at,
            )
            .await;
            let teardown_result = execution_sandbox
                .cleanup_sandbox(&SandboxCleanupRequest {
                    sandbox_id: sandbox_id.clone(),
                    observed_at,
                    terminal_reason: SandboxTerminalReason::Cancelled,
                })
                .await;
            if let Err(error) = terminal_result(None, audit_result, teardown_result) {
                tracing::error!(
                    %sandbox_id,
                    reason = security_error_reason(&error),
                    "cancelled sandbox terminal handling requires recovery"
                );
            }
        });
    }
}

impl fmt::Debug for EnterpriseSecurityPipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnterpriseSecurityPipeline")
            .field("providers", &"[CONFIGURED]")
            .finish()
    }
}

impl EnterpriseSecurityPipeline {
    pub async fn execute(
        &self,
        mut operation: EnterpriseWorkerOperation,
    ) -> Result<SandboxExecution, SecurityError> {
        let observed_at = self.trusted_clock.now_unix();
        operation.observed_at = observed_at.max(0);
        operation.identity_request.observed_at = operation.observed_at;
        operation.scope_request.observed_at = operation.observed_at;
        if observed_at < 0 {
            return self
                .deny_unresolved(
                    &operation.scope_request,
                    None,
                    "clock",
                    SecurityError::Unavailable("trusted clock returned invalid time".to_string()),
                )
                .await;
        }
        let workload = match self
            .workload_identity
            .authenticate_workload(&operation.identity_request)
            .await
        {
            Ok(workload) => workload,
            Err(error) => {
                return self
                    .deny_unresolved(&operation.scope_request, None, "identity", error)
                    .await;
            }
        };
        let scope = match self
            .execution_scope
            .resolve_execution_scope(&workload, &operation.scope_request)
            .await
        {
            Ok(scope) => scope,
            Err(error) => {
                return self
                    .deny_unresolved(&operation.scope_request, Some(&workload), "scope", error)
                    .await;
            }
        };
        if let Err(error) = validate_resolved_scope(&workload, &scope, &operation) {
            return self
                .deny(&workload, &scope, "scope", error, operation.observed_at)
                .await;
        }
        self.run_stage(
            &workload,
            &scope,
            "placement",
            operation.observed_at,
            operation
                .placement_policy
                .evaluate(&operation.placement_candidate)
                .map(|_| ())
                .map_err(SecurityError::Denied),
        )
        .await?;
        self.run_stage(
            &workload,
            &scope,
            "revocation",
            operation.observed_at,
            self.check_revocation_checkpoint(
                &scope,
                SecurityCheckpoint::Dispatch,
                operation.observed_at,
            )
            .await
            .map(|_| ()),
        )
        .await?;
        let secret_resource = secret_resource(&operation);
        self.run_stage(
            &workload,
            &scope,
            "authorization",
            operation.observed_at,
            self.authorize_operation(&workload, &scope, &operation, secret_resource.as_ref())
                .await,
        )
        .await?;
        let (_secret_lease, observed_at) = self
            .checkout_secret(
                &workload,
                &scope,
                &operation,
                secret_resource.as_ref(),
                operation.observed_at,
            )
            .await?;
        let (mut terminal_guard, execution, observed_at) = self
            .run_sandbox(&workload, &scope, operation, observed_at)
            .await?;
        self.finish_success(&workload, &scope, &mut terminal_guard, observed_at)
            .await?;
        Ok(execution)
    }

    async fn authorize_operation(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        operation: &EnterpriseWorkerOperation,
        secret_resource: Option<&ProtectedResource>,
    ) -> Result<(), SecurityError> {
        let requests = authorization_requests(
            workload,
            scope,
            &operation.scope_request.resource,
            secret_resource,
        );
        for request in &requests {
            let authorization = self.tenant_authorization.authorize_tenant(request).await?;
            validate_authorization(&authorization, request, operation.observed_at)?;
        }
        Ok(())
    }

    async fn validate_lease(
        &self,
        scope: &ExecutionSecurityScope,
        observed_at: i64,
    ) -> Result<(), SecurityError> {
        let lease = self
            .task_lease
            .validate_claim(&scope.task_lease_claim, observed_at)
            .await
            .map_err(|error| lease_security_error(&error))?;
        if lease.claim() != scope.task_lease_claim || observed_at >= lease.expires_at {
            return Err(SecurityError::Denied(SecurityDenialReason::LeaseRejected));
        }
        Ok(())
    }

    async fn checkout_secret(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        operation: &EnterpriseWorkerOperation,
        secret_resource: Option<&ProtectedResource>,
        previous_observed_at: i64,
    ) -> Result<(Option<SecretLease>, i64), SecurityError> {
        let (Some(secret), Some(resource)) = (operation.secret.as_ref(), secret_resource) else {
            if operation.secret.is_none() && secret_resource.is_none() {
                return Ok((None, previous_observed_at));
            }
            return self
                .deny(
                    workload,
                    scope,
                    "secret",
                    SecurityError::Invalid("secret resource resolution mismatch".to_string()),
                    previous_observed_at,
                )
                .await;
        };
        let observed_at = match self.fresh_observed_at(previous_observed_at) {
            Ok(observed_at) => observed_at,
            Err(error) => {
                return self
                    .deny(workload, scope, "clock", error, previous_observed_at)
                    .await;
            }
        };
        self.run_stage(
            workload,
            scope,
            "revocation",
            observed_at,
            self.check_revocation_checkpoint(scope, SecurityCheckpoint::LeaseRenewal, observed_at)
                .await
                .map(|_| ()),
        )
        .await?;
        self.run_stage(
            workload,
            scope,
            "lease",
            observed_at,
            self.validate_lease(scope, observed_at).await,
        )
        .await?;
        let admission = match self
            .validate_capability(
                workload,
                scope,
                ProtectedAction::SecretCheckout,
                resource,
                secret.capability_token.clone(),
                observed_at,
            )
            .await
        {
            Ok(admission) => admission,
            Err(error) => {
                return self
                    .deny(workload, scope, "capability", error, observed_at)
                    .await;
            }
        };
        match self
            .secret_broker
            .checkout_secret(&SecretCheckoutRequest {
                admission,
                selector: secret.selector.clone(),
                observed_at,
            })
            .await
        {
            Ok(lease) => Ok((Some(lease), observed_at)),
            Err(error) => {
                self.deny(workload, scope, "secret", error, observed_at)
                    .await
            }
        }
    }

    async fn run_sandbox(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        operation: EnterpriseWorkerOperation,
        previous_observed_at: i64,
    ) -> Result<(SandboxTerminalGuard, SandboxExecution, i64), SecurityError> {
        let (prepare_admission, prepare_observed_at) = self
            .admit_sandbox_prepare(workload, scope, &operation, previous_observed_at)
            .await?;
        let prepared = match self
            .execution_sandbox
            .prepare_sandbox(&SandboxPrepareRequest {
                admission: prepare_admission,
                profile: operation.profile,
            })
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                return self
                    .deny(workload, scope, "sandbox", error, prepare_observed_at)
                    .await;
            }
        };
        let mut terminal_guard = SandboxTerminalGuard::new(
            Arc::clone(&self.execution_sandbox),
            Arc::clone(&self.execution_audit),
            Arc::clone(&self.trusted_clock),
            workload.clone(),
            scope.clone(),
            prepared.sandbox_id.clone(),
        );
        let (execute_admission, execute_observed_at) = self
            .admit_sandbox_execute(
                workload,
                scope,
                &operation.scope_request.resource,
                operation.sandbox_execute_token,
                &mut terminal_guard,
                prepare_observed_at,
            )
            .await?;
        let execution = match self
            .execution_sandbox
            .execute_sandbox(&SandboxExecuteRequest {
                admission: execute_admission,
                sandbox: prepared.clone(),
                argv: operation.argv,
                env: operation.env,
                observed_at: execute_observed_at,
            })
            .await
        {
            Ok(execution) => execution,
            Err(error) => {
                return self
                    .deny_and_teardown(
                        workload,
                        scope,
                        "sandbox",
                        error,
                        &mut terminal_guard,
                        execute_observed_at,
                    )
                    .await;
            }
        };
        Ok((terminal_guard, execution, execute_observed_at))
    }

    async fn admit_sandbox_prepare(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        operation: &EnterpriseWorkerOperation,
        previous_observed_at: i64,
    ) -> Result<(CapabilityAdmission, i64), SecurityError> {
        let observed_at = match self.fresh_observed_at(previous_observed_at) {
            Ok(observed_at) => observed_at,
            Err(error) => {
                return self
                    .deny(workload, scope, "clock", error, previous_observed_at)
                    .await;
            }
        };
        self.run_stage(
            workload,
            scope,
            "revocation",
            observed_at,
            self.check_revocation_checkpoint(scope, SecurityCheckpoint::LeaseRenewal, observed_at)
                .await
                .map(|_| ()),
        )
        .await?;
        self.run_stage(
            workload,
            scope,
            "lease",
            observed_at,
            self.validate_lease(scope, observed_at).await,
        )
        .await?;
        match self
            .validate_capability(
                workload,
                scope,
                ProtectedAction::SandboxPrepare,
                &operation.scope_request.resource,
                operation.sandbox_prepare_token.clone(),
                observed_at,
            )
            .await
        {
            Ok(admission) => Ok((admission, observed_at)),
            Err(error) => {
                self.deny(workload, scope, "capability", error, observed_at)
                    .await
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn admit_sandbox_execute(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        resource: &ProtectedResource,
        token: CapabilityToken,
        terminal_guard: &mut SandboxTerminalGuard,
        previous_observed_at: i64,
    ) -> Result<(CapabilityAdmission, i64), SecurityError> {
        let observed_at = match self.fresh_observed_at(previous_observed_at) {
            Ok(observed_at) => observed_at,
            Err(error) => {
                return self
                    .deny_and_teardown(
                        workload,
                        scope,
                        "clock",
                        error,
                        terminal_guard,
                        previous_observed_at,
                    )
                    .await;
            }
        };
        if let Err(error) = self
            .check_revocation_checkpoint(scope, SecurityCheckpoint::LeaseRenewal, observed_at)
            .await
        {
            return self
                .deny_and_teardown(
                    workload,
                    scope,
                    "revocation",
                    error,
                    terminal_guard,
                    observed_at,
                )
                .await;
        }
        if let Err(error) = self.validate_lease(scope, observed_at).await {
            return self
                .deny_and_teardown(workload, scope, "lease", error, terminal_guard, observed_at)
                .await;
        }
        match self
            .validate_capability(
                workload,
                scope,
                ProtectedAction::SandboxExecute,
                resource,
                token,
                observed_at,
            )
            .await
        {
            Ok(admission) => Ok((admission, observed_at)),
            Err(error) => {
                self.deny_and_teardown(
                    workload,
                    scope,
                    "capability",
                    error,
                    terminal_guard,
                    observed_at,
                )
                .await
            }
        }
    }

    fn fresh_observed_at(&self, previous_observed_at: i64) -> Result<i64, SecurityError> {
        let observed_at = self.trusted_clock.now_unix();
        if observed_at < 0 || observed_at < previous_observed_at {
            return Err(SecurityError::Unavailable(
                "trusted clock moved backwards or returned invalid time".to_string(),
            ));
        }
        Ok(observed_at)
    }

    pub async fn check_revocation_checkpoint(
        &self,
        scope: &ExecutionSecurityScope,
        checkpoint: SecurityCheckpoint,
        observed_at: i64,
    ) -> Result<SecurityEpochStatus, SecurityError> {
        if observed_at < 0 || observed_at >= scope.valid_until {
            return Err(SecurityError::Denied(SecurityDenialReason::LeaseRejected));
        }
        let status = self
            .policy_revocation
            .check_security_epoch(&SecurityEpochRequest {
                checkpoint,
                organization_ref: scope.organization_ref.clone(),
                project_ref: scope.project_ref.clone(),
                execution_snapshot_ref: scope.execution_snapshot_ref.clone(),
                pinned_epoch: scope.policy_revocation_epoch,
                observed_at,
            })
            .await?;
        if status.observed_at > observed_at {
            return Err(SecurityError::Unavailable(
                "policy revocation authority returned future state".to_string(),
            ));
        }
        status
            .validate_pinned_epoch(scope.policy_revocation_epoch)
            .map_err(SecurityError::Denied)?;
        Ok(status)
    }

    async fn finish_success(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        terminal_guard: &mut SandboxTerminalGuard,
        observed_at: i64,
    ) -> Result<(), SecurityError> {
        let audit_result = self
            .audit_decision(workload, scope, "accepted", "complete", None, observed_at)
            .await;
        let terminal_reason = if audit_result.is_ok() {
            SandboxTerminalReason::Success
        } else {
            SandboxTerminalReason::Failure
        };
        let teardown_result = self
            .cleanup(terminal_guard, observed_at, terminal_reason)
            .await;
        terminal_result(None, audit_result, teardown_result)
    }

    async fn run_stage<T>(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        stage: &'static str,
        observed_at: i64,
        result: Result<T, SecurityError>,
    ) -> Result<T, SecurityError> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => self.deny(workload, scope, stage, error, observed_at).await,
        }
    }

    async fn validate_capability(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        action: ProtectedAction,
        resource: &ProtectedResource,
        token: CapabilityToken,
        observed_at: i64,
    ) -> Result<CapabilityAdmission, SecurityError> {
        let admission = self
            .attempt_capability
            .validate_capability(&CapabilityValidationRequest {
                token,
                scope: scope.clone(),
                action,
                resource: resource.clone(),
                observed_at,
            })
            .await?;
        if admission.workload != *workload
            || admission.scope != *scope
            || admission.action != action
            || admission.resource != *resource
            || observed_at < admission.issued_at
            || observed_at >= admission.expires_at
        {
            return Err(SecurityError::Denied(
                SecurityDenialReason::CapabilityScopeMismatch,
            ));
        }
        Ok(admission)
    }

    async fn deny<T>(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        stage: &'static str,
        error: SecurityError,
        observed_at: i64,
    ) -> Result<T, SecurityError> {
        let reason = security_error_reason(&error);
        self.audit_decision(workload, scope, "denied", stage, Some(reason), observed_at)
            .await?;
        Err(error)
    }

    async fn deny_unresolved<T>(
        &self,
        request: &ExecutionScopeResolveRequest,
        workload: Option<&AuthenticatedWorkload>,
        stage: &'static str,
        error: SecurityError,
    ) -> Result<T, SecurityError> {
        let reason = security_error_reason(&error);
        let payload = json!({
            "decision": "denied",
            "stage": stage,
            "reason": reason,
            "execution_task_id": request.execution_task_id,
        });
        let audit_id = agentd_core::types::AuditEventId::new();
        let audit = ExecutionAuditAppend {
            id: audit_id.clone(),
            idempotency_scope: format!("enterprise-security:{}", request.execution_task_id),
            idempotency_key: audit_id.to_string(),
            event_type: "execution.security_denied".to_string(),
            actor_kind: workload.map_or(AuditActorKind::System, |_| AuditActorKind::Worker),
            actor_ref: workload.map_or_else(
                || "unverified_workload".to_string(),
                |authenticated| authenticated.spiffe_uri.clone(),
            ),
            payload_sha256: sha256_json(&payload)?,
            payload,
            links: unresolved_audit_links(request, workload),
            execution_artifact_id: None,
            occurred_at: request.observed_at,
        };
        self.execution_audit
            .append_audit(&audit)
            .await
            .map_err(|audit_error| {
                SecurityError::Unavailable(format!(
                    "required unresolved security audit failed: {audit_error}"
                ))
            })?;
        Err(error)
    }

    async fn deny_and_teardown<T>(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        stage: &'static str,
        error: SecurityError,
        terminal_guard: &mut SandboxTerminalGuard,
        observed_at: i64,
    ) -> Result<T, SecurityError> {
        let reason = security_error_reason(&error);
        let audit_result = self
            .audit_decision(workload, scope, "denied", stage, Some(reason), observed_at)
            .await;
        let teardown_result = self
            .cleanup(terminal_guard, observed_at, SandboxTerminalReason::Failure)
            .await;
        terminal_result(Some(error), audit_result, teardown_result)?;
        unreachable!("a denied operation always carries a primary error")
    }

    async fn cleanup(
        &self,
        terminal_guard: &mut SandboxTerminalGuard,
        observed_at: i64,
        terminal_reason: SandboxTerminalReason,
    ) -> Result<(), SecurityError> {
        terminal_guard.cleanup(observed_at, terminal_reason).await
    }

    async fn audit_decision(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        decision: &str,
        stage: &str,
        reason: Option<&str>,
        observed_at: i64,
    ) -> Result<(), SecurityError> {
        append_security_decision(
            self.execution_audit.as_ref(),
            workload,
            scope,
            decision,
            stage,
            reason,
            observed_at,
        )
        .await
    }
}

#[allow(clippy::too_many_arguments)]
async fn append_security_decision(
    execution_audit: &dyn ExecutionAuditPort,
    workload: &AuthenticatedWorkload,
    scope: &ExecutionSecurityScope,
    decision: &str,
    stage: &str,
    reason: Option<&str>,
    observed_at: i64,
) -> Result<(), SecurityError> {
    let payload = json!({
        "decision": decision,
        "stage": stage,
        "reason": reason,
        "execution_task_id": scope.task_lease_claim.execution_task_id,
        "worker_incarnation_id": scope.worker_incarnation_id,
        "fencing_token": scope.task_lease_claim.fencing_token,
    });
    let audit_id = agentd_core::types::AuditEventId::new();
    let request = ExecutionAuditAppend {
        id: audit_id.clone(),
        idempotency_scope: format!(
            "enterprise-security:{}",
            scope.task_lease_claim.execution_task_id
        ),
        idempotency_key: audit_id.to_string(),
        event_type: format!("execution.security_{decision}"),
        actor_kind: AuditActorKind::Worker,
        actor_ref: workload.spiffe_uri.clone(),
        payload_sha256: sha256_json(&payload)?,
        payload,
        links: audit_links(scope),
        execution_artifact_id: None,
        occurred_at: observed_at,
    };
    execution_audit
        .append_audit(&request)
        .await
        .map(|_| ())
        .map_err(|error| {
            SecurityError::Unavailable(format!("required security audit failed: {error}"))
        })
}

fn secret_resource(operation: &EnterpriseWorkerOperation) -> Option<ProtectedResource> {
    operation.secret.as_ref().map(|secret| ProtectedResource {
        organization_ref: operation.scope_request.resource.organization_ref.clone(),
        project_ref: operation.scope_request.resource.project_ref.clone(),
        execution_snapshot_ref: operation
            .scope_request
            .resource
            .execution_snapshot_ref
            .clone(),
        kind: ProtectedResourceKind::Secret(secret.selector.clone()),
    })
}

fn validate_resolved_scope(
    workload: &AuthenticatedWorkload,
    scope: &ExecutionSecurityScope,
    operation: &EnterpriseWorkerOperation,
) -> Result<(), SecurityError> {
    scope
        .authorize_resource(&operation.scope_request.resource)
        .map_err(SecurityError::Denied)?;
    if operation.scope_request.execution_task_id != scope.task_lease_claim.execution_task_id
        || workload.worker_incarnation_id.as_ref() != Some(&scope.worker_incarnation_id)
        || scope.task_lease_claim.worker_incarnation_id != scope.worker_incarnation_id
        || operation.observed_at < workload.not_before
        || operation.observed_at >= workload.not_after
        || operation.observed_at >= scope.valid_until
        || workload.trust_domain != operation.placement_candidate.worker_trust_domain
        || scope.sandbox_profile_id != operation.profile.profile_id
        || scope.egress_profile_id != operation.placement_candidate.egress_profile_id
        || operation.profile.image_digest != operation.placement_candidate.image_digest
        || operation.profile.tenant_cache_namespace
            != operation.placement_candidate.tenant_cache_namespace
        || !matches!(
            operation.scope_request.resource.kind,
            ProtectedResourceKind::Execution
        )
    {
        return Err(SecurityError::Denied(SecurityDenialReason::LeaseRejected));
    }
    Ok(())
}

fn authorization_requests(
    workload: &AuthenticatedWorkload,
    scope: &ExecutionSecurityScope,
    execution_resource: &ProtectedResource,
    secret_resource: Option<&ProtectedResource>,
) -> Vec<TenantAuthorizationRequest> {
    let mut requests = vec![
        TenantAuthorizationRequest {
            workload: workload.clone(),
            scope: scope.clone(),
            action: ProtectedAction::SandboxPrepare,
            resource: execution_resource.clone(),
        },
        TenantAuthorizationRequest {
            workload: workload.clone(),
            scope: scope.clone(),
            action: ProtectedAction::SandboxExecute,
            resource: execution_resource.clone(),
        },
    ];
    if let Some(resource) = secret_resource {
        requests.push(TenantAuthorizationRequest {
            workload: workload.clone(),
            scope: scope.clone(),
            action: ProtectedAction::SecretCheckout,
            resource: resource.clone(),
        });
    }
    requests
}

fn validate_authorization(
    authorization: &TenantAuthorization,
    request: &TenantAuthorizationRequest,
    observed_at: i64,
) -> Result<(), SecurityError> {
    if authorization.workload != request.workload
        || authorization.scope != request.scope
        || authorization.action != request.action
        || authorization.resource != request.resource
        || observed_at < authorization.authorized_at
        || observed_at >= authorization.expires_at
    {
        return Err(SecurityError::Denied(SecurityDenialReason::ActionDenied));
    }
    Ok(())
}

fn lease_security_error(error: &agentd_core::ports::TaskLeaseError) -> SecurityError {
    if error.rejection_reason().is_some() {
        SecurityError::Denied(SecurityDenialReason::LeaseRejected)
    } else {
        SecurityError::Unavailable(format!("task lease validation unavailable: {error}"))
    }
}

fn security_error_reason(error: &SecurityError) -> &'static str {
    match error {
        SecurityError::Denied(reason) => reason.as_str(),
        SecurityError::Invalid(_) => "invalid_security_request",
        SecurityError::Unavailable(_) => "security_provider_unavailable",
    }
}

fn terminal_result(
    primary: Option<SecurityError>,
    audit_result: Result<(), SecurityError>,
    teardown_result: Result<(), SecurityError>,
) -> Result<(), SecurityError> {
    let audit_error = audit_result.err();
    let teardown_error = teardown_result.err();
    let failure_count = usize::from(primary.is_some())
        + usize::from(audit_error.is_some())
        + usize::from(teardown_error.is_some());
    if failure_count > 1 {
        let mut reasons = Vec::with_capacity(failure_count);
        if let Some(error) = primary.as_ref() {
            reasons.push(format!("primary={}", security_error_reason(error)));
        }
        if let Some(error) = audit_error.as_ref() {
            reasons.push(format!("audit={}", security_error_reason(error)));
        }
        if let Some(error) = teardown_error.as_ref() {
            reasons.push(format!("teardown={}", security_error_reason(error)));
        }
        return Err(SecurityError::Unavailable(format!(
            "compound terminal security failure: {}",
            reasons.join("; ")
        )));
    }
    if let Some(error) = teardown_error.or(audit_error).or(primary) {
        Err(error)
    } else {
        Ok(())
    }
}

fn audit_links(scope: &ExecutionSecurityScope) -> ExecutionEvidenceLinks {
    ExecutionEvidenceLinks {
        execution_run_id: scope.audit_context.execution_run_id.clone(),
        execution_task_id: Some(scope.task_lease_claim.execution_task_id.clone()),
        runtime_session_id: None,
        runtime_attempt_id: None,
        worker_incarnation_id: Some(scope.worker_incarnation_id.clone()),
        snapshot: ExecutionSnapshotLink {
            authority_key: scope.authority_key.to_string(),
            resource_kind: "execution_snapshot".to_string(),
            resource_id: scope.execution_snapshot_ref.resource_id().to_string(),
            resource_version: scope.execution_snapshot_ref.resource_version().to_string(),
            content_sha256: scope.audit_context.snapshot_content_sha256.clone(),
        },
        target_repository_id: scope.audit_context.target_repository_id.clone(),
        target_base_commit: scope.audit_context.target_base_commit.clone(),
    }
}

fn unresolved_audit_links(
    request: &ExecutionScopeResolveRequest,
    workload: Option<&AuthenticatedWorkload>,
) -> ExecutionEvidenceLinks {
    ExecutionEvidenceLinks {
        execution_run_id: request.audit_context.execution_run_id.clone(),
        execution_task_id: Some(request.execution_task_id.clone()),
        runtime_session_id: None,
        runtime_attempt_id: None,
        worker_incarnation_id: workload
            .and_then(|authenticated| authenticated.worker_incarnation_id.clone()),
        snapshot: ExecutionSnapshotLink {
            authority_key: request
                .resource
                .execution_snapshot_ref
                .authority_key()
                .to_string(),
            resource_kind: "execution_snapshot".to_string(),
            resource_id: request
                .resource
                .execution_snapshot_ref
                .resource_id()
                .to_string(),
            resource_version: request
                .resource
                .execution_snapshot_ref
                .resource_version()
                .to_string(),
            content_sha256: request.audit_context.snapshot_content_sha256.clone(),
        },
        target_repository_id: request.audit_context.target_repository_id.clone(),
        target_base_commit: request.audit_context.target_base_commit.clone(),
    }
}

fn sha256_json(value: &Value) -> Result<String, SecurityError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| SecurityError::Invalid(format!("invalid security audit: {error}")))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
