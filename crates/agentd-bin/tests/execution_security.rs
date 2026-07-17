use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentd_bin::security::{
    EnterpriseSecretRequest, EnterpriseSecurityProviders, EnterpriseWorkerOperation,
    ExecutionScopeResolveRequest, ExecutionSecurityScopePort, SecurityProviderKind,
    SecurityRuntime, SecurityRuntimeMode, build_security_runtime,
    validate_enterprise_control_plane_auth,
};
use agentd_bin::{AgentdCli, DaemonConfig, daemon};
use agentd_core::ports::{
    AttemptCapabilityPort, AuditPage, AuditReadRequest, Clock, ContentRedactionPort,
    ExecutionAuditAppend, ExecutionAuditPort, ExecutionAuditRecord, ExecutionEvidenceError,
    ExecutionSandboxPort, PlacementAdmissionPort, PolicyRevocationPort, SecretBrokerPort,
    SecurityError, TaskLeaseCloseRequest, TaskLeaseDispatchRequest, TaskLeaseError, TaskLeasePort,
    TaskLeaseRenewRequest, TenantAuthorizationPort, WorkloadIdentityPort,
};
use agentd_core::test_support::FixedClock;
use agentd_core::types::{
    AttemptCapabilityId, AuthenticatedWorkload, AuthorityKey, CapabilityAdmission,
    CapabilityIssueRequest, CapabilityToken, CapabilityValidationRequest, DataClassification,
    EgressPolicy, ExecutionSandboxProfile, ExecutionSecurityScope, FencingToken, LeaseId,
    LeaseStatus, OciSandboxRuntime, OrganizationRef, PlacementAdmission, PlacementCandidate,
    PlacementPolicy, PreparedSandbox, ProjectExecutionSnapshotRef, ProjectRef, ProtectedResource,
    ProtectedResourceKind, RbacPolicyVersionRef, RunId, SandboxCacheSharing, SandboxCleanupRequest,
    SandboxExecuteRequest, SandboxExecution, SandboxLimits, SandboxLinuxCapabilities, SandboxMount,
    SandboxMountAccess, SandboxPrepareRequest, SandboxPrivilegeEscalation, SandboxRootFilesystem,
    SandboxWorkspace, SecretCheckoutRequest, SecretLease, SecretMaterial, SecretSelector,
    SecurityAuditContext, SecurityCheckpoint, SecurityDenialReason, SecurityEpochRequest,
    SecurityEpochStatus, TaskLeaseClaim, TaskLeaseGrant, TaskRunId, TenantAuthorization,
    TenantAuthorizationRequest, WorkerId, WorkerIncarnationId, WorkloadIdentityRequest,
    WorkloadRole,
};
use agentd_surface::http::{AgentTokenMode, AuthConfig};
use clap::Parser;
use serde_json::Value;
use tokio::sync::Notify;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureStage {
    Identity,
    Scope,
    Placement,
    Revocation,
    Authorization,
    Lease,
    Capability,
    Secret,
    SandboxPrepare,
    SandboxExecute,
    Redaction,
    Audit,
    Teardown,
}

#[derive(Debug)]
struct RecordingPorts {
    failures: Vec<FailureStage>,
    unavailable: Option<FailureStage>,
    calls: Mutex<Vec<&'static str>>,
    audit_payloads: Mutex<Vec<Value>>,
    observed_at: i64,
    block_sandbox_execute: bool,
    sandbox_execute_started: Notify,
    teardown_completed: Notify,
    observations: Mutex<Vec<(&'static str, i64)>>,
    checkpoints: Mutex<Vec<SecurityCheckpoint>>,
}

impl RecordingPorts {
    fn new(failure: Option<FailureStage>) -> Self {
        Self::at(failure, 150)
    }

    fn at(failure: Option<FailureStage>, observed_at: i64) -> Self {
        Self {
            failures: failure.into_iter().collect(),
            unavailable: None,
            calls: Mutex::new(Vec::new()),
            audit_payloads: Mutex::new(Vec::new()),
            observed_at,
            block_sandbox_execute: false,
            sandbox_execute_started: Notify::new(),
            teardown_completed: Notify::new(),
            observations: Mutex::new(Vec::new()),
            checkpoints: Mutex::new(Vec::new()),
        }
    }

    fn unavailable(stage: FailureStage) -> Self {
        Self {
            failures: vec![stage],
            unavailable: Some(stage),
            calls: Mutex::new(Vec::new()),
            audit_payloads: Mutex::new(Vec::new()),
            observed_at: 150,
            block_sandbox_execute: false,
            sandbox_execute_started: Notify::new(),
            teardown_completed: Notify::new(),
            observations: Mutex::new(Vec::new()),
            checkpoints: Mutex::new(Vec::new()),
        }
    }

    fn with_failures(failures: Vec<FailureStage>) -> Self {
        Self {
            failures,
            unavailable: None,
            calls: Mutex::new(Vec::new()),
            audit_payloads: Mutex::new(Vec::new()),
            observed_at: 150,
            block_sandbox_execute: false,
            sandbox_execute_started: Notify::new(),
            teardown_completed: Notify::new(),
            observations: Mutex::new(Vec::new()),
            checkpoints: Mutex::new(Vec::new()),
        }
    }

    fn blocking_sandbox_execute() -> Self {
        Self {
            failures: Vec::new(),
            unavailable: None,
            calls: Mutex::new(Vec::new()),
            audit_payloads: Mutex::new(Vec::new()),
            observed_at: 150,
            block_sandbox_execute: true,
            sandbox_execute_started: Notify::new(),
            teardown_completed: Notify::new(),
            observations: Mutex::new(Vec::new()),
            checkpoints: Mutex::new(Vec::new()),
        }
    }

    fn fails(&self, stage: FailureStage) -> bool {
        self.failures.contains(&stage)
    }

    fn record(&self, stage: &'static str) {
        let mut calls = self.calls.lock().expect("calls lock");
        if calls.last().copied() != Some(stage) {
            calls.push(stage);
        }
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().expect("calls lock").clone()
    }

    fn audit_payloads(&self) -> Vec<Value> {
        self.audit_payloads.lock().expect("audit payloads").clone()
    }

    fn observe(&self, stage: &'static str, observed_at: i64) {
        self.observations
            .lock()
            .expect("observations")
            .push((stage, observed_at));
    }

    fn observations(&self) -> Vec<(&'static str, i64)> {
        self.observations.lock().expect("observations").clone()
    }

    fn checkpoints(&self) -> Vec<SecurityCheckpoint> {
        self.checkpoints.lock().expect("checkpoints").clone()
    }

    fn fail_security(&self, stage: FailureStage) -> Result<(), SecurityError> {
        if self.fails(stage) {
            if self.unavailable == Some(stage) {
                Err(SecurityError::Unavailable(format!(
                    "scripted {stage:?} provider outage"
                )))
            } else {
                Err(SecurityError::Denied(SecurityDenialReason::ActionDenied))
            }
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl WorkloadIdentityPort for RecordingPorts {
    async fn authenticate_workload(
        &self,
        _request: &WorkloadIdentityRequest,
    ) -> Result<AuthenticatedWorkload, SecurityError> {
        self.record("identity");
        self.fail_security(FailureStage::Identity)?;
        Ok(workload())
    }
}

#[async_trait::async_trait]
impl ExecutionSecurityScopePort for RecordingPorts {
    async fn resolve_execution_scope(
        &self,
        authenticated: &AuthenticatedWorkload,
        request: &ExecutionScopeResolveRequest,
    ) -> Result<ExecutionSecurityScope, SecurityError> {
        self.record("scope");
        self.fail_security(FailureStage::Scope)?;
        assert_eq!(authenticated, &workload());
        assert_eq!(request.execution_task_id, claim().execution_task_id);
        assert_eq!(request.resource, execution_resource());
        assert_eq!(request.observed_at, self.observed_at);
        Ok(scope())
    }
}

#[async_trait::async_trait]
impl PlacementAdmissionPort for RecordingPorts {
    async fn admit_placement(
        &self,
        authenticated: &AuthenticatedWorkload,
        resolved_scope: &ExecutionSecurityScope,
        sandbox_profile: &ExecutionSandboxProfile,
        observed_at: i64,
    ) -> Result<PlacementAdmission, SecurityError> {
        self.record("placement");
        self.fail_security(FailureStage::Placement)?;
        assert_eq!(authenticated, &workload());
        assert_eq!(resolved_scope, &scope());
        assert_eq!(sandbox_profile, &profile());
        assert_eq!(observed_at, self.observed_at);
        placement_policy()
            .evaluate(&placement_candidate())
            .map_err(SecurityError::Denied)
    }
}

#[async_trait::async_trait]
impl TenantAuthorizationPort for RecordingPorts {
    async fn authorize_tenant(
        &self,
        request: &TenantAuthorizationRequest,
    ) -> Result<TenantAuthorization, SecurityError> {
        self.record("authorization");
        self.fail_security(FailureStage::Authorization)?;
        Ok(TenantAuthorization {
            workload: request.workload.clone(),
            scope: request.scope.clone(),
            action: request.action,
            resource: request.resource.clone(),
            authorized_at: 150,
            expires_at: 300,
        })
    }
}

#[async_trait::async_trait]
impl PolicyRevocationPort for RecordingPorts {
    async fn check_security_epoch(
        &self,
        request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityError> {
        self.record("revocation");
        self.observe("revocation", request.observed_at);
        self.checkpoints
            .lock()
            .expect("checkpoints")
            .push(request.checkpoint);
        self.fail_security(FailureStage::Revocation)?;
        assert_eq!(request.pinned_epoch, 9);
        Ok(SecurityEpochStatus {
            checkpoint: request.checkpoint,
            organization_ref: request.organization_ref.clone(),
            project_ref: request.project_ref.clone(),
            execution_snapshot_ref: request.execution_snapshot_ref.clone(),
            current_epoch: 9,
            observed_at: request.observed_at,
        })
    }
}

#[async_trait::async_trait]
impl TaskLeasePort for RecordingPorts {
    async fn dispatch(
        &self,
        _request: &TaskLeaseDispatchRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        Err(TaskLeaseError::Invalid(
            "unused in security gate".to_string(),
        ))
    }

    async fn renew(
        &self,
        _request: &TaskLeaseRenewRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        Err(TaskLeaseError::Invalid(
            "unused in security gate".to_string(),
        ))
    }

    async fn release(
        &self,
        _request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        Err(TaskLeaseError::Invalid(
            "unused in security gate".to_string(),
        ))
    }

    async fn cancel(
        &self,
        _request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        Err(TaskLeaseError::Invalid(
            "unused in security gate".to_string(),
        ))
    }

    async fn validate_claim(
        &self,
        requested: &TaskLeaseClaim,
        observed_at: i64,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        self.record("lease");
        self.observe("lease", observed_at);
        if self.fails(FailureStage::Lease) {
            return Err(TaskLeaseError::Rejected {
                reason: agentd_core::ports::TaskLeaseRejectionReason::StaleFencingToken,
                message: "scripted lease failure".to_string(),
            });
        }
        assert_eq!(requested, &claim());
        Ok(lease_grant())
    }

    async fn expire_due(&self, _observed_at: i64) -> Result<u64, TaskLeaseError> {
        Err(TaskLeaseError::Invalid(
            "unused in security gate".to_string(),
        ))
    }
}

#[async_trait::async_trait]
impl AttemptCapabilityPort for RecordingPorts {
    async fn issue_capability(
        &self,
        _request: &CapabilityIssueRequest,
    ) -> Result<(CapabilityToken, CapabilityAdmission), SecurityError> {
        Err(SecurityError::Invalid(
            "production gate validates pre-issued capabilities".to_string(),
        ))
    }

    async fn validate_capability(
        &self,
        request: &CapabilityValidationRequest,
    ) -> Result<CapabilityAdmission, SecurityError> {
        self.record("capability");
        self.observe("capability", request.observed_at);
        self.fail_security(FailureStage::Capability)?;
        Ok(CapabilityAdmission {
            id: AttemptCapabilityId::new(),
            workload: workload(),
            scope: request.scope.clone(),
            action: request.action,
            resource: request.resource.clone(),
            issued_at: 140,
            expires_at: 300,
        })
    }

    async fn revoke_capability(
        &self,
        _id: &AttemptCapabilityId,
        _observed_at: i64,
    ) -> Result<(), SecurityError> {
        Err(SecurityError::Invalid(
            "unused in security gate".to_string(),
        ))
    }
}

#[async_trait::async_trait]
impl SecretBrokerPort for RecordingPorts {
    async fn checkout_secret(
        &self,
        request: &SecretCheckoutRequest,
    ) -> Result<SecretLease, SecurityError> {
        self.record("secret");
        self.observe("secret", request.observed_at);
        self.fail_security(FailureStage::Secret)?;
        Ok(SecretLease {
            selector: request.selector.clone(),
            material: SecretMaterial::new(b"transient-secret".to_vec()),
            expires_at: 250,
        })
    }
}

#[async_trait::async_trait]
impl ExecutionSandboxPort for RecordingPorts {
    async fn prepare_sandbox(
        &self,
        request: &SandboxPrepareRequest,
    ) -> Result<PreparedSandbox, SecurityError> {
        self.record("sandbox");
        self.fail_security(FailureStage::SandboxPrepare)?;
        Ok(PreparedSandbox {
            sandbox_id: "sb_enterprise".to_string(),
            profile: request.profile.clone(),
            created_at: 150,
            expires_at: 300,
        })
    }

    async fn execute_sandbox(
        &self,
        request: &SandboxExecuteRequest,
    ) -> Result<SandboxExecution, SecurityError> {
        self.observe("sandbox_execute", request.observed_at);
        if self.block_sandbox_execute {
            self.sandbox_execute_started.notify_waiters();
            std::future::pending::<()>().await;
        }
        self.fail_security(FailureStage::SandboxExecute)?;
        Ok(SandboxExecution {
            exit_code: 0,
            stdout: b"ok transient-output-secret".to_vec(),
            stderr: Vec::new(),
        })
    }

    async fn cleanup_sandbox(&self, request: &SandboxCleanupRequest) -> Result<(), SecurityError> {
        self.record("teardown");
        assert_eq!(request.sandbox_id, "sb_enterprise");
        if request.terminal_reason == agentd_core::types::SandboxTerminalReason::Cancelled {
            self.teardown_completed.notify_waiters();
        }
        if self.fails(FailureStage::Teardown) {
            Err(SecurityError::Denied(
                SecurityDenialReason::SandboxCleanupFailed,
            ))
        } else {
            Ok(())
        }
    }
}

#[async_trait::async_trait]
impl ContentRedactionPort for RecordingPorts {
    async fn redact_content(&self, content: &[u8]) -> Result<Vec<u8>, SecurityError> {
        self.record("redaction");
        self.fail_security(FailureStage::Redaction)?;
        Ok(String::from_utf8_lossy(content)
            .replace("transient-output-secret", "[REDACTED]")
            .into_bytes())
    }
}

#[async_trait::async_trait]
impl ExecutionAuditPort for RecordingPorts {
    async fn append_audit(
        &self,
        request: &ExecutionAuditAppend,
    ) -> Result<ExecutionAuditRecord, ExecutionEvidenceError> {
        self.record("audit");
        let payload = request.payload.to_string();
        assert!(!payload.contains("transient-secret"));
        for byte in [b'A', b'B', b'C'] {
            assert!(!payload.contains(&String::from_utf8(vec![byte; 32]).expect("ASCII token")));
            assert!(!payload.contains(&hex::encode([byte; 32])));
        }
        self.audit_payloads
            .lock()
            .expect("audit payloads")
            .push(request.payload.clone());
        if self.fails(FailureStage::Audit) {
            return Err(ExecutionEvidenceError::Unavailable(
                "scripted audit failure".to_string(),
            ));
        }
        Ok(ExecutionAuditRecord {
            append: request.clone(),
            sequence: 1,
            recorded_at: request.occurred_at,
        })
    }

    async fn read_audit(
        &self,
        _request: &AuditReadRequest,
    ) -> Result<AuditPage, ExecutionEvidenceError> {
        Ok(AuditPage {
            records: Vec::new(),
            next_after_sequence: None,
        })
    }
}

fn providers(ports: &Arc<RecordingPorts>) -> EnterpriseSecurityProviders {
    providers_at(ports, 150)
}

fn providers_at(ports: &Arc<RecordingPorts>, observed_at: i64) -> EnterpriseSecurityProviders {
    providers_with_clock(ports, Arc::new(FixedClock::new(observed_at)))
}

fn providers_with_clock(
    ports: &Arc<RecordingPorts>,
    clock: Arc<dyn Clock>,
) -> EnterpriseSecurityProviders {
    EnterpriseSecurityProviders::new(
        Arc::clone(ports) as Arc<dyn WorkloadIdentityPort>,
        Arc::clone(ports) as Arc<dyn ExecutionSecurityScopePort>,
        Arc::clone(ports) as Arc<dyn PlacementAdmissionPort>,
        Arc::clone(ports) as Arc<dyn TenantAuthorizationPort>,
        Arc::clone(ports) as Arc<dyn TaskLeasePort>,
        Arc::clone(ports) as Arc<dyn AttemptCapabilityPort>,
        Arc::clone(ports) as Arc<dyn SecretBrokerPort>,
        Arc::clone(ports) as Arc<dyn ExecutionSandboxPort>,
        Arc::clone(ports) as Arc<dyn ExecutionAuditPort>,
        Arc::clone(ports) as Arc<dyn ContentRedactionPort>,
        Arc::clone(ports) as Arc<dyn PolicyRevocationPort>,
        clock,
    )
}

#[derive(Debug)]
struct SequenceClock {
    times: Mutex<VecDeque<i64>>,
    last: Mutex<i64>,
}

impl SequenceClock {
    fn new(times: impl IntoIterator<Item = i64>) -> Self {
        let times = VecDeque::from_iter(times);
        let last = times.front().copied().unwrap_or(0);
        Self {
            times: Mutex::new(times),
            last: Mutex::new(last),
        }
    }
}

impl Clock for SequenceClock {
    fn now_unix(&self) -> i64 {
        let next = self.times.lock().expect("clock times").pop_front();
        if let Some(next) = next {
            *self.last.lock().expect("clock last") = next;
            next
        } else {
            *self.last.lock().expect("clock last")
        }
    }
}

fn authority_key() -> AuthorityKey {
    AuthorityKey::new("specify:enterprise-test").expect("authority key")
}

fn organization() -> OrganizationRef {
    OrganizationRef::new(authority_key(), "org-a", "1").expect("organization")
}

fn project() -> ProjectRef {
    ProjectRef::new(authority_key(), "project-a", "2").expect("project")
}

fn snapshot() -> ProjectExecutionSnapshotRef {
    ProjectExecutionSnapshotRef::new(authority_key(), "snapshot-a", "3").expect("snapshot")
}

fn worker_incarnation() -> WorkerIncarnationId {
    WorkerIncarnationId::from_string("wi_01ARZ3NDEKTSV4RRFFQ69G5FAV")
}

fn claim() -> TaskLeaseClaim {
    TaskLeaseClaim {
        execution_task_id: TaskRunId::from_string("tr_01ARZ3NDEKTSV4RRFFQ69G5FAW"),
        worker_incarnation_id: worker_incarnation(),
        lease_id: LeaseId::from_string("ls_01ARZ3NDEKTSV4RRFFQ69G5FAX"),
        fencing_token: FencingToken::new(7).expect("fencing token"),
    }
}

fn lease_grant() -> TaskLeaseGrant {
    TaskLeaseGrant {
        lease_id: claim().lease_id,
        execution_task_id: claim().execution_task_id,
        worker_incarnation_id: worker_incarnation(),
        fencing_token: FencingToken::new(7).expect("fencing token"),
        status: LeaseStatus::Active,
        acquired_at: 100,
        expires_at: 300,
        renewed_at: None,
        terminal_at: None,
        terminal_reason: None,
        record_version: 1,
    }
}

fn workload() -> AuthenticatedWorkload {
    AuthenticatedWorkload {
        spiffe_uri: format!("spiffe://agents.example/worker/{}", worker_incarnation()),
        role: WorkloadRole::Worker,
        trust_domain: "agents.example".to_string(),
        certificate_sha256: "a".repeat(64),
        not_before: 100,
        not_after: 300,
        worker_id: Some(WorkerId::from_string("wk_01ARZ3NDEKTSV4RRFFQ69G5FAY")),
        worker_incarnation_id: Some(worker_incarnation()),
    }
}

fn scope() -> ExecutionSecurityScope {
    ExecutionSecurityScope {
        authority_key: authority_key(),
        organization_ref: organization(),
        project_ref: project(),
        execution_snapshot_ref: snapshot(),
        rbac_policy_version_ref: RbacPolicyVersionRef::new(authority_key(), "rbac-a", "4")
            .expect("rbac ref"),
        worker_incarnation_id: worker_incarnation(),
        task_lease_claim: claim(),
        sandbox_profile_id: "oci-restricted-v1".to_string(),
        egress_profile_id: "deny-all-v1".to_string(),
        policy_revocation_epoch: 9,
        valid_until: 280,
        audit_context: SecurityAuditContext {
            execution_run_id: RunId::from_string("r_01ARZ3NDEKTSV4RRFFQ69G5FAZ"),
            snapshot_content_sha256: "b".repeat(64),
            target_repository_id: "repository-a".to_string(),
            target_base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        },
    }
}

fn execution_resource() -> ProtectedResource {
    ProtectedResource {
        organization_ref: organization(),
        project_ref: project(),
        execution_snapshot_ref: snapshot(),
        kind: ProtectedResourceKind::Execution,
    }
}

fn profile() -> ExecutionSandboxProfile {
    ExecutionSandboxProfile {
        profile_id: "oci-restricted-v1".to_string(),
        runtime: OciSandboxRuntime::Docker,
        image_digest: format!("sha256:{}", "c".repeat(64)),
        root_filesystem: SandboxRootFilesystem::ReadOnly,
        workspace: SandboxWorkspace::Ephemeral,
        mounts: vec![SandboxMount {
            source_id: "input-bundle".to_string(),
            target: "/workspace/input".to_string(),
            access: SandboxMountAccess::ReadOnly,
        }],
        linux_capabilities: SandboxLinuxCapabilities::DropAll,
        privilege_escalation: SandboxPrivilegeEscalation::Denied,
        seccomp_profile: "runtime-default".to_string(),
        limits: SandboxLimits {
            pids: 64,
            memory_bytes: 512 * 1024 * 1024,
            cpu_millis: 1_000,
        },
        tenant_cache_namespace: "specify:enterprise-test/org-a/project-a".to_string(),
        cache_sharing: SandboxCacheSharing::TenantOnly,
        egress: EgressPolicy::DenyAll,
    }
}

fn placement_policy() -> PlacementPolicy {
    PlacementPolicy {
        data_classification: DataClassification::Restricted,
        allowed_regions: BTreeSet::from(["local-zone".to_string()]),
        allowed_worker_trust_domains: BTreeSet::from(["agents.example".to_string()]),
        require_signed_image: true,
        require_dedicated_pool: true,
        egress_profile_id: "deny-all-v1".to_string(),
        tenant_cache_namespace: "specify:enterprise-test/org-a/project-a".to_string(),
    }
}

fn placement_candidate() -> PlacementCandidate {
    PlacementCandidate {
        supported_data_classifications: BTreeSet::from([DataClassification::Restricted]),
        region: "local-zone".to_string(),
        worker_trust_domain: "agents.example".to_string(),
        image_digest: format!("sha256:{}", "c".repeat(64)),
        image_signature_verified: true,
        dedicated_pool: true,
        egress_profile_id: "deny-all-v1".to_string(),
        tenant_cache_namespace: "specify:enterprise-test/org-a/project-a".to_string(),
    }
}

fn operation() -> EnterpriseWorkerOperation {
    EnterpriseWorkerOperation {
        identity_request: WorkloadIdentityRequest {
            peer_certificates_der: vec![vec![1, 2, 3]],
            observed_at: 150,
        },
        scope_request: ExecutionScopeResolveRequest {
            execution_task_id: claim().execution_task_id,
            resource: execution_resource(),
            audit_context: scope().audit_context,
            observed_at: 150,
        },
        sandbox_prepare_token: CapabilityToken::new([b'A'; 32]),
        sandbox_execute_token: CapabilityToken::new([b'B'; 32]),
        secret: Some(EnterpriseSecretRequest {
            selector: SecretSelector::new("repository/app-token").expect("secret selector"),
            capability_token: CapabilityToken::new([b'C'; 32]),
        }),
        profile: profile(),
        argv: vec!["cargo".to_string(), "test".to_string()],
        env: BTreeMap::from([("CI".to_string(), "1".to_string())]),
        observed_at: 150,
    }
}

fn configured_auth() -> AuthConfig {
    AuthConfig {
        api_token: Some("compatibility-listener-disabled".to_string()),
        agent_token_mode: AgentTokenMode::Hard,
        agent_tokens: BTreeMap::new(),
    }
}

async fn assert_failure_stops(failure: FailureStage, expected_calls: Vec<&'static str>) {
    let ports = Arc::new(RecordingPorts::new(Some(failure)));
    let runtime = build_security_runtime(
        SecurityRuntimeMode::Enterprise,
        &configured_auth(),
        Some(providers(&ports)),
    )
    .expect("enterprise composition");
    let SecurityRuntime::Enterprise(pipeline) = runtime else {
        panic!("enterprise mode must build the enterprise pipeline");
    };

    let error = Box::pin(pipeline.execute(operation()))
        .await
        .expect_err("scripted stage failure must fail closed");
    assert_eq!(
        ports.calls(),
        expected_calls,
        "failure at {failure:?}: {error}"
    );
}

fn expected_failure_calls(failure: FailureStage) -> Vec<&'static str> {
    let mut calls = match failure {
        FailureStage::Identity => vec!["identity"],
        FailureStage::Scope => vec!["identity", "scope"],
        FailureStage::Placement => vec!["identity", "scope", "placement"],
        FailureStage::Revocation => vec!["identity", "scope", "placement", "revocation"],
        FailureStage::Authorization => {
            vec![
                "identity",
                "scope",
                "placement",
                "revocation",
                "authorization",
            ]
        }
        FailureStage::Lease => vec![
            "identity",
            "scope",
            "placement",
            "revocation",
            "authorization",
            "revocation",
            "lease",
        ],
        FailureStage::Capability => vec![
            "identity",
            "scope",
            "placement",
            "revocation",
            "authorization",
            "revocation",
            "lease",
            "capability",
        ],
        FailureStage::Secret => vec![
            "identity",
            "scope",
            "placement",
            "revocation",
            "authorization",
            "revocation",
            "lease",
            "capability",
            "secret",
        ],
        FailureStage::SandboxPrepare => vec![
            "identity",
            "scope",
            "placement",
            "revocation",
            "authorization",
            "revocation",
            "lease",
            "capability",
            "secret",
            "revocation",
            "lease",
            "capability",
            "sandbox",
        ],
        FailureStage::SandboxExecute => vec![
            "identity",
            "scope",
            "placement",
            "revocation",
            "authorization",
            "revocation",
            "lease",
            "capability",
            "secret",
            "revocation",
            "lease",
            "capability",
            "sandbox",
            "revocation",
            "lease",
            "capability",
        ],
        FailureStage::Redaction | FailureStage::Audit | FailureStage::Teardown => vec![
            "identity",
            "scope",
            "placement",
            "revocation",
            "authorization",
            "revocation",
            "lease",
            "capability",
            "secret",
            "revocation",
            "lease",
            "capability",
            "sandbox",
            "revocation",
            "lease",
            "capability",
            "revocation",
            "redaction",
        ],
    };
    calls.push("audit");
    if matches!(
        failure,
        FailureStage::SandboxExecute
            | FailureStage::Redaction
            | FailureStage::Audit
            | FailureStage::Teardown
    ) {
        calls.push("teardown");
    }
    calls
}

#[tokio::test]
async fn production_security_gate_orders_checks_and_stops_on_failure() {
    for failure in [
        FailureStage::Identity,
        FailureStage::Scope,
        FailureStage::Placement,
        FailureStage::Revocation,
        FailureStage::Authorization,
        FailureStage::Lease,
        FailureStage::Capability,
        FailureStage::Secret,
        FailureStage::SandboxPrepare,
        FailureStage::SandboxExecute,
        FailureStage::Redaction,
        FailureStage::Audit,
        FailureStage::Teardown,
    ] {
        assert_failure_stops(failure, expected_failure_calls(failure)).await;
    }

    let ports = Arc::new(RecordingPorts::new(None));
    let runtime = build_security_runtime(
        SecurityRuntimeMode::Enterprise,
        &configured_auth(),
        Some(providers(&ports)),
    )
    .expect("enterprise composition");
    let SecurityRuntime::Enterprise(pipeline) = runtime else {
        panic!("enterprise mode must build the enterprise pipeline");
    };
    let result = Box::pin(pipeline.execute(operation()))
        .await
        .expect("secure operation");
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout, b"ok [REDACTED]");
    assert!(!String::from_utf8_lossy(&result.stdout).contains("transient-output-secret"));
    assert_eq!(
        ports.checkpoints(),
        vec![
            SecurityCheckpoint::Dispatch,
            SecurityCheckpoint::LeaseRenewal,
            SecurityCheckpoint::LeaseRenewal,
            SecurityCheckpoint::LeaseRenewal,
            SecurityCheckpoint::ArtifactAcceptance,
            SecurityCheckpoint::Delivery,
            SecurityCheckpoint::Release,
        ]
    );
    assert_eq!(
        ports.calls(),
        vec![
            "identity",
            "scope",
            "placement",
            "revocation",
            "authorization",
            "revocation",
            "lease",
            "capability",
            "secret",
            "revocation",
            "lease",
            "capability",
            "sandbox",
            "revocation",
            "lease",
            "capability",
            "revocation",
            "redaction",
            "audit",
            "teardown",
        ]
    );
}

#[tokio::test]
async fn production_security_gate_uses_trusted_clock_not_request_time() {
    let ports = Arc::new(RecordingPorts::at(None, 350));
    let runtime = build_security_runtime(
        SecurityRuntimeMode::Enterprise,
        &configured_auth(),
        Some(providers_at(&ports, 350)),
    )
    .expect("enterprise composition");
    let SecurityRuntime::Enterprise(pipeline) = runtime else {
        panic!("enterprise mode must build the enterprise pipeline");
    };

    let error = Box::pin(pipeline.execute(operation()))
        .await
        .expect_err("expired workload cannot replay a caller-selected timestamp");
    assert_eq!(
        error,
        SecurityError::Denied(SecurityDenialReason::LeaseRejected)
    );
    assert_eq!(ports.calls(), vec!["identity", "scope", "audit"]);
}

#[tokio::test]
async fn production_security_gate_audits_provider_unavailability_with_stable_reason() {
    let ports = Arc::new(RecordingPorts::unavailable(FailureStage::Authorization));
    let runtime = build_security_runtime(
        SecurityRuntimeMode::Enterprise,
        &configured_auth(),
        Some(providers(&ports)),
    )
    .expect("enterprise composition");
    let SecurityRuntime::Enterprise(pipeline) = runtime else {
        panic!("enterprise mode must build the enterprise pipeline");
    };

    let error = Box::pin(pipeline.execute(operation()))
        .await
        .expect_err("unavailable authorization provider must fail closed");
    assert!(matches!(error, SecurityError::Unavailable(_)));
    assert_eq!(
        ports.audit_payloads()[0]["reason"],
        "security_provider_unavailable"
    );
    assert_eq!(
        ports.calls(),
        vec![
            "identity",
            "scope",
            "placement",
            "revocation",
            "authorization",
            "audit",
        ]
    );
}

#[tokio::test]
async fn production_security_gate_preserves_audit_and_teardown_failures() {
    let ports = Arc::new(RecordingPorts::with_failures(vec![
        FailureStage::SandboxExecute,
        FailureStage::Audit,
        FailureStage::Teardown,
    ]));
    let runtime = build_security_runtime(
        SecurityRuntimeMode::Enterprise,
        &configured_auth(),
        Some(providers(&ports)),
    )
    .expect("enterprise composition");
    let SecurityRuntime::Enterprise(pipeline) = runtime else {
        panic!("enterprise mode must build the enterprise pipeline");
    };

    let error = Box::pin(pipeline.execute(operation()))
        .await
        .expect_err("compound terminal failures must fail closed");
    let message = error.to_string();
    assert!(
        message.contains("audit"),
        "missing audit failure: {message}"
    );
    assert!(
        message.contains("teardown"),
        "missing teardown failure: {message}"
    );
    assert_eq!(
        ports.calls(),
        vec![
            "identity",
            "scope",
            "placement",
            "revocation",
            "authorization",
            "revocation",
            "lease",
            "capability",
            "secret",
            "revocation",
            "lease",
            "capability",
            "sandbox",
            "revocation",
            "lease",
            "capability",
            "audit",
            "teardown",
        ]
    );
}

#[tokio::test]
async fn production_security_gate_cleans_up_when_operation_is_cancelled() {
    let ports = Arc::new(RecordingPorts::blocking_sandbox_execute());
    let execute_started = ports.sandbox_execute_started.notified();
    let teardown_completed = ports.teardown_completed.notified();
    let runtime = build_security_runtime(
        SecurityRuntimeMode::Enterprise,
        &configured_auth(),
        Some(providers(&ports)),
    )
    .expect("enterprise composition");
    let SecurityRuntime::Enterprise(pipeline) = runtime else {
        panic!("enterprise mode must build the enterprise pipeline");
    };

    let operation_task = tokio::spawn(async move { Box::pin(pipeline.execute(operation())).await });
    tokio::time::timeout(Duration::from_secs(1), execute_started)
        .await
        .expect("sandbox execution must start");
    operation_task.abort();
    assert!(
        operation_task
            .await
            .expect_err("operation is cancelled")
            .is_cancelled(),
        "operation task must report cancellation"
    );
    tokio::time::timeout(Duration::from_secs(1), teardown_completed)
        .await
        .expect("cancelled operation must trigger sandbox teardown");
    assert_eq!(
        ports.calls(),
        vec![
            "identity",
            "scope",
            "placement",
            "revocation",
            "authorization",
            "revocation",
            "lease",
            "capability",
            "secret",
            "revocation",
            "lease",
            "capability",
            "sandbox",
            "revocation",
            "lease",
            "capability",
            "audit",
            "teardown",
        ]
    );
    assert_eq!(ports.audit_payloads()[0]["reason"], "operation_cancelled");
}

#[tokio::test]
async fn production_security_gate_revalidates_before_each_external_side_effect() {
    let ports = Arc::new(RecordingPorts::new(None));
    let clock = Arc::new(SequenceClock::new([150, 150, 150, 300, 300]));
    let runtime = build_security_runtime(
        SecurityRuntimeMode::Enterprise,
        &configured_auth(),
        Some(providers_with_clock(&ports, clock)),
    )
    .expect("enterprise composition");
    let SecurityRuntime::Enterprise(pipeline) = runtime else {
        panic!("enterprise mode must build the enterprise pipeline");
    };

    let error = Box::pin(pipeline.execute(operation()))
        .await
        .expect_err("expired lease must stop execution after sandbox prepare");
    assert_eq!(
        error,
        SecurityError::Denied(SecurityDenialReason::LeaseRejected)
    );
    assert!(
        !ports.observations().contains(&("lease", 300)),
        "expired scope checkpoint must stop before lease validation"
    );
    assert!(
        !ports
            .observations()
            .iter()
            .any(|(stage, _)| *stage == "sandbox_execute"),
        "expired lease must stop before sandbox execute"
    );
    assert_eq!(ports.calls().last(), Some(&"teardown"));
}

#[tokio::test]
async fn enterprise_security_mode_rejects_missing_providers_and_open_auth() {
    let cli = AgentdCli::try_parse_from(["agentd", "--security-mode", "enterprise"])
        .expect("enterprise mode parses");
    assert_eq!(cli.config.security_mode, SecurityRuntimeMode::Enterprise);

    let open_ports = Arc::new(RecordingPorts::new(None));
    let open_error = build_security_runtime(
        SecurityRuntimeMode::Enterprise,
        &AuthConfig::open(),
        Some(providers(&open_ports)),
    )
    .expect_err("enterprise mode rejects open auth before composition");
    assert!(open_error.to_string().contains("open_auth"));
    assert!(open_ports.calls().is_empty());

    let agent_only_auth = AuthConfig {
        api_token: None,
        agent_token_mode: AgentTokenMode::Hard,
        agent_tokens: BTreeMap::from([("worker-a".to_string(), "worker-token".to_string())]),
    };
    assert!(
        validate_enterprise_control_plane_auth(&agent_only_auth)
            .expect_err("enterprise control plane requires an operator bearer token")
            .to_string()
            .contains("open_auth")
    );

    let audit_only_auth = AuthConfig {
        api_token: Some("operator-token".to_string()),
        agent_token_mode: AgentTokenMode::Audit,
        agent_tokens: BTreeMap::from([("worker-a".to_string(), "worker-token".to_string())]),
    };
    let audit_only_ports = Arc::new(RecordingPorts::new(None));
    let audit_only_error = build_security_runtime(
        SecurityRuntimeMode::Enterprise,
        &audit_only_auth,
        Some(providers(&audit_only_ports)),
    )
    .expect_err("enterprise mode rejects audit-only agent token enforcement");
    assert!(audit_only_error.to_string().contains("audit_only_auth"));
    assert!(audit_only_ports.calls().is_empty());

    for kind in SecurityProviderKind::ALL {
        let ports = Arc::new(RecordingPorts::new(None));
        let selected = providers(&ports).without(kind);
        let error = build_security_runtime(
            SecurityRuntimeMode::Enterprise,
            &configured_auth(),
            Some(selected),
        )
        .expect_err("missing provider must fail startup");
        assert!(
            error.to_string().contains(kind.as_str()),
            "missing provider error must name {}: {error}",
            kind.as_str()
        );
        assert!(ports.calls().is_empty());
    }

    let missing_all =
        build_security_runtime(SecurityRuntimeMode::Enterprise, &configured_auth(), None)
            .expect_err("enterprise mode requires explicit providers");
    assert!(missing_all.to_string().contains("workload_identity"));

    let temp = tempfile::tempdir().expect("tempdir");
    let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("reserve free port");
    let port = probe.local_addr().expect("probe address").port();
    drop(probe);
    let db_path = temp.path().join("enterprise-must-not-open.db");
    let daemon_error = daemon::serve(DaemonConfig {
        security_mode: SecurityRuntimeMode::Enterprise,
        db_path: db_path.clone(),
        port,
        workflows_dir: PathBuf::from("workflows"),
        repo_dir: PathBuf::from("."),
        worktree_base: temp.path().join("worktrees"),
        log_level: "info".to_string(),
        api_token: Some("closed-listener".to_string()),
        agent_tokens: Vec::new(),
        agent_token_mode: "hard".to_string(),
        enterprise: Default::default(),
    })
    .await
    .expect_err("missing enterprise providers reject daemon startup");
    assert!(daemon_error.to_string().contains("workload_identity"));
    assert!(!db_path.exists(), "startup must fail before opening SQLite");
    let _still_free = std::net::TcpListener::bind(("127.0.0.1", port))
        .expect("startup must fail before binding the HTTP listener");

    let standalone =
        build_security_runtime(SecurityRuntimeMode::Standalone, &AuthConfig::open(), None)
            .expect("explicit standalone mode preserves compatibility behavior");
    assert!(matches!(standalone, SecurityRuntime::Standalone));
}
