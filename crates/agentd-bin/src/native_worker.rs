//! Composition-root wiring for the native worker.
//!
//! `agentd-tmux::native` owns disposable PTY resources. This module binds that
//! resource lifecycle to the durable runtime session/attempt repositories.

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use agentd_core::ports::{
    ArtifactIndexPort, ArtifactListRequest, CapabilityAdmission, Clock, ExecutionArtifactKind,
    ExecutionArtifactPublish, ExecutionEvidenceError, ExecutionEvidenceLinks,
    ExecutionSecurityScope, NativeRuntimeAttemptStart, NativeRuntimeAttemptState,
    NativeRuntimeControlPort, PageLimit, ProtectedAction, ProtectedResource, TaskLeaseCloseRequest,
    TaskLeasePort, TaskLeaseRenewRequest, WorkerArtifactAcknowledgement, WorkerArtifactReport,
};
use agentd_core::types::{
    ExecutionArtifactId, NativeExecutionSpec, RunId, RuntimeAttemptId, RuntimeSessionId,
    RuntimeSessionStatus, TaskLeaseClaim, TaskLeaseGrant, WorkerIncarnationId,
};
use agentd_security::{ExecutionSandboxProfile, SandboxLaunchRequest, oci_command};
use agentd_store::content_store::ArtifactObjectStore;
use agentd_store::runtime_session_repo::{self};
use agentd_store::{SqliteStore, StoreError};
use agentd_tmux::native::{
    NativeProcessConfig, NativeProcessEvent, NativeRuntime, NativeRuntimeError, NativeSpoolRecord,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NativeWorkerError {
    #[error("worker fleet error: {0}")]
    Fleet(String),
    #[error("durable runtime state error: {0}")]
    Store(#[from] StoreError),
    #[error("native runtime error: {0}")]
    Native(#[from] NativeRuntimeError),
    #[error("native worker blocking task failed: {0}")]
    Join(String),
    #[error("artifact acknowledgement failed: {0}")]
    Evidence(String),
    #[error("content store error: {0}")]
    ContentStore(String),
    #[error("invalid native recovery request: {0}")]
    InvalidRecovery(String),
}

impl From<ExecutionEvidenceError> for NativeWorkerError {
    fn from(error: ExecutionEvidenceError) -> Self {
        Self::Evidence(error.to_string())
    }
}

#[derive(Clone)]
pub struct AgentdWorker {
    store: SqliteStore,
    runtime_control: Arc<dyn NativeRuntimeControlPort>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeWorkerSecurityBinding {
    pub scope: ExecutionSecurityScope,
    pub capability: CapabilityAdmission,
}

/// Convert a control-plane grant into the binding required by native launch.
/// Remote workers must use the transmitted scope and session identity; they
/// must never reconstruct either from agent names or local tmux state.
pub fn security_binding_from_grant(
    grant: &TaskLeaseGrant,
) -> Result<NativeWorkerSecurityBinding, NativeWorkerError> {
    let scope = grant.security_scope.clone().ok_or_else(|| {
        NativeWorkerError::InvalidRecovery("native grant is missing security scope".into())
    })?;
    if grant.runtime_session_id.is_none() {
        return Err(NativeWorkerError::InvalidRecovery(
            "native grant is missing runtime session id".into(),
        ));
    }
    let capability = CapabilityAdmission {
        action: ProtectedAction::SandboxExecute,
        resource: ProtectedResource::Sandbox(scope.project_ref.resource_id().to_owned()),
        fencing_token: grant.fencing_token,
        scope: scope.clone(),
    };
    Ok(NativeWorkerSecurityBinding { scope, capability })
}

/// Return the complete identity tuple required by a native execution
/// callback. Keeping this as one operation prevents callers from accepting a
/// grant with a binding but no durable session (or vice versa).
pub fn native_grant_execution_context(
    grant: &TaskLeaseGrant,
) -> Result<(RuntimeSessionId, NativeWorkerSecurityBinding), NativeWorkerError> {
    let session_id = grant.runtime_session_id.clone().ok_or_else(|| {
        NativeWorkerError::InvalidRecovery("native grant is missing runtime session id".into())
    })?;
    let binding = security_binding_from_grant(grant)?;
    if binding.scope.lease_claim != grant.claim() {
        return Err(NativeWorkerError::InvalidRecovery(
            "native grant security scope does not match lease claim".into(),
        ));
    }
    Ok((session_id, binding))
}

#[derive(Debug, Clone)]
pub struct NativeRecoveryRequest {
    pub session_id: RuntimeSessionId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub config: NativeProcessConfig,
}

/// Convert a durable execution spec into the only native process config
/// accepted by the worker runtime.
pub fn native_process_config_from_spec(
    spec: &NativeExecutionSpec,
) -> Result<NativeProcessConfig, NativeWorkerError> {
    spec.validate()
        .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?;
    if !spec.provider_matches_program() {
        return Err(NativeWorkerError::InvalidRecovery(
            "execution spec provider and executable do not match".into(),
        ));
    }
    Ok(NativeProcessConfig {
        program: spec.program.clone(),
        args: spec.args.clone(),
        cwd: spec.cwd.clone(),
        env: spec.env.clone(),
        ..NativeProcessConfig::default()
    })
}

#[derive(Debug, Default)]
pub struct NativeRecoveryRegistry {
    requests: Mutex<Vec<NativeRecoveryRequest>>,
}

impl NativeRecoveryRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, request: NativeRecoveryRequest) -> Result<(), NativeWorkerError> {
        if request.config.program.trim().is_empty() {
            return Err(NativeWorkerError::InvalidRecovery(
                "provider program is required".into(),
            ));
        }
        let provider = request
            .config
            .program
            .rsplit('/')
            .next()
            .unwrap_or_default();
        if !matches!(provider, "codex" | "claude" | "claude-code") {
            return Err(NativeWorkerError::InvalidRecovery(format!(
                "unsupported provider executable: {provider}"
            )));
        }
        if let Ok(mut requests) = self.requests.lock() {
            if let Some(existing) = requests
                .iter_mut()
                .find(|existing| existing.session_id == request.session_id)
            {
                *existing = request;
            } else {
                requests.push(request);
            }
            return Ok(());
        }
        Err(NativeWorkerError::Join(
            "recovery registry lock poisoned".into(),
        ))
    }

    /// Rehydrate Codex recovery requests after a daemon restart from durable
    /// `resume_pending` sessions. The native session reference is read later
    /// from the current attempt, so no provider state is kept in memory.
    pub async fn rehydrate_pending_codex(
        &self,
        worker: &AgentdWorker,
        worker_incarnation_id: WorkerIncarnationId,
        config: NativeProcessConfig,
    ) -> Result<usize, NativeWorkerError> {
        let sessions =
            runtime_session_repo::list_resume_pending_sessions(worker.store.pool()).await?;
        for session in &sessions {
            self.register(NativeRecoveryRequest {
                session_id: session.id.clone(),
                worker_incarnation_id: worker_incarnation_id.clone(),
                config: config.clone(),
            })?;
        }
        Ok(sessions.len())
    }

    pub async fn recover_one(
        &self,
        worker: &AgentdWorker,
    ) -> Result<Option<AgentdWorkerHandle>, NativeWorkerError> {
        let candidates = self
            .requests
            .lock()
            .map_err(|_| NativeWorkerError::Join("recovery registry lock poisoned".into()))?
            .len();
        for _ in 0..candidates {
            let request = self
                .requests
                .lock()
                .map_err(|_| NativeWorkerError::Join("recovery registry lock poisoned".into()))?
                .remove(0);
            match worker
                .recover_if_pending(
                    request.session_id.clone(),
                    request.worker_incarnation_id.clone(),
                    request.config.clone(),
                )
                .await
            {
                Ok(Some(handle)) => return Ok(Some(handle)),
                Ok(None) => {
                    // The durable state has already moved on; discard this
                    // stale in-memory recovery request and inspect the next.
                }
                Err(error) => {
                    self.requests
                        .lock()
                        .map_err(|_| {
                            NativeWorkerError::Join("recovery registry lock poisoned".into())
                        })?
                        .insert(0, request);
                    return Err(error);
                }
            }
        }
        Ok(None)
    }
}

/// Build the provider-native Codex resume invocation from a persisted thread id.
pub fn codex_resume_config(
    mut config: NativeProcessConfig,
    thread_ref: String,
) -> NativeProcessConfig {
    config.args = ["exec".to_string(), "resume".to_string(), thread_ref]
        .into_iter()
        .chain(config.args)
        .collect();
    config.native_session_ref = config.args.get(2).cloned();
    config
}

/// Build a provider-native resume invocation without starting the provider.
pub fn provider_resume_config(
    mut config: NativeProcessConfig,
    thread_ref: String,
) -> NativeProcessConfig {
    let executable = config.program.rsplit('/').next().unwrap_or_default();
    if executable == "codex" {
        return codex_resume_config(config, thread_ref);
    }
    if matches!(executable, "claude" | "claude-code") {
        config.args = ["--resume".to_string(), thread_ref]
            .into_iter()
            .chain(config.args)
            .collect();
        config.native_session_ref = config.args.get(1).cloned();
    }
    config
}

pub struct AgentdWorkerHandle {
    store: SqliteStore,
    runtime_control: Arc<dyn NativeRuntimeControlPort>,
    runtime: Arc<NativeRuntime>,
    session_id: RuntimeSessionId,
    attempt_id: RuntimeAttemptId,
    lease_claim: Option<TaskLeaseClaim>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeWorkerRuntimeSnapshot {
    pub status: agentd_tmux::native::NativeProcessStatus,
    pub native_session_ref: Option<String>,
    pub output: Vec<u8>,
}

fn replace_all_bytes(output: &mut Vec<u8>, needle: &[u8], replacement: &[u8]) {
    if needle.is_empty() {
        return;
    }
    let mut cursor = 0;
    while let Some(relative) = output[cursor..]
        .windows(needle.len())
        .position(|window| window == needle)
    {
        let start = cursor + relative;
        output.splice(start..start + needle.len(), replacement.iter().copied());
        cursor = start + replacement.len();
    }
}

/// Background lease renewal owned by the caller for the lifetime of a native
/// execution. It stops on process exit or on the first fenced renewal error.
#[derive(Debug)]
pub struct NativeLeaseRenewal {
    task: tokio::task::JoinHandle<Result<(), NativeWorkerError>>,
}

impl NativeLeaseRenewal {
    pub async fn wait(self) -> Result<(), NativeWorkerError> {
        self.task
            .await
            .map_err(|error| NativeWorkerError::Join(error.to_string()))?
    }

    pub fn abort(&self) {
        self.task.abort();
    }
}

impl AgentdWorker {
    pub(crate) fn store(&self) -> &SqliteStore {
        &self.store
    }
    #[must_use]
    pub fn new(store: SqliteStore) -> Self {
        let runtime_control = Arc::new(
            agentd_store::native_runtime_control_plane::SqliteNativeRuntimeControlPlane::new(
                store.pool().clone(),
            ),
        );
        Self {
            store,
            runtime_control,
        }
    }

    /// Construct a worker whose runtime identity mutations cross the supplied
    /// control-plane port. The SQLite store remains available for daemon-only
    /// concerns such as artifact publication and lease operations.
    pub fn with_runtime_control(
        store: SqliteStore,
        runtime_control: Arc<dyn NativeRuntimeControlPort>,
    ) -> Self {
        Self {
            store,
            runtime_control,
        }
    }

    pub async fn cutover_inventory(
        &self,
    ) -> Result<agentd_store::doctor::CutoverInventory, NativeWorkerError> {
        agentd_store::doctor::OperationalDoctor::new(self.store.pool().clone())
            .cutover_inventory()
            .await
            .map_err(NativeWorkerError::Store)
    }

    pub async fn cutover_project_state(
        &self,
        project_id: &str,
    ) -> Result<Option<agentd_store::cutover_repo::CutoverProjectState>, NativeWorkerError> {
        agentd_store::cutover_repo::get(self.store.pool(), project_id)
            .await
            .map_err(NativeWorkerError::Store)
    }

    pub async fn transition_cutover_project(
        &self,
        project_id: &str,
        phase: agentd_store::cutover_repo::CutoverPhase,
        authority_revision: &str,
        matrix_cursor: i64,
        lease_epoch: i64,
    ) -> Result<agentd_store::cutover_repo::CutoverProjectState, NativeWorkerError> {
        if matches!(
            phase,
            agentd_store::cutover_repo::CutoverPhase::Cutover
                | agentd_store::cutover_repo::CutoverPhase::Drain
                | agentd_store::cutover_repo::CutoverPhase::Retired
        ) {
            let inventory = self.cutover_inventory().await?;
            if !inventory.ready_for_cutover {
                return Err(NativeWorkerError::InvalidRecovery(
                    "cutover gate is not ready: active work, workers, or runtime state remain"
                        .into(),
                ));
            }
        }
        agentd_store::cutover_repo::transition(
            self.store.pool(),
            project_id,
            phase,
            authority_revision,
            matrix_cursor,
            lease_epoch,
        )
        .await
        .map_err(NativeWorkerError::Store)
    }

    pub async fn rollback_cutover_project(
        &self,
        project_id: &str,
        lease_epoch: i64,
    ) -> Result<agentd_store::cutover_repo::CutoverProjectState, NativeWorkerError> {
        agentd_store::cutover_repo::rollback(self.store.pool(), project_id, lease_epoch)
            .await
            .map_err(NativeWorkerError::Store)
    }

    pub async fn drain_until_ready(
        &self,
        timeout_ms: u64,
    ) -> Result<agentd_store::doctor::CutoverInventory, NativeWorkerError> {
        let deadline = tokio::time::Instant::now()
            .checked_add(Duration::from_millis(timeout_ms.min(300_000)))
            .ok_or_else(|| NativeWorkerError::InvalidRecovery("drain timeout overflow".into()))?;
        loop {
            let inventory = self.cutover_inventory().await?;
            if inventory.ready_for_cutover {
                return Ok(inventory);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(NativeWorkerError::InvalidRecovery(format!(
                    "drain timed out: active_leases={}, runtime_running={}, resume_pending={}, workers_draining={}",
                    inventory.active_leases,
                    inventory.runtime_running,
                    inventory.runtime_resume_pending,
                    inventory.workers.draining
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Wait for all durable runs belonging to one project to reach a terminal
    /// state before the project enters the drain phase. The query is scoped to
    /// the project, so unrelated tenants do not block a cutover.
    pub async fn drain_project_until_ready(
        &self,
        project_id: &str,
        timeout_ms: u64,
    ) -> Result<(), NativeWorkerError> {
        if project_id.trim().is_empty() {
            return Err(NativeWorkerError::InvalidRecovery(
                "project id is required for cutover drain".into(),
            ));
        }
        let deadline = tokio::time::Instant::now()
            .checked_add(Duration::from_millis(timeout_ms.min(300_000)))
            .ok_or_else(|| NativeWorkerError::InvalidRecovery("drain timeout overflow".into()))?;
        loop {
            let in_flight =
                agentd_store::run_repo::count_in_flight_for_project(self.store.pool(), project_id)
                    .await?;
            if in_flight == 0 {
                self.drain_until_ready(timeout_ms).await?;
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(NativeWorkerError::InvalidRecovery(format!(
                    "project drain timed out: project_id={project_id}, in_flight_runs={in_flight}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn list_artifacts_for_run(
        &self,
        run_id: &str,
    ) -> Result<agentd_core::ports::ArtifactPage, NativeWorkerError> {
        let run_id = RunId::from_string(run_id.to_owned());
        let lease_port = agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane::new(
            self.store.pool().clone(),
        );
        let evidence = agentd_store::execution_evidence_control_plane::
            SqliteExecutionEvidenceControlPlane::new(self.store.pool().clone(), lease_port);
        evidence
            .list_artifacts(&ArtifactListRequest {
                execution_run_id: run_id,
                cursor: None,
                limit: PageLimit::new(PageLimit::MAX).expect("bounded page limit"),
            })
            .await
            .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))
    }

    pub async fn validate_session_task(
        &self,
        session_id: &RuntimeSessionId,
        task_id: &agentd_core::types::TaskRunId,
    ) -> Result<(), NativeWorkerError> {
        let session = runtime_session_repo::get_session(self.store.pool(), session_id)
            .await?
            .ok_or_else(|| {
                NativeWorkerError::InvalidRecovery("runtime session not found".into())
            })?;
        if session.execution_task_id != *task_id {
            return Err(NativeWorkerError::InvalidRecovery(
                "runtime session task does not match lease task".into(),
            ));
        }
        Ok(())
    }

    pub fn validate_security_binding(
        binding: &NativeWorkerSecurityBinding,
        worker_incarnation_id: &WorkerIncarnationId,
        observed_at: i64,
    ) -> Result<(), NativeWorkerError> {
        binding
            .scope
            .validate()
            .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?;
        if &binding.scope.worker_incarnation_id != worker_incarnation_id
            || binding.capability.scope != binding.scope
            || binding.capability.action != ProtectedAction::SandboxExecute
            || !matches!(binding.capability.resource, ProtectedResource::Sandbox(_))
            || observed_at < binding.scope.valid_from
            || observed_at >= binding.scope.valid_until
        {
            return Err(NativeWorkerError::InvalidRecovery(
                "native worker security binding does not match current scope".to_string(),
            ));
        }
        Ok(())
    }

    pub async fn start_secured(
        &self,
        session_id: RuntimeSessionId,
        worker_incarnation_id: WorkerIncarnationId,
        config: NativeProcessConfig,
        binding: &NativeWorkerSecurityBinding,
        observed_at: i64,
    ) -> Result<AgentdWorkerHandle, NativeWorkerError> {
        Self::validate_security_binding(binding, &worker_incarnation_id, observed_at)?;
        agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane::new(
            self.store.pool().clone(),
        )
        .validate_claim(&binding.scope.lease_claim, observed_at)
        .await
        .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?;
        let runtime = std::env::var("AGENTD_SECURITY_SANDBOX_RUNTIME").map_err(|_| {
            NativeWorkerError::InvalidRecovery(
                "secured native worker requires AGENTD_SECURITY_SANDBOX_RUNTIME".to_string(),
            )
        })?;
        let workspace = config.cwd.clone().unwrap_or_else(|| ".".to_string());
        let profile = ExecutionSandboxProfile {
            image_digest: binding.scope.sandbox_profile.clone(),
            workspace: workspace.clone().into(),
            ephemeral_workspace: false,
            input_mount: None,
            output_mount: workspace.into(),
            egress_profile: binding.scope.egress_profile.clone(),
            memory_bytes: 2 * 1024 * 1024 * 1024,
            cpu_quota: 2,
        };
        profile
            .validate()
            .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?;
        let launch = SandboxLaunchRequest {
            program: config.program.clone(),
            args: config.args.clone(),
            environment: config.env.clone(),
            profile,
        };
        launch
            .validate()
            .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?;
        let (program, args) = oci_command(&runtime, &launch);
        let mut sandbox_config = config;
        sandbox_config.program = program;
        sandbox_config.args = args;
        let mut handle = self
            .start(session_id, worker_incarnation_id, sandbox_config)
            .await?;
        handle.lease_claim = Some(binding.scope.lease_claim.clone());
        Ok(handle)
    }

    /// Start a native process only after creating its durable attempt record.
    /// A spawn failure is reconciled as `gone` before the error is returned.
    pub async fn start(
        &self,
        session_id: RuntimeSessionId,
        worker_incarnation_id: WorkerIncarnationId,
        config: NativeProcessConfig,
    ) -> Result<AgentdWorkerHandle, NativeWorkerError> {
        let task_id = runtime_session_repo::get_session(self.store.pool(), &session_id)
            .await?
            .ok_or_else(|| NativeWorkerError::InvalidRecovery("runtime session not found".into()))?
            .execution_task_id;
        self.start_for_task(session_id, task_id, worker_incarnation_id, config)
            .await
    }

    /// Start a native process through the injected runtime control plane.
    /// Remote workers use this entry point with the task id carried by the
    /// lease grant and never need a local runtime-session database.
    pub async fn start_for_task(
        &self,
        session_id: RuntimeSessionId,
        task_id: agentd_core::types::TaskRunId,
        worker_incarnation_id: WorkerIncarnationId,
        config: NativeProcessConfig,
    ) -> Result<AgentdWorkerHandle, NativeWorkerError> {
        let attempt_id = RuntimeAttemptId::new();
        let observed_at = crate::clock::SystemClock::default().now_unix();
        let start = self
            .runtime_control
            .start_attempt(&NativeRuntimeAttemptStart {
                attempt_id: attempt_id.clone(),
                session_id: session_id.clone(),
                task_id,
                worker_incarnation_id,
                observed_at,
            })
            .await
            .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?;

        let spawn_config = config;
        let runtime =
            match tokio::task::spawn_blocking(move || NativeRuntime::spawn(spawn_config)).await {
                Ok(Ok(runtime)) => runtime,
                Ok(Err(error)) => {
                    let _ = self
                        .runtime_control
                        .mark_attempt_terminal(&NativeRuntimeAttemptState {
                            attempt_id: attempt_id.clone(),
                            session_id: session_id.clone(),
                            status: agentd_core::types::RuntimeAttemptStatus::Gone,
                            native_session_ref: None,
                            observed_at,
                        })
                        .await;
                    return Err(error.into());
                }
                Err(error) => {
                    let _ = self
                        .runtime_control
                        .mark_attempt_terminal(&NativeRuntimeAttemptState {
                            attempt_id: attempt_id.clone(),
                            session_id: session_id.clone(),
                            status: agentd_core::types::RuntimeAttemptStatus::Gone,
                            native_session_ref: None,
                            observed_at,
                        })
                        .await;
                    return Err(NativeWorkerError::Join(error.to_string()));
                }
            };
        let runtime = Arc::new(runtime);
        if let Err(error) = self
            .runtime_control
            .update_attempt(&NativeRuntimeAttemptState {
                attempt_id: attempt_id.clone(),
                session_id: session_id.clone(),
                status: agentd_core::types::RuntimeAttemptStatus::Running,
                native_session_ref: start.native_session_ref.clone(),
                observed_at,
            })
            .await
        {
            let _ = self
                .runtime_control
                .mark_attempt_terminal(&NativeRuntimeAttemptState {
                    attempt_id: attempt_id.clone(),
                    session_id: session_id.clone(),
                    status: agentd_core::types::RuntimeAttemptStatus::Gone,
                    native_session_ref: None,
                    observed_at,
                })
                .await;
            return Err(NativeWorkerError::InvalidRecovery(error.to_string()));
        }
        Ok(AgentdWorkerHandle {
            store: self.store.clone(),
            runtime_control: self.runtime_control.clone(),
            runtime,
            session_id,
            attempt_id,
            lease_claim: None,
        })
    }

    /// Resume a `resume_pending` session using the persisted provider-native
    /// reference when the caller does not supply one.
    pub async fn resume(
        &self,
        session_id: RuntimeSessionId,
        worker_incarnation_id: WorkerIncarnationId,
        mut config: NativeProcessConfig,
    ) -> Result<AgentdWorkerHandle, NativeWorkerError> {
        if config.native_session_ref.is_none() {
            config.native_session_ref = self
                .runtime_control
                .session_view(&session_id)
                .await
                .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?
                .and_then(|view| view.latest_native_session_ref);
        }
        if let Some(thread_ref) = config.native_session_ref.clone() {
            config = provider_resume_config(config, thread_ref);
        }
        self.start(session_id, worker_incarnation_id, config).await
    }

    /// Recover a session only when durable control state explicitly requests
    /// it. The decision reads the control-plane view, so a remote worker can
    /// recover without a local runtime-session database.
    pub async fn recover_if_pending(
        &self,
        session_id: RuntimeSessionId,
        worker_incarnation_id: WorkerIncarnationId,
        config: NativeProcessConfig,
    ) -> Result<Option<AgentdWorkerHandle>, NativeWorkerError> {
        let Some(view) = self
            .runtime_control
            .session_view(&session_id)
            .await
            .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?
        else {
            return Ok(None);
        };
        if view.status != RuntimeSessionStatus::ResumePending {
            return Ok(None);
        }
        self.resume(session_id, worker_incarnation_id, config)
            .await
            .map(Some)
    }
}

impl AgentdWorkerHandle {
    #[must_use]
    pub fn attempt_id(&self) -> &RuntimeAttemptId {
        &self.attempt_id
    }

    #[must_use]
    pub fn runtime(&self) -> &NativeRuntime {
        &self.runtime
    }

    #[must_use]
    pub fn output_snapshot(&self) -> Vec<u8> {
        self.runtime.output()
    }

    #[must_use]
    pub fn process_status(&self) -> agentd_tmux::native::NativeProcessStatus {
        self.runtime.status()
    }

    #[must_use]
    pub fn snapshot(&self) -> NativeWorkerRuntimeSnapshot {
        let (status, native_session_ref, output) = self.runtime.snapshot();
        NativeWorkerRuntimeSnapshot {
            status,
            native_session_ref,
            output,
        }
    }

    #[must_use]
    pub fn native_session_ref(&self) -> Option<String> {
        self.runtime.native_session_ref()
    }

    /// Persist a provider-native session reference discovered from runtime
    /// output or a provider callback while this attempt remains current.
    pub async fn set_native_session_ref(
        &self,
        native_session_ref: &str,
    ) -> Result<(), NativeWorkerError> {
        self.runtime_control
            .update_attempt(&NativeRuntimeAttemptState {
                attempt_id: self.attempt_id.clone(),
                session_id: self.session_id.clone(),
                status: agentd_core::types::RuntimeAttemptStatus::Running,
                native_session_ref: Some(native_session_ref.to_owned()),
                observed_at: crate::clock::SystemClock::default().now_unix(),
            })
            .await
            .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))
    }

    #[must_use]
    pub fn secured_lease_claim(&self) -> Option<&TaskLeaseClaim> {
        self.lease_claim.as_ref()
    }

    pub async fn release_secured_lease(
        &self,
        observed_at: i64,
        reason: impl Into<String>,
    ) -> Result<agentd_core::types::TaskLeaseGrant, NativeWorkerError> {
        let claim = self.lease_claim.as_ref().ok_or_else(|| {
            NativeWorkerError::InvalidRecovery("native handle has no secured lease".into())
        })?;
        self.release_lease(claim, observed_at, reason).await
    }

    pub async fn cancel_secured_lease(
        &self,
        observed_at: i64,
        reason: impl Into<String>,
    ) -> Result<agentd_core::types::TaskLeaseGrant, NativeWorkerError> {
        let claim = self.lease_claim.as_ref().ok_or_else(|| {
            NativeWorkerError::InvalidRecovery("native handle has no secured lease".into())
        })?;
        self.cancel_lease(claim, observed_at, reason).await
    }

    /// Request process termination; callers should still call `wait` to
    /// reconcile the durable attempt outcome.
    pub fn terminate(&self) -> Result<(), NativeWorkerError> {
        self.runtime.terminate().map_err(NativeWorkerError::Native)
    }

    /// Renew the task lease while the native process is still running.
    ///
    /// Renewal is deliberately fenced through the durable control plane; a
    /// stale worker cannot extend a lease after reassignment.
    pub async fn renew_lease(
        &self,
        claim: &TaskLeaseClaim,
        observed_at: i64,
        expires_at: i64,
    ) -> Result<agentd_core::types::TaskLeaseGrant, NativeWorkerError> {
        agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane::new(
            self.store.pool().clone(),
        )
        .renew(&TaskLeaseRenewRequest {
            claim: claim.clone(),
            observed_at,
            expires_at,
        })
        .await
        .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))
    }

    /// Release a completed native execution through the fenced control plane.
    pub async fn release_lease(
        &self,
        claim: &TaskLeaseClaim,
        observed_at: i64,
        reason: impl Into<String>,
    ) -> Result<agentd_core::types::TaskLeaseGrant, NativeWorkerError> {
        agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane::new(
            self.store.pool().clone(),
        )
        .release(&TaskLeaseCloseRequest {
            claim: claim.clone(),
            observed_at,
            reason: reason.into(),
        })
        .await
        .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))
    }

    /// Cancel a native execution through the same fencing boundary.
    pub async fn cancel_lease(
        &self,
        claim: &TaskLeaseClaim,
        observed_at: i64,
        reason: impl Into<String>,
    ) -> Result<agentd_core::types::TaskLeaseGrant, NativeWorkerError> {
        agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane::new(
            self.store.pool().clone(),
        )
        .cancel(&TaskLeaseCloseRequest {
            claim: claim.clone(),
            observed_at,
            reason: reason.into(),
        })
        .await
        .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))
    }

    /// Start periodic, fenced lease renewal for this running process.
    pub fn spawn_lease_renewal(
        &self,
        claim: TaskLeaseClaim,
        interval: Duration,
        lease_duration: Duration,
    ) -> NativeLeaseRenewal {
        let store = self.store.clone();
        let runtime = Arc::clone(&self.runtime);
        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval.max(Duration::from_millis(10)));
            ticker.tick().await;
            loop {
                if runtime.is_terminal() {
                    return Ok(());
                }
                ticker.tick().await;
                let observed_at = crate::clock::SystemClock::default().now_unix();
                let expires_at = observed_at.saturating_add(lease_duration.as_secs() as i64);
                let renewal =
                    agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane::new(
                        store.pool().clone(),
                    )
                    .renew(&TaskLeaseRenewRequest {
                        claim: claim.clone(),
                        observed_at,
                        expires_at,
                    })
                    .await;
                if let Err(error) = renewal {
                    let _ = runtime.terminate();
                    return Err(NativeWorkerError::InvalidRecovery(error.to_string()));
                }
            }
        });
        NativeLeaseRenewal { task }
    }

    /// Persist the bounded PTY output for a later artifact publish/ack.
    pub fn spool_output(
        &self,
        path: impl AsRef<Path>,
    ) -> Result<NativeSpoolRecord, NativeWorkerError> {
        self.runtime
            .spool_output(path)
            .map_err(NativeWorkerError::Native)
    }

    pub fn spool_output_redacted(
        &self,
        path: impl AsRef<Path>,
        secrets: &[agentd_core::ports::SecretMaterial],
    ) -> Result<NativeSpoolRecord, NativeWorkerError> {
        self.runtime
            .spool_output_redacted(path, secrets)
            .map_err(NativeWorkerError::Native)
    }

    /// Store the current output under a content-addressed reference while
    /// retaining the native runtime's bounded snapshot semantics.
    pub fn spool_output_to_store(
        &self,
        store: &dyn ArtifactObjectStore,
    ) -> Result<NativeSpoolRecord, NativeWorkerError> {
        let output = self.runtime.output();
        let stored = store
            .put_bytes(&output)
            .map_err(|error| NativeWorkerError::ContentStore(error.to_string()))?;
        Ok(NativeSpoolRecord {
            storage_ref: stored.storage_ref,
            content_sha256: stored.sha256,
            size_bytes: stored.size_bytes,
        })
    }

    /// Redacted content-store variant for transcripts and logs containing
    /// checked-out secret material.
    pub fn spool_output_redacted_to_store(
        &self,
        store: &dyn ArtifactObjectStore,
        secrets: &[agentd_core::ports::SecretMaterial],
    ) -> Result<NativeSpoolRecord, NativeWorkerError> {
        let mut output = self.runtime.output();
        for secret in secrets {
            if !secret.as_bytes().is_empty() {
                replace_all_bytes(&mut output, secret.as_bytes(), b"[REDACTED]");
            }
        }
        let stored = store
            .put_bytes(&output)
            .map_err(|error| NativeWorkerError::ContentStore(error.to_string()))?;
        Ok(NativeSpoolRecord {
            storage_ref: stored.storage_ref,
            content_sha256: stored.sha256,
            size_bytes: stored.size_bytes,
        })
    }

    /// Construct the immutable artifact envelope for a previously spooled log.
    pub fn spooled_artifact(
        &self,
        record: NativeSpoolRecord,
        id: ExecutionArtifactId,
        kind: ExecutionArtifactKind,
        media_type: String,
        provenance: serde_json::Value,
        links: ExecutionEvidenceLinks,
    ) -> ExecutionArtifactPublish {
        ExecutionArtifactPublish {
            id,
            kind,
            content_sha256: record.content_sha256,
            size_bytes: record.size_bytes,
            media_type,
            storage_ref: record.storage_ref,
            provenance,
            links,
        }
    }

    /// Publish a spooled artifact through the fenced evidence port.
    pub async fn acknowledge_spooled_artifact<P: ArtifactIndexPort + ?Sized>(
        &self,
        port: &P,
        claim: TaskLeaseClaim,
        observed_at: i64,
        artifact: ExecutionArtifactPublish,
    ) -> Result<WorkerArtifactAcknowledgement, NativeWorkerError> {
        port.acknowledge_worker_artifact(&WorkerArtifactReport {
            claim,
            observed_at,
            artifact,
        })
        .await
        .map_err(NativeWorkerError::from)
    }

    /// Retry acknowledgement for an already persisted spool record without
    /// touching the native process or recalculating its content digest.
    pub async fn retry_spooled_artifact<P: ArtifactIndexPort + ?Sized>(
        &self,
        port: &P,
        record: NativeSpoolRecord,
        claim: TaskLeaseClaim,
        observed_at: i64,
        id: ExecutionArtifactId,
        kind: ExecutionArtifactKind,
        media_type: String,
        provenance: serde_json::Value,
        links: ExecutionEvidenceLinks,
    ) -> Result<WorkerArtifactAcknowledgement, NativeWorkerError> {
        let artifact = self.spooled_artifact(record, id, kind, media_type, provenance, links);
        self.acknowledge_spooled_artifact(port, claim, observed_at, artifact)
            .await
    }

    /// Spool bounded PTY output and acknowledge its immutable artifact under
    /// the same fenced claim. The spool remains on disk when acknowledgement
    /// fails, allowing the caller to retry without rerunning the process.
    pub async fn spool_and_acknowledge<P: ArtifactIndexPort + ?Sized>(
        &self,
        port: &P,
        spool_path: impl AsRef<Path>,
        claim: TaskLeaseClaim,
        observed_at: i64,
        id: ExecutionArtifactId,
        kind: ExecutionArtifactKind,
        media_type: String,
        provenance: serde_json::Value,
        links: ExecutionEvidenceLinks,
    ) -> Result<WorkerArtifactAcknowledgement, NativeWorkerError> {
        let record = self.spool_output(spool_path)?;
        let artifact = self.spooled_artifact(record, id, kind, media_type, provenance, links);
        self.acknowledge_spooled_artifact(port, claim, observed_at, artifact)
            .await
    }

    pub async fn spool_and_acknowledge_redacted<P: ArtifactIndexPort + ?Sized>(
        &self,
        port: &P,
        spool_path: impl AsRef<Path>,
        secrets: &[agentd_core::ports::SecretMaterial],
        claim: TaskLeaseClaim,
        observed_at: i64,
        id: ExecutionArtifactId,
        kind: ExecutionArtifactKind,
        media_type: String,
        provenance: serde_json::Value,
        links: ExecutionEvidenceLinks,
    ) -> Result<WorkerArtifactAcknowledgement, NativeWorkerError> {
        let record = self.spool_output_redacted(spool_path, secrets)?;
        let artifact = self.spooled_artifact(record, id, kind, media_type, provenance, links);
        self.acknowledge_spooled_artifact(port, claim, observed_at, artifact)
            .await
    }

    /// Store output content by digest and acknowledge its immutable artifact
    /// under the current fenced lease in one operation.
    pub async fn spool_and_acknowledge_to_store<P: ArtifactIndexPort + ?Sized>(
        &self,
        port: &P,
        content_store: &dyn ArtifactObjectStore,
        claim: TaskLeaseClaim,
        observed_at: i64,
        id: ExecutionArtifactId,
        kind: ExecutionArtifactKind,
        media_type: String,
        provenance: serde_json::Value,
        links: ExecutionEvidenceLinks,
    ) -> Result<WorkerArtifactAcknowledgement, NativeWorkerError> {
        let record = self.spool_output_to_store(content_store)?;
        let artifact = self.spooled_artifact(record, id, kind, media_type, provenance, links);
        self.acknowledge_spooled_artifact(port, claim, observed_at, artifact)
            .await
    }

    /// Redacted content-addressed variant for transcript and log artifacts.
    pub async fn spool_and_acknowledge_redacted_to_store<P: ArtifactIndexPort + ?Sized>(
        &self,
        port: &P,
        content_store: &dyn ArtifactObjectStore,
        secrets: &[agentd_core::ports::SecretMaterial],
        claim: TaskLeaseClaim,
        observed_at: i64,
        id: ExecutionArtifactId,
        kind: ExecutionArtifactKind,
        media_type: String,
        provenance: serde_json::Value,
        links: ExecutionEvidenceLinks,
    ) -> Result<WorkerArtifactAcknowledgement, NativeWorkerError> {
        let record = self.spool_output_redacted_to_store(content_store, secrets)?;
        let artifact = self.spooled_artifact(record, id, kind, media_type, provenance, links);
        self.acknowledge_spooled_artifact(port, claim, observed_at, artifact)
            .await
    }

    /// Terminate the process and reconcile its durable attempt in one operation.
    pub async fn terminate_and_reconcile(
        &self,
        timeout: Duration,
    ) -> Result<NativeProcessEvent, NativeWorkerError> {
        self.terminate()?;
        self.wait(timeout).await
    }

    /// Wait for the native process and atomically reconcile its terminal state.
    pub async fn wait(&self, timeout: Duration) -> Result<NativeProcessEvent, NativeWorkerError> {
        let runtime = Arc::clone(&self.runtime);
        let event = tokio::task::spawn_blocking(move || runtime.wait(timeout))
            .await
            .map_err(|error| NativeWorkerError::Join(error.to_string()))??;
        match &event {
            NativeProcessEvent::Exited { code, .. } => {
                let _ = code;
                self.runtime_control
                    .mark_attempt_terminal(&NativeRuntimeAttemptState {
                        attempt_id: self.attempt_id.clone(),
                        session_id: self.session_id.clone(),
                        status: agentd_core::types::RuntimeAttemptStatus::Exited,
                        native_session_ref: self.native_session_ref(),
                        observed_at: crate::clock::SystemClock::default().now_unix(),
                    })
                    .await
                    .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?;
            }
            NativeProcessEvent::Gone { .. } => {
                self.runtime_control
                    .mark_attempt_terminal(&NativeRuntimeAttemptState {
                        attempt_id: self.attempt_id.clone(),
                        session_id: self.session_id.clone(),
                        status: agentd_core::types::RuntimeAttemptStatus::Gone,
                        native_session_ref: self.native_session_ref(),
                        observed_at: crate::clock::SystemClock::default().now_unix(),
                    })
                    .await
                    .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?;
            }
        }
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        native_grant_execution_context, native_process_config_from_spec, replace_all_bytes,
    };
    use agentd_core::types::{
        FencingToken, LeaseId, LeaseStatus, NativeExecutionSpec, TaskLeaseGrant, TaskRunId,
        WorkerIncarnationId,
    };

    fn codex_spec() -> NativeExecutionSpec {
        NativeExecutionSpec {
            version: 1,
            provider: "codex".into(),
            program: "codex".into(),
            args: vec!["exec".into()],
            cwd: Some("/tmp/project".into()),
            env: vec![("MCP_URL".into(), "http://127.0.0.1:9".into())],
        }
    }

    #[test]
    fn durable_codex_spec_maps_to_native_config() {
        let config = native_process_config_from_spec(&codex_spec()).expect("valid spec");
        assert_eq!(config.program, "codex");
        assert_eq!(config.args, vec!["exec"]);
        assert_eq!(config.cwd.as_deref(), Some("/tmp/project"));
        assert_eq!(config.env[0].0, "MCP_URL");
    }

    #[test]
    fn spec_rejects_provider_executable_mismatch() {
        let mut spec = codex_spec();
        spec.program = "other-agent".into();
        assert!(native_process_config_from_spec(&spec).is_err());
    }

    #[test]
    fn redaction_replaces_every_occurrence_in_binary_output() {
        let mut output = b"xsecretsecret-y".to_vec();
        replace_all_bytes(&mut output, b"secret", b"[REDACTED]");
        assert_eq!(output, b"x[REDACTED][REDACTED]-y");
    }

    #[test]
    fn redaction_ignores_empty_secret() {
        let mut output = b"unchanged".to_vec();
        replace_all_bytes(&mut output, b"", b"[REDACTED]");
        assert_eq!(output, b"unchanged");
    }

    #[test]
    fn native_grant_context_rejects_incomplete_cross_process_grants() {
        let grant = TaskLeaseGrant {
            lease_id: LeaseId::from_string("ls_test"),
            execution_task_id: TaskRunId::from_string("tr_test"),
            worker_incarnation_id: WorkerIncarnationId::from_string("wi_test"),
            fencing_token: FencingToken::new(1).expect("token"),
            status: LeaseStatus::Active,
            acquired_at: 1,
            expires_at: 2,
            renewed_at: None,
            terminal_at: None,
            terminal_reason: None,
            record_version: 1,
            execution_spec: Some(codex_spec()),
            security_scope: None,
            runtime_session_id: None,
        };
        assert!(native_grant_execution_context(&grant).is_err());
    }
}
