//! Fail-closed enterprise execution-security composition.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use agentd_core::ports::{
    AttemptCapabilityPort, AuditActorKind, ExecutionAuditAppend, ExecutionAuditPort,
    ExecutionEvidenceLinks, ExecutionSandboxPort, ExecutionSnapshotLink, SecretBrokerPort,
    SecurityError, TaskLeasePort, TenantAuthorizationPort, WorkloadIdentityPort,
};
use agentd_core::types::{
    AuthenticatedWorkload, CapabilityAdmission, CapabilityToken, CapabilityValidationRequest,
    ExecutionSandboxProfile, ExecutionSecurityScope, PreparedSandbox, ProtectedAction,
    ProtectedResource, ProtectedResourceKind, SandboxCleanupRequest, SandboxExecuteRequest,
    SandboxExecution, SandboxPrepareRequest, SandboxTerminalReason, SecretCheckoutRequest,
    SecretLease, SecretSelector, SecurityAuditContext, SecurityDenialReason, TaskRunId,
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
}

impl SecurityProviderKind {
    pub const ALL: [Self; 8] = [
        Self::WorkloadIdentity,
        Self::ExecutionScope,
        Self::TenantAuthorization,
        Self::TaskLease,
        Self::AttemptCapability,
        Self::SecretBroker,
        Self::ExecutionSandbox,
        Self::ExecutionAudit,
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
}

struct OperationAdmissions {
    prepare: CapabilityAdmission,
    execute: CapabilityAdmission,
    secret: Option<CapabilityAdmission>,
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
        operation: EnterpriseWorkerOperation,
    ) -> Result<SandboxExecution, SecurityError> {
        validate_operation_times(&operation)?;
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
        self.run_stage(
            &workload,
            &scope,
            "lease",
            operation.observed_at,
            self.validate_lease(&scope, operation.observed_at).await,
        )
        .await?;
        let admissions = match self
            .admit_operation(&workload, &scope, &operation, secret_resource.as_ref())
            .await
        {
            Ok(admissions) => admissions,
            Err(error) => {
                return self
                    .deny(
                        &workload,
                        &scope,
                        "capability",
                        error,
                        operation.observed_at,
                    )
                    .await;
            }
        };
        let _secret_lease = match self.checkout_secret(&operation, &admissions).await {
            Ok(lease) => lease,
            Err(error) => {
                return self
                    .deny(&workload, &scope, "secret", error, operation.observed_at)
                    .await;
            }
        };
        let observed_at = operation.observed_at;
        let (prepared, execution) = self
            .run_sandbox(&workload, &scope, operation, admissions)
            .await?;
        self.finish_success(&workload, &scope, &prepared, observed_at)
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

    async fn admit_operation(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        operation: &EnterpriseWorkerOperation,
        secret_resource: Option<&ProtectedResource>,
    ) -> Result<OperationAdmissions, SecurityError> {
        let prepare = self
            .validate_capability(
                workload,
                scope,
                ProtectedAction::SandboxPrepare,
                &operation.scope_request.resource,
                operation.sandbox_prepare_token.clone(),
                operation.observed_at,
            )
            .await?;
        let execute = self
            .validate_capability(
                workload,
                scope,
                ProtectedAction::SandboxExecute,
                &operation.scope_request.resource,
                operation.sandbox_execute_token.clone(),
                operation.observed_at,
            )
            .await?;
        let secret = match (operation.secret.as_ref(), secret_resource) {
            (Some(secret), Some(resource)) => Some(
                self.validate_capability(
                    workload,
                    scope,
                    ProtectedAction::SecretCheckout,
                    resource,
                    secret.capability_token.clone(),
                    operation.observed_at,
                )
                .await?,
            ),
            (None, None) => None,
            _ => {
                return Err(SecurityError::Invalid(
                    "secret resource resolution mismatch".to_string(),
                ));
            }
        };
        Ok(OperationAdmissions {
            prepare,
            execute,
            secret,
        })
    }

    async fn checkout_secret(
        &self,
        operation: &EnterpriseWorkerOperation,
        admissions: &OperationAdmissions,
    ) -> Result<Option<SecretLease>, SecurityError> {
        match (operation.secret.as_ref(), admissions.secret.as_ref()) {
            (Some(secret), Some(admission)) => self
                .secret_broker
                .checkout_secret(&SecretCheckoutRequest {
                    admission: admission.clone(),
                    selector: secret.selector.clone(),
                    observed_at: operation.observed_at,
                })
                .await
                .map(Some),
            (None, None) => Ok(None),
            _ => Err(SecurityError::Invalid(
                "secret admission mismatch".to_string(),
            )),
        }
    }

    async fn run_sandbox(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        operation: EnterpriseWorkerOperation,
        admissions: OperationAdmissions,
    ) -> Result<(PreparedSandbox, SandboxExecution), SecurityError> {
        let prepared = match self
            .execution_sandbox
            .prepare_sandbox(&SandboxPrepareRequest {
                admission: admissions.prepare,
                profile: operation.profile,
            })
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                return self
                    .deny(workload, scope, "sandbox", error, operation.observed_at)
                    .await;
            }
        };
        let execution = match self
            .execution_sandbox
            .execute_sandbox(&SandboxExecuteRequest {
                admission: admissions.execute,
                sandbox: prepared.clone(),
                argv: operation.argv,
                env: operation.env,
                observed_at: operation.observed_at,
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
                        &prepared.sandbox_id,
                        operation.observed_at,
                    )
                    .await;
            }
        };
        Ok((prepared, execution))
    }

    async fn finish_success(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        prepared: &PreparedSandbox,
        observed_at: i64,
    ) -> Result<(), SecurityError> {
        if let Err(error) = self
            .audit_decision(workload, scope, "accepted", "complete", None, observed_at)
            .await
        {
            let _ = self
                .cleanup(
                    &prepared.sandbox_id,
                    observed_at,
                    SandboxTerminalReason::Failure,
                )
                .await;
            return Err(error);
        }
        self.cleanup(
            &prepared.sandbox_id,
            observed_at,
            SandboxTerminalReason::Success,
        )
        .await
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
        self.audit_decision(
            workload,
            scope,
            "denied",
            stage,
            error.denial_reason(),
            observed_at,
        )
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
        let payload = json!({
            "decision": "denied",
            "stage": stage,
            "reason": error.denial_reason().map(SecurityDenialReason::as_str),
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
        sandbox_id: &str,
        observed_at: i64,
    ) -> Result<T, SecurityError> {
        let audit_result = self
            .audit_decision(
                workload,
                scope,
                "denied",
                stage,
                error.denial_reason(),
                observed_at,
            )
            .await;
        let _ = self
            .cleanup(sandbox_id, observed_at, SandboxTerminalReason::Failure)
            .await;
        audit_result?;
        Err(error)
    }

    async fn cleanup(
        &self,
        sandbox_id: &str,
        observed_at: i64,
        terminal_reason: SandboxTerminalReason,
    ) -> Result<(), SecurityError> {
        self.execution_sandbox
            .cleanup_sandbox(&SandboxCleanupRequest {
                sandbox_id: sandbox_id.to_string(),
                observed_at,
                terminal_reason,
            })
            .await
    }

    async fn audit_decision(
        &self,
        workload: &AuthenticatedWorkload,
        scope: &ExecutionSecurityScope,
        decision: &str,
        stage: &str,
        reason: Option<SecurityDenialReason>,
        observed_at: i64,
    ) -> Result<(), SecurityError> {
        let payload = json!({
            "decision": decision,
            "stage": stage,
            "reason": reason.map(SecurityDenialReason::as_str),
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
        self.execution_audit
            .append_audit(&request)
            .await
            .map(|_| ())
            .map_err(|error| {
                SecurityError::Unavailable(format!("required security audit failed: {error}"))
            })
    }
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

fn validate_operation_times(operation: &EnterpriseWorkerOperation) -> Result<(), SecurityError> {
    if operation.observed_at < 0
        || operation.identity_request.observed_at != operation.observed_at
        || operation.scope_request.observed_at != operation.observed_at
    {
        return Err(SecurityError::Invalid(
            "enterprise security operation times must match".to_string(),
        ));
    }
    Ok(())
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
