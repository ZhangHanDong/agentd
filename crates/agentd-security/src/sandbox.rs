//! OCI sandbox adapter.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agentd_core::ports::{
    AuditActorKind, CommandRunner, ExecutionAuditAppend, ExecutionAuditPort,
    ExecutionEvidenceLinks, ExecutionSandboxPort, ExecutionSnapshotLink, RunOpts, SecurityError,
};
use agentd_core::types::{
    CapabilityAdmission, EgressPolicy, OciSandboxRuntime, PreparedSandbox, ProtectedAction,
    ProtectedResourceKind, SandboxCacheSharing, SandboxCleanupRequest, SandboxExecuteRequest,
    SandboxExecution, SandboxLinuxCapabilities, SandboxMountAccess, SandboxPrepareRequest,
    SandboxPrivilegeEscalation, SandboxRootFilesystem, SandboxTerminalReason, SandboxWorkspace,
    SecurityDenialReason,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct OciSandboxConfig {
    pub runtime_bin: String,
    pub workspace_root: PathBuf,
    pub cache_root: PathBuf,
    pub allowed_input_root: PathBuf,
    pub input_sources: HashMap<String, PathBuf>,
    pub command_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxTeardownRecord {
    pub sandbox_id: String,
    pub terminal_reason: SandboxTerminalReason,
    pub observed_at: i64,
    pub error_kind: String,
}

#[derive(Clone)]
struct SandboxState {
    prepare_admission: CapabilityAdmission,
    prepared: PreparedSandbox,
    workspace_path: PathBuf,
    cache_path: PathBuf,
}

pub struct OciSandboxAdapter {
    runner: Arc<dyn CommandRunner>,
    audit: Arc<dyn ExecutionAuditPort>,
    config: OciSandboxConfig,
    states: Mutex<HashMap<String, SandboxState>>,
    teardown_records: Mutex<Vec<SandboxTeardownRecord>>,
}

impl std::fmt::Debug for OciSandboxAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OciSandboxAdapter")
            .field("runtime_bin", &self.config.runtime_bin)
            .field("workspace_root", &"[INTERNAL]")
            .field("cache_root", &"[INTERNAL]")
            .finish_non_exhaustive()
    }
}

impl OciSandboxAdapter {
    pub fn new(
        runner: Arc<dyn CommandRunner>,
        audit: Arc<dyn ExecutionAuditPort>,
        mut config: OciSandboxConfig,
    ) -> Result<Self, SecurityError> {
        if !matches!(config.runtime_bin.as_str(), "docker" | "podman")
            || config.command_timeout.is_zero()
        {
            return Err(SecurityError::Invalid(
                "OCI sandbox requires docker or podman and a positive timeout".to_string(),
            ));
        }
        std::fs::create_dir_all(&config.workspace_root).map_err(filesystem_unavailable)?;
        std::fs::create_dir_all(&config.cache_root).map_err(filesystem_unavailable)?;
        config.workspace_root = config
            .workspace_root
            .canonicalize()
            .map_err(filesystem_unavailable)?;
        config.cache_root = config
            .cache_root
            .canonicalize()
            .map_err(filesystem_unavailable)?;
        config.allowed_input_root = config
            .allowed_input_root
            .canonicalize()
            .map_err(filesystem_unavailable)?;
        for (source_id, source_path) in &mut config.input_sources {
            if source_id.trim().is_empty() || source_id.contains('\0') {
                return Err(SecurityError::Invalid(
                    "sandbox input source id is invalid".to_string(),
                ));
            }
            *source_path = source_path.canonicalize().map_err(filesystem_unavailable)?;
            if !source_path.starts_with(&config.allowed_input_root)
                || source_path.to_string_lossy().contains(',')
            {
                return Err(SecurityError::Denied(
                    SecurityDenialReason::SandboxProfileDenied,
                ));
            }
        }
        Ok(Self {
            runner,
            audit,
            config,
            states: Mutex::new(HashMap::new()),
            teardown_records: Mutex::new(Vec::new()),
        })
    }

    #[must_use]
    pub fn teardown_records(&self) -> Vec<SandboxTeardownRecord> {
        self.teardown_records
            .lock()
            .map_or_else(|_| Vec::new(), |records| records.clone())
    }

    pub fn recover_orphans(&self, observed_at: i64) -> Result<u64, SecurityError> {
        if observed_at < 0 {
            return Err(SecurityError::Invalid(
                "sandbox recovery time must be non-negative".to_string(),
            ));
        }
        let entries =
            std::fs::read_dir(&self.config.workspace_root).map_err(filesystem_unavailable)?;
        let mut recovered = 0_u64;
        for entry in entries {
            let entry = entry.map_err(filesystem_unavailable)?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("sb_")
                || !entry.file_type().map_err(filesystem_unavailable)?.is_dir()
            {
                continue;
            }
            std::fs::remove_dir_all(entry.path()).map_err(|error| {
                SecurityError::Unavailable(format!("sandbox recovery cleanup failed: {error}"))
            })?;
            recovered = recovered.saturating_add(1);
        }
        if let Ok(mut states) = self.states.lock() {
            states.retain(|_, state| state.workspace_path.exists());
        }
        Ok(recovered)
    }

    fn state(&self, sandbox_id: &str) -> Result<Option<SandboxState>, SecurityError> {
        self.states
            .lock()
            .map(|states| states.get(sandbox_id).cloned())
            .map_err(|_| SecurityError::Unavailable("sandbox state lock poisoned".to_string()))
    }
}

#[async_trait::async_trait]
impl ExecutionSandboxPort for OciSandboxAdapter {
    async fn prepare_sandbox(
        &self,
        request: &SandboxPrepareRequest,
    ) -> Result<PreparedSandbox, SecurityError> {
        validate_prepare(request, &self.config)?;
        let sandbox_id = format!("sb_{}", ulid::Ulid::new());
        let workspace_path = self.config.workspace_root.join(&sandbox_id);
        std::fs::create_dir(&workspace_path).map_err(filesystem_unavailable)?;
        for child in ["workspace", "output", "secrets"] {
            if let Err(error) = std::fs::create_dir(workspace_path.join(child)) {
                let _ = std::fs::remove_dir_all(&workspace_path);
                return Err(filesystem_unavailable(error));
            }
        }
        let cache_key = hex::encode(Sha256::digest(
            request.profile.tenant_cache_namespace.as_bytes(),
        ));
        let cache_path = self.config.cache_root.join(cache_key);
        std::fs::create_dir_all(&cache_path).map_err(filesystem_unavailable)?;
        let prepared = PreparedSandbox {
            sandbox_id: sandbox_id.clone(),
            profile: request.profile.clone(),
            created_at: request.admission.issued_at,
            expires_at: request.admission.expires_at,
        };
        let state = SandboxState {
            prepare_admission: request.admission.clone(),
            prepared: prepared.clone(),
            workspace_path,
            cache_path,
        };
        self.states
            .lock()
            .map_err(|_| SecurityError::Unavailable("sandbox state lock poisoned".to_string()))?
            .insert(sandbox_id, state);
        Ok(prepared)
    }

    async fn execute_sandbox(
        &self,
        request: &SandboxExecuteRequest,
    ) -> Result<SandboxExecution, SecurityError> {
        let state = self
            .state(&request.sandbox.sandbox_id)?
            .ok_or(SecurityError::Denied(
                SecurityDenialReason::SandboxProfileDenied,
            ))?;
        validate_execute(request, &state)?;
        let args = build_oci_args(request, &state, &self.config)?;
        let output = match self
            .runner
            .run(
                &self.config.runtime_bin,
                &args,
                RunOpts {
                    cwd: None,
                    env: HashMap::new(),
                    stdin: None,
                    timeout: self.config.command_timeout,
                },
            )
            .await
        {
            Ok(output) => output,
            Err(_error) => {
                self.audit_sandbox_failure(
                    &request.admission,
                    "execution.sandbox_start_failed",
                    request.observed_at,
                )
                .await?;
                return Err(SecurityError::Denied(
                    SecurityDenialReason::SandboxStartFailed,
                ));
            }
        };
        Ok(SandboxExecution {
            exit_code: output.status,
            stdout: output.stdout.into_bytes(),
            stderr: output.stderr.into_bytes(),
        })
    }

    async fn cleanup_sandbox(&self, request: &SandboxCleanupRequest) -> Result<(), SecurityError> {
        if request.observed_at < 0 {
            return Err(SecurityError::Invalid(
                "sandbox cleanup time must be non-negative".to_string(),
            ));
        }
        let Some(state) = self.state(&request.sandbox_id)? else {
            let path = self.config.workspace_root.join(&request.sandbox_id);
            if path.exists() {
                std::fs::remove_dir_all(path).map_err(filesystem_unavailable)?;
            }
            return Ok(());
        };
        match std::fs::remove_dir_all(&state.workspace_path) {
            Ok(()) => {
                self.states
                    .lock()
                    .map_err(|_| {
                        SecurityError::Unavailable("sandbox state lock poisoned".to_string())
                    })?
                    .remove(&request.sandbox_id);
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.states
                    .lock()
                    .map_err(|_| {
                        SecurityError::Unavailable("sandbox state lock poisoned".to_string())
                    })?
                    .remove(&request.sandbox_id);
                Ok(())
            }
            Err(error) => {
                self.teardown_records
                    .lock()
                    .map_err(|_| {
                        SecurityError::Unavailable("teardown record lock poisoned".to_string())
                    })?
                    .push(SandboxTeardownRecord {
                        sandbox_id: request.sandbox_id.clone(),
                        terminal_reason: request.terminal_reason,
                        observed_at: request.observed_at,
                        error_kind: format!("{:?}", error.kind()).to_ascii_lowercase(),
                    });
                self.audit_sandbox_failure(
                    &state.prepare_admission,
                    "execution.sandbox_cleanup_failed",
                    request.observed_at,
                )
                .await?;
                Err(SecurityError::Denied(
                    SecurityDenialReason::SandboxCleanupFailed,
                ))
            }
        }
    }
}

impl OciSandboxAdapter {
    async fn audit_sandbox_failure(
        &self,
        admission: &CapabilityAdmission,
        event_type: &str,
        observed_at: i64,
    ) -> Result<(), SecurityError> {
        let payload = json!({
            "capability_id": admission.id,
            "event_type": event_type,
            "execution_task_id": admission.scope.task_lease_claim.execution_task_id,
            "worker_incarnation_id": admission.scope.worker_incarnation_id,
        });
        let audit_id = agentd_core::types::AuditEventId::new();
        let event = ExecutionAuditAppend {
            id: audit_id.clone(),
            idempotency_scope: format!(
                "sandbox-failure:{}",
                admission.scope.task_lease_claim.execution_task_id
            ),
            idempotency_key: audit_id.to_string(),
            event_type: event_type.to_string(),
            actor_kind: AuditActorKind::Worker,
            actor_ref: admission.scope.worker_incarnation_id.to_string(),
            payload_sha256: sha256_json(&payload)?,
            payload,
            links: audit_links(admission),
            execution_artifact_id: None,
            occurred_at: observed_at,
        };
        self.audit
            .append_audit(&event)
            .await
            .map(|_| ())
            .map_err(|error| {
                SecurityError::Unavailable(format!("required sandbox audit failed: {error}"))
            })
    }
}

fn validate_prepare(
    request: &SandboxPrepareRequest,
    config: &OciSandboxConfig,
) -> Result<(), SecurityError> {
    let profile = &request.profile;
    if request.admission.action != ProtectedAction::SandboxPrepare
        || !matches!(
            request.admission.resource.kind,
            ProtectedResourceKind::Execution
        )
        || request
            .admission
            .scope
            .authorize_resource(&request.admission.resource)
            .is_err()
        || profile.profile_id != request.admission.scope.sandbox_profile_id
        || request.admission.expires_at <= request.admission.issued_at
        || profile.root_filesystem != SandboxRootFilesystem::ReadOnly
        || profile.workspace != SandboxWorkspace::Ephemeral
        || profile.linux_capabilities != SandboxLinuxCapabilities::DropAll
        || profile.privilege_escalation != SandboxPrivilegeEscalation::Denied
        || profile.cache_sharing != SandboxCacheSharing::TenantOnly
        || profile.egress != EgressPolicy::DenyAll
        || profile.seccomp_profile.trim().is_empty()
        || profile.seccomp_profile == "unconfined"
        || profile.limits.pids == 0
        || profile.limits.memory_bytes == 0
        || profile.limits.cpu_millis == 0
        || !valid_image_digest(&profile.image_digest)
        || profile.tenant_cache_namespace != expected_cache_namespace(&request.admission)
        || !runtime_matches(profile.runtime, &config.runtime_bin)
    {
        return Err(SecurityError::Denied(
            SecurityDenialReason::SandboxProfileDenied,
        ));
    }
    let mut targets = HashSet::new();
    for mount in &profile.mounts {
        if mount.access != SandboxMountAccess::ReadOnly
            || !mount.target.starts_with("/workspace/input")
            || mount.target.contains(',')
            || !targets.insert(&mount.target)
            || !config.input_sources.contains_key(&mount.source_id)
        {
            return Err(SecurityError::Denied(
                SecurityDenialReason::SandboxProfileDenied,
            ));
        }
    }
    Ok(())
}

fn validate_execute(
    request: &SandboxExecuteRequest,
    state: &SandboxState,
) -> Result<(), SecurityError> {
    if request.admission.action != ProtectedAction::SandboxExecute
        || !matches!(
            request.admission.resource.kind,
            ProtectedResourceKind::Execution
        )
        || request
            .admission
            .scope
            .authorize_resource(&request.admission.resource)
            .is_err()
        || request.admission.scope != state.prepare_admission.scope
        || request.admission.workload != state.prepare_admission.workload
        || request.admission.resource != state.prepare_admission.resource
        || request.sandbox != state.prepared
        || request.observed_at < request.admission.issued_at
        || request.observed_at >= request.admission.expires_at
        || request.argv.is_empty()
        || request.argv.iter().any(|arg| arg.contains('\0'))
        || request.env.iter().any(|(key, value)| {
            !valid_env_key(key) || value.contains('\0') || contains_sensitive_env_name(key)
        })
    {
        return Err(SecurityError::Denied(
            SecurityDenialReason::SandboxProfileDenied,
        ));
    }
    Ok(())
}

fn build_oci_args(
    request: &SandboxExecuteRequest,
    state: &SandboxState,
    config: &OciSandboxConfig,
) -> Result<Vec<String>, SecurityError> {
    let profile = &request.sandbox.profile;
    let cpu = f64::from(profile.limits.cpu_millis) / 1_000.0;
    let mut args = vec![
        "run".to_string(),
        "--rm".to_string(),
        "--name".to_string(),
        format!("agentd-{}", request.sandbox.sandbox_id),
        "--read-only".to_string(),
        "--cap-drop".to_string(),
        "ALL".to_string(),
        "--security-opt".to_string(),
        "no-new-privileges".to_string(),
        "--security-opt".to_string(),
        format!("seccomp={}", profile.seccomp_profile),
        "--pids-limit".to_string(),
        profile.limits.pids.to_string(),
        "--memory".to_string(),
        profile.limits.memory_bytes.to_string(),
        "--cpus".to_string(),
        format!("{cpu:.3}"),
        "--network".to_string(),
        "none".to_string(),
        "--mount".to_string(),
        mount_arg(&state.workspace_path, "/workspace", false)?,
        "--mount".to_string(),
        mount_arg(&state.cache_path, "/cache", false)?,
        "--env".to_string(),
        format!(
            "AGENTD_TENANT_CACHE_NAMESPACE={}",
            profile.tenant_cache_namespace
        ),
    ];
    for mount in &profile.mounts {
        let source = config
            .input_sources
            .get(&mount.source_id)
            .ok_or(SecurityError::Denied(
                SecurityDenialReason::SandboxProfileDenied,
            ))?;
        args.push("--mount".to_string());
        args.push(mount_arg(source, &mount.target, true)?);
    }
    for (key, value) in &request.env {
        args.push("--env".to_string());
        args.push(format!("{key}={value}"));
    }
    args.push(profile.image_digest.clone());
    args.extend(request.argv.iter().cloned());
    Ok(args)
}

fn mount_arg(source: &Path, target: &str, read_only: bool) -> Result<String, SecurityError> {
    let source = source.to_string_lossy();
    if source.contains(',') || target.contains(',') {
        return Err(SecurityError::Denied(
            SecurityDenialReason::SandboxProfileDenied,
        ));
    }
    Ok(format!(
        "type=bind,src={source},dst={target}{}",
        if read_only { ",readonly" } else { "" }
    ))
}

fn valid_image_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn expected_cache_namespace(admission: &CapabilityAdmission) -> String {
    format!(
        "{}/{}/{}",
        admission.scope.authority_key,
        admission.scope.organization_ref.resource_id(),
        admission.scope.project_ref.resource_id()
    )
}

fn runtime_matches(runtime: OciSandboxRuntime, runtime_bin: &str) -> bool {
    matches!(
        (runtime, runtime_bin),
        (OciSandboxRuntime::Docker, "docker") | (OciSandboxRuntime::Podman, "podman")
    )
}

fn valid_env_key(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn contains_sensitive_env_name(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    ["SECRET", "TOKEN", "PASSWORD", "CREDENTIAL", "PRIVATE_KEY"]
        .iter()
        .any(|needle| upper.contains(needle))
}

fn audit_links(admission: &CapabilityAdmission) -> ExecutionEvidenceLinks {
    let scope = &admission.scope;
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

fn sha256_json(value: &Value) -> Result<String, SecurityError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| SecurityError::Invalid(format!("invalid sandbox audit: {error}")))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn filesystem_unavailable(error: impl std::fmt::Display) -> SecurityError {
    SecurityError::Unavailable(format!("sandbox filesystem unavailable: {error}"))
}
