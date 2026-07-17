//! Native PTY-backed workflow and registered-agent lifecycle.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentd_core::CoreError;
use agentd_core::ports::{
    AgentAllocation, AgentAllocationStatus, AgentBackend, InteractiveSandboxPort,
    NativeRuntimeError, PolicyRevocationPort, RuntimeCommand, RuntimeDimensions, RuntimeProvider,
    RuntimeSandboxCommandRequest, RuntimeSandboxRef, RuntimeSessionRegistration,
    RuntimeShutdownMethod, RuntimeShutdownRequest, RuntimeTerminalReason, RuntimeTextInput,
    SecurityError,
};
use agentd_core::types::{
    AgentHandle, AgentId, AttemptCapabilityId, AuthenticatedWorkload, AuthorityKey, BackendKind,
    CapabilityAdmission, CliKind, EgressPolicy, ExecutionSandboxProfile, ExecutionSecurityScope,
    FencingToken, LeaseId, OciSandboxRuntime, PreparedSandbox, ProjectExecutionSnapshotRef,
    ProjectRef, ProtectedAction, ProtectedResource, ProtectedResourceKind, RbacPolicyVersionRef,
    RepositoryRef, RunId, SandboxCacheSharing, SandboxLimits, SandboxLinuxCapabilities,
    SandboxMount, SandboxMountAccess, SandboxPrivilegeEscalation, SandboxRootFilesystem,
    SandboxWorkspace, SecurityAuditContext, SecurityEpochRequest, SecurityEpochStatus,
    WorkerIncarnationId, WorkloadRole,
};
use agentd_runtime::ProviderCommand;
use agentd_store::SqliteStore;
use agentd_store::native_agent_binding::{
    NativeAgentRuntimeBinding, active_native_agent_binding, ensure_native_agent_profile,
    ensure_native_execution_task, ensure_native_runtime_authority, finish_native_agent_binding,
    native_agent_binding, record_native_agent_binding,
};
use sha2::{Digest, Sha256};

use crate::host::{AgentLifecycle, AgentLifecycleShutdown, AgentLifecycleShutdownReport};
use crate::runtime::{NativeRuntimeService, NativeRuntimeStartRequest, provider_command_sha256};

const MAX_CAPTURE_BYTES: u64 = 256 * 1024;
const MAX_TRANSCRIPT_BYTES: u64 = 64 * 1024 * 1024;
const IDLE_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1_000;
const AUTHORITY_LIFETIME_SECONDS: i64 = 366 * 24 * 60 * 60;

#[derive(Debug, Clone)]
pub struct NativeAgentBackend {
    store: SqliteStore,
    service: Arc<NativeRuntimeService>,
    host_instance_id: String,
}

impl NativeAgentBackend {
    #[must_use]
    pub fn new(
        store: SqliteStore,
        service: Arc<NativeRuntimeService>,
        host_instance_id: impl Into<String>,
    ) -> Self {
        Self {
            store,
            service,
            host_instance_id: host_instance_id.into(),
        }
    }

    #[must_use]
    pub fn service(&self) -> Arc<NativeRuntimeService> {
        Arc::clone(&self.service)
    }

    async fn start_native(
        &self,
        request: agentd_core::types::SpawnRequest,
    ) -> Result<AgentHandle, CoreError> {
        if !matches!(
            request.launch_strategy,
            agentd_core::types::LaunchStrategy::Direct
        ) {
            return Err(CoreError::Backend(
                "native runtime owns child supervision and rejects external launch scopes"
                    .to_string(),
            ));
        }
        let worktree = fs::canonicalize(&request.worktree).map_err(|error| {
            CoreError::Backend(format!(
                "native runtime worktree {} is unavailable: {error}",
                request.worktree.display()
            ))
        })?;
        if !worktree.is_dir() {
            return Err(CoreError::Backend(
                "native runtime worktree is not a directory".to_string(),
            ));
        }
        let observed_at = now_unix()?;
        let authority =
            ensure_native_runtime_authority(self.store.pool(), &self.host_instance_id).await?;
        let provider = runtime_provider(request.cli);
        let runtime_name = provider.as_str();
        let profile_id =
            ensure_native_agent_profile(self.store.pool(), request.agent_id.as_str(), runtime_name)
                .await?;
        let (execution_task_id, synthetic_task) =
            ensure_native_execution_task(self.store.pool(), request.execution_task_id.as_ref())
                .await?;
        let provider_command = provider_command(provider, &request, &worktree);
        let command_sha256 = provider_command_sha256(&provider_command);
        let security = local_security_context(
            &request.agent_id,
            &execution_task_id,
            &authority.worker_id,
            &authority.worker_incarnation_id,
            &worktree,
            &command_sha256,
            observed_at,
        )?;
        let session_id = agentd_core::types::RuntimeSessionId::new();
        let attempt_id = agentd_core::types::RuntimeAttemptId::new();
        let start = NativeRuntimeStartRequest {
            registration: RuntimeSessionRegistration {
                session_id: session_id.clone(),
                execution_task_id: execution_task_id.clone(),
                agent_profile_id: profile_id,
                snapshot_ref: security.snapshot_ref.clone(),
                snapshot_content_sha256: security.snapshot_sha256.clone(),
                provider,
                command_sha256,
                sandbox: RuntimeSandboxRef {
                    sandbox_id: security.sandbox.sandbox_id.clone(),
                    profile_sha256: security.sandbox_profile_sha256,
                    expires_at: security.sandbox.expires_at,
                },
                max_capture_bytes: MAX_CAPTURE_BYTES,
                max_transcript_bytes: MAX_TRANSCRIPT_BYTES,
                idle_timeout_ms: IDLE_TIMEOUT_MS,
                created_at: observed_at,
            },
            attempt_id: attempt_id.clone(),
            worker_incarnation_id: authority.worker_incarnation_id,
            host_instance_id: authority.host_instance_id,
            admission: security.admission.clone(),
            sandbox: security.sandbox,
            provider_command,
            dimensions: RuntimeDimensions {
                rows: 40,
                columns: 140,
                pixel_width: 0,
                pixel_height: 0,
            },
        };
        let view = self.service.start(start).await.map_err(native_error)?;
        let attempt = view.attempt.as_ref().ok_or_else(|| {
            CoreError::Backend("native runtime start returned no attempt".to_string())
        })?;
        let binding = record_native_agent_binding(
            self.store.pool(),
            &NativeAgentRuntimeBinding {
                runtime_session_id: session_id.clone(),
                runtime_attempt_id: attempt_id.clone(),
                agent_id: request.agent_id.as_str().to_string(),
                execution_task_id,
                synthetic_task,
                capability: security.admission.clone(),
                worktree: worktree.to_string_lossy().into_owned(),
                status: "active".to_string(),
                created_at: observed_at,
                finished_at: None,
            },
        )
        .await?;
        if let Some(prompt) = request.initial_prompt.filter(|prompt| !prompt.is_empty()) {
            if let Err(error) = self
                .service
                .send_text(
                    &binding.capability,
                    RuntimeTextInput {
                        attempt_id: attempt_id.clone(),
                        idempotency_key: format!("initial:{}", attempt_id.as_str()),
                        text: prompt,
                        submit: true,
                        observed_at,
                    },
                )
                .await
            {
                let _ = self
                    .shutdown_binding(&binding, RuntimeTerminalReason::Failed)
                    .await;
                return Err(native_error(error));
            }
        }
        Ok(handle_from_binding(
            &binding,
            attempt.pid,
            unix_system_time(observed_at),
        ))
    }

    async fn dispatch_active(
        &self,
        request: &agentd_core::types::SpawnRequest,
    ) -> Result<Option<AgentHandle>, CoreError> {
        let Some(binding) =
            active_native_agent_binding(self.store.pool(), request.agent_id.as_str()).await?
        else {
            return Ok(None);
        };
        let view = self
            .service
            .snapshot(&binding.runtime_session_id)
            .await
            .map_err(native_error)?;
        let Some(live) = view.live else {
            return Ok(None);
        };
        if live.status.is_terminal() || live.attempt_id != binding.runtime_attempt_id {
            return Ok(None);
        }
        if let Some(prompt) = request
            .initial_prompt
            .as_ref()
            .filter(|prompt| !prompt.is_empty())
        {
            self.service
                .send_text(
                    &binding.capability,
                    RuntimeTextInput {
                        attempt_id: binding.runtime_attempt_id.clone(),
                        idempotency_key: format!(
                            "dispatch:{}:{}",
                            binding.runtime_attempt_id.as_str(),
                            sha256(prompt.as_bytes())
                        ),
                        text: prompt.clone(),
                        submit: true,
                        observed_at: now_unix()?,
                    },
                )
                .await
                .map_err(native_error)?;
        }
        Ok(Some(handle_from_binding(
            &binding,
            live.pid,
            unix_system_time(live.started_at),
        )))
    }

    async fn shutdown_binding(
        &self,
        binding: &NativeAgentRuntimeBinding,
        reason: RuntimeTerminalReason,
    ) -> Result<agentd_core::ports::RuntimeShutdownReport, CoreError> {
        let report = self
            .service
            .shutdown(
                &binding.capability,
                RuntimeShutdownRequest {
                    attempt_id: binding.runtime_attempt_id.clone(),
                    idempotency_key: format!(
                        "agent-shutdown:{}:{reason:?}",
                        binding.runtime_attempt_id.as_str()
                    ),
                    graceful_timeout_ms: 5_000,
                    interrupt_timeout_ms: 2_000,
                    reason,
                    observed_at: now_unix()?,
                },
            )
            .await
            .map_err(native_error)?;
        finish_native_agent_binding(
            self.store.pool(),
            &binding.runtime_session_id,
            report.finished_at,
        )
        .await?;
        Ok(report)
    }
}

#[async_trait::async_trait]
impl AgentBackend for NativeAgentBackend {
    async fn spawn(
        &self,
        request: agentd_core::types::SpawnRequest,
    ) -> Result<AgentHandle, CoreError> {
        self.start_native(request).await
    }

    async fn dispatch_allocated(
        &self,
        request: agentd_core::types::SpawnRequest,
        allocation: &AgentAllocation,
    ) -> Result<AgentHandle, CoreError> {
        if allocation.status == AgentAllocationStatus::Routed {
            if let Some(handle) = self.dispatch_active(&request).await? {
                return Ok(handle);
            }
        }
        self.start_native(request).await
    }
}

#[derive(Debug, Clone)]
pub struct NativeAgentLifecycle {
    backend: Arc<NativeAgentBackend>,
}

impl NativeAgentLifecycle {
    #[must_use]
    pub fn new(backend: Arc<NativeAgentBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait::async_trait]
impl AgentLifecycle for NativeAgentLifecycle {
    async fn shutdown(
        &self,
        handle: &AgentHandle,
        options: AgentLifecycleShutdown,
    ) -> Result<AgentLifecycleShutdownReport, CoreError> {
        let (session_id, attempt_id) = parse_address(&handle.address)?;
        let binding = native_agent_binding(self.backend.store.pool(), &session_id)
            .await?
            .ok_or_else(|| CoreError::Backend("native agent binding not found".to_string()))?;
        if binding.runtime_attempt_id != attempt_id {
            return Err(CoreError::Backend(
                "native agent address is not the bound attempt".to_string(),
            ));
        }
        let report = self
            .backend
            .shutdown_binding(&binding, RuntimeTerminalReason::Cancelled)
            .await?;
        write_archive_pointer(&options.archive_to, &report)?;
        Ok(AgentLifecycleShutdownReport {
            method: shutdown_method(report.method).to_string(),
            final_capture_sha: report.transcript.content_sha256,
        })
    }

    async fn rebind(&self, target: &str) -> Result<Option<AgentHandle>, CoreError> {
        let (session_id, attempt_id) = parse_address(target)?;
        let Some(binding) = native_agent_binding(self.backend.store.pool(), &session_id).await?
        else {
            return Ok(None);
        };
        if binding.runtime_attempt_id != attempt_id || binding.status != "active" {
            return Ok(None);
        }
        let view = self
            .backend
            .service
            .snapshot(&session_id)
            .await
            .map_err(native_error)?;
        let Some(live) = view.live else {
            return Ok(None);
        };
        if live.status.is_terminal() || live.attempt_id != attempt_id {
            return Ok(None);
        }
        Ok(Some(handle_from_binding(
            &binding,
            live.pid,
            unix_system_time(live.started_at),
        )))
    }
}

#[derive(Debug)]
pub struct LocalInteractiveSandbox {
    allowed_roots: Vec<PathBuf>,
}

impl LocalInteractiveSandbox {
    pub fn new(roots: impl IntoIterator<Item = PathBuf>) -> Result<Self, CoreError> {
        let mut allowed_roots = Vec::new();
        for root in roots {
            fs::create_dir_all(&root)?;
            allowed_roots.push(fs::canonicalize(&root).map_err(|error| {
                CoreError::Backend(format!(
                    "native sandbox root {} is unavailable: {error}",
                    root.display()
                ))
            })?);
        }
        Ok(Self { allowed_roots })
    }
}

#[async_trait::async_trait]
impl InteractiveSandboxPort for LocalInteractiveSandbox {
    async fn interactive_command(
        &self,
        request: &RuntimeSandboxCommandRequest,
    ) -> Result<RuntimeCommand, NativeRuntimeError> {
        if request.argv.is_empty()
            || request.sandbox.expires_at <= request.observed_at
            || request.admission.scope.sandbox_profile_id != request.sandbox.profile.profile_id
        {
            return Err(NativeRuntimeError::Denied(
                "local interactive sandbox binding is invalid".to_string(),
            ));
        }
        let working_directory = fs::canonicalize(&request.working_directory).map_err(|_| {
            NativeRuntimeError::Denied("runtime working directory is unavailable".to_string())
        })?;
        if !self
            .allowed_roots
            .iter()
            .any(|root| working_directory.starts_with(root))
        {
            return Err(NativeRuntimeError::Denied(
                "runtime working directory is outside configured roots".to_string(),
            ));
        }
        Ok(RuntimeCommand {
            program: request.argv[0].clone(),
            arguments: request.argv[1..].to_vec(),
            environment: request.environment.clone(),
            working_directory,
        })
    }
}

#[derive(Debug, Default)]
pub struct StandalonePolicyRevocation;

#[async_trait::async_trait]
impl PolicyRevocationPort for StandalonePolicyRevocation {
    async fn check_security_epoch(
        &self,
        request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityError> {
        Ok(SecurityEpochStatus {
            checkpoint: request.checkpoint,
            organization_ref: request.organization_ref.clone(),
            project_ref: request.project_ref.clone(),
            execution_snapshot_ref: request.execution_snapshot_ref.clone(),
            current_epoch: request.pinned_epoch,
            observed_at: request.observed_at,
        })
    }
}

struct LocalSecurityContext {
    admission: CapabilityAdmission,
    snapshot_ref: ProjectExecutionSnapshotRef,
    snapshot_sha256: String,
    sandbox: PreparedSandbox,
    sandbox_profile_sha256: String,
}

#[allow(clippy::too_many_arguments)]
fn local_security_context(
    agent_id: &AgentId,
    execution_task_id: &agentd_core::types::TaskRunId,
    worker_id: &agentd_core::types::WorkerId,
    worker_incarnation_id: &WorkerIncarnationId,
    worktree: &Path,
    command_sha256: &str,
    observed_at: i64,
) -> Result<LocalSecurityContext, CoreError> {
    let expires_at = observed_at
        .checked_add(AUTHORITY_LIFETIME_SECONDS)
        .ok_or_else(|| CoreError::Invariant("native capability expiry overflow".to_string()))?;
    let authority_key = AuthorityKey::new("agentd-local").map_err(project_ref_error)?;
    let organization_ref =
        agentd_core::types::OrganizationRef::new(authority_key.clone(), "local-organization", "v1")
            .map_err(project_ref_error)?;
    let project_ref =
        ProjectRef::new(authority_key.clone(), "local-project", "v1").map_err(project_ref_error)?;
    let snapshot_sha256 = sha256(
        format!(
            "{}:{}:{}",
            execution_task_id.as_str(),
            worktree.display(),
            command_sha256
        )
        .as_bytes(),
    );
    let snapshot_ref = ProjectExecutionSnapshotRef::new(
        authority_key.clone(),
        execution_task_id.as_str(),
        &snapshot_sha256,
    )
    .map_err(project_ref_error)?;
    let rbac_policy_version_ref =
        RbacPolicyVersionRef::new(authority_key.clone(), "local-native-policy", "v1")
            .map_err(project_ref_error)?;
    let repository_ref = RepositoryRef::new(
        authority_key.clone(),
        format!("worktree-{}", sha256(worktree.to_string_lossy().as_bytes())),
        command_sha256,
    )
    .map_err(project_ref_error)?;
    let profile = ExecutionSandboxProfile {
        profile_id: "local-native-pty".to_string(),
        runtime: OciSandboxRuntime::Docker,
        image_digest: format!("sha256:{}", "0".repeat(64)),
        root_filesystem: SandboxRootFilesystem::ReadOnly,
        workspace: SandboxWorkspace::Ephemeral,
        mounts: vec![SandboxMount {
            source_id: sha256(worktree.to_string_lossy().as_bytes()),
            target: "/workspace".to_string(),
            access: SandboxMountAccess::ReadWrite,
        }],
        linux_capabilities: SandboxLinuxCapabilities::DropAll,
        privilege_escalation: SandboxPrivilegeEscalation::Denied,
        seccomp_profile: "agentd-local-native".to_string(),
        limits: SandboxLimits {
            pids: 512,
            memory_bytes: 16 * 1024 * 1024 * 1024,
            cpu_millis: 8_000,
        },
        tenant_cache_namespace: "local-agentd".to_string(),
        cache_sharing: SandboxCacheSharing::TenantOnly,
        egress: EgressPolicy::Allow(vec!["https".to_string(), "ssh".to_string()]),
    };
    let profile_bytes = serde_json::to_vec(&profile)?;
    let sandbox_profile_sha256 = sha256(&profile_bytes);
    let sandbox = PreparedSandbox {
        sandbox_id: format!("sb_{}", &snapshot_sha256[..26]),
        profile: profile.clone(),
        created_at: observed_at,
        expires_at,
    };
    let resource = ProtectedResource {
        organization_ref: organization_ref.clone(),
        project_ref: project_ref.clone(),
        execution_snapshot_ref: snapshot_ref.clone(),
        kind: ProtectedResourceKind::Repository(repository_ref),
    };
    let scope = ExecutionSecurityScope {
        authority_key,
        organization_ref,
        project_ref,
        execution_snapshot_ref: snapshot_ref.clone(),
        rbac_policy_version_ref,
        worker_incarnation_id: worker_incarnation_id.clone(),
        task_lease_claim: agentd_core::types::TaskLeaseClaim {
            execution_task_id: execution_task_id.clone(),
            worker_incarnation_id: worker_incarnation_id.clone(),
            lease_id: LeaseId::new(),
            fencing_token: FencingToken::new(1)
                .map_err(|error| CoreError::Invariant(format!("native fencing token: {error}")))?,
        },
        sandbox_profile_id: profile.profile_id.clone(),
        egress_profile_id: "local-native-egress".to_string(),
        policy_revocation_epoch: 1,
        valid_until: expires_at,
        audit_context: SecurityAuditContext {
            execution_run_id: RunId::new(),
            snapshot_content_sha256: snapshot_sha256.clone(),
            target_repository_id: worktree.to_string_lossy().into_owned(),
            target_base_commit: command_sha256.to_string(),
        },
    };
    let admission = CapabilityAdmission {
        id: AttemptCapabilityId::new(),
        workload: AuthenticatedWorkload {
            spiffe_uri: format!("spiffe://local.agentd/worker/{}", worker_id.as_str()),
            role: WorkloadRole::Worker,
            trust_domain: "local.agentd".to_string(),
            certificate_sha256: sha256(worker_id.as_str().as_bytes()),
            not_before: observed_at,
            not_after: expires_at,
            worker_id: Some(worker_id.clone()),
            worker_incarnation_id: Some(worker_incarnation_id.clone()),
        },
        scope,
        action: ProtectedAction::SandboxExecute,
        resource,
        issued_at: observed_at,
        expires_at,
    };
    Ok(LocalSecurityContext {
        admission,
        snapshot_ref,
        snapshot_sha256,
        sandbox,
        sandbox_profile_sha256,
    })
}

fn provider_command(
    provider: RuntimeProvider,
    request: &agentd_core::types::SpawnRequest,
    worktree: &Path,
) -> ProviderCommand {
    let (program, arguments) = match provider {
        RuntimeProvider::Codex => (
            "codex".to_string(),
            vec![
                "--ask-for-approval".to_string(),
                "never".to_string(),
                "--sandbox".to_string(),
                "danger-full-access".to_string(),
            ],
        ),
        RuntimeProvider::ClaudeCode => (
            "claude".to_string(),
            vec!["--dangerously-skip-permissions".to_string()],
        ),
        RuntimeProvider::Custom => unreachable!("spawn request has no custom CLI variant"),
    };
    ProviderCommand {
        provider,
        program,
        arguments,
        environment: request
            .env_overrides
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>(),
        working_directory: worktree.to_path_buf(),
        custom_resume_arguments: None,
    }
}

fn runtime_provider(cli: CliKind) -> RuntimeProvider {
    match cli {
        CliKind::Codex => RuntimeProvider::Codex,
        CliKind::ClaudeCode => RuntimeProvider::ClaudeCode,
    }
}

fn handle_from_binding(
    binding: &NativeAgentRuntimeBinding,
    pid: Option<u32>,
    spawned_at: SystemTime,
) -> AgentHandle {
    AgentHandle {
        agent_id: AgentId::parsed(&binding.agent_id),
        backend: BackendKind::NativeRuntime,
        address: format!(
            "native://{}/{}",
            binding.runtime_session_id.as_str(),
            binding.runtime_attempt_id.as_str()
        ),
        pane_id: None,
        pid,
        session_name: binding.runtime_session_id.to_string(),
        spawned_at,
    }
}

fn parse_address(
    address: &str,
) -> Result<
    (
        agentd_core::types::RuntimeSessionId,
        agentd_core::types::RuntimeAttemptId,
    ),
    CoreError,
> {
    let rest = address
        .strip_prefix("native://")
        .ok_or_else(|| CoreError::Backend("runtime address must use native://".to_string()))?;
    let (session, attempt) = rest
        .split_once('/')
        .ok_or_else(|| CoreError::Backend("runtime address is missing its attempt".to_string()))?;
    if !session.starts_with("rs_") || !attempt.starts_with("ra_") {
        return Err(CoreError::Backend(
            "runtime address contains invalid ids".to_string(),
        ));
    }
    Ok((
        agentd_core::types::RuntimeSessionId::from_string(session),
        agentd_core::types::RuntimeAttemptId::from_string(attempt),
    ))
}

fn write_archive_pointer(
    path: &Path,
    report: &agentd_core::ports::RuntimeShutdownReport,
) -> Result<(), CoreError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "runtime_session_id": report.session_id,
        "runtime_attempt_id": report.attempt_id,
        "transcript_sha256": report.transcript.content_sha256,
        "transcript_storage_ref": report.transcript.storage_ref,
        "size_bytes": report.transcript.size_bytes,
        "truncated": report.transcript.truncated,
    }))?;
    fs::write(path, bytes)?;
    Ok(())
}

fn shutdown_method(method: RuntimeShutdownMethod) -> &'static str {
    match method {
        RuntimeShutdownMethod::Graceful => "graceful",
        RuntimeShutdownMethod::Interrupt => "interrupt",
        RuntimeShutdownMethod::Kill => "kill",
        RuntimeShutdownMethod::AlreadyExited => "already_exited",
    }
}

fn project_ref_error(error: impl std::fmt::Display) -> CoreError {
    CoreError::Invariant(format!("native authority reference is invalid: {error}"))
}

fn native_error(error: NativeRuntimeError) -> CoreError {
    CoreError::Backend(error.to_string())
}

fn now_unix() -> Result<i64, CoreError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| CoreError::Backend(format!("system clock is invalid: {error}")))?
        .as_secs();
    i64::try_from(seconds).map_err(|_| CoreError::Backend("system clock exceeds i64".to_string()))
}

fn unix_system_time(seconds: i64) -> SystemTime {
    u64::try_from(seconds)
        .ok()
        .and_then(|seconds| UNIX_EPOCH.checked_add(Duration::from_secs(seconds)))
        .unwrap_or(UNIX_EPOCH)
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
