//! Fail-closed daemon orchestration for the AD-E5 native runtime.

use std::sync::Arc;

use agentd_core::ports::{
    Clock, ContentRedactionPort, InteractiveSandboxPort, NativeRuntimeError, PolicyRevocationPort,
    RuntimeBackend, RuntimeDimensions, RuntimeEvent, RuntimeEventKind, RuntimeEventPayload,
    RuntimeEventPort, RuntimeKeyInput, RuntimeLaunchRequest, RuntimeLedgerPort,
    RuntimeRecoveryDisposition, RuntimeRecoveryRecord, RuntimeRecoveryRequest,
    RuntimeResizeRequest, RuntimeSandboxCommandRequest, RuntimeSessionRegistration,
    RuntimeShutdownMethod, RuntimeShutdownReport, RuntimeShutdownRequest, RuntimeSnapshot,
    RuntimeTerminalReason, RuntimeTextInput, RuntimeView, RuntimeWaitRequest,
};
use agentd_core::types::{
    CapabilityAdmission, PreparedSandbox, ProtectedAction, RuntimeAttemptId, RuntimeEventId,
    RuntimeSessionId, RuntimeSessionStatus, SecurityCheckpoint, SecurityEpochRequest,
    WorkerIncarnationId,
};
use agentd_runtime::{
    ContentAddressedTranscriptStore, NativePtyRuntime, ProviderCommand, RuntimeProviderAdapter,
};
use agentd_store::{SqliteNativeRuntimeControlPlane, SqliteStore};
use sha2::{Digest, Sha256};

/// Complete input needed to start an initial or provider-native resumed attempt.
#[derive(Debug, Clone)]
pub struct NativeRuntimeStartRequest {
    pub registration: RuntimeSessionRegistration,
    pub attempt_id: RuntimeAttemptId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub host_instance_id: String,
    pub admission: CapabilityAdmission,
    pub sandbox: PreparedSandbox,
    pub provider_command: ProviderCommand,
    pub dimensions: RuntimeDimensions,
}

/// Shared read model consumed by HTTP, agentctl, Matrix, and Robrix projections.
pub type NativeRuntimeView = RuntimeView;

/// Native runtime lifecycle service. Missing providers cannot be represented.
pub struct NativeRuntimeService {
    backend: Arc<dyn RuntimeBackend>,
    ledger: Arc<dyn RuntimeLedgerPort>,
    sandbox: Arc<dyn InteractiveSandboxPort>,
    policy_revocation: Arc<dyn PolicyRevocationPort>,
    trusted_clock: Arc<dyn Clock>,
}

#[derive(Debug, Clone)]
pub struct NativeRuntimeCompositionConfig {
    pub transcript_root: std::path::PathBuf,
    pub max_transcript_object_bytes: u64,
}

/// Compose the production native runtime from durable and injected providers.
pub fn compose_native_runtime(
    store: &SqliteStore,
    sandbox: Arc<dyn InteractiveSandboxPort>,
    redactor: Arc<dyn ContentRedactionPort>,
    policy_revocation: Arc<dyn PolicyRevocationPort>,
    trusted_clock: Arc<dyn Clock>,
    config: &NativeRuntimeCompositionConfig,
) -> Result<Arc<NativeRuntimeService>, NativeRuntimeError> {
    let ledger = Arc::new(SqliteNativeRuntimeControlPlane::new(store.pool().clone()));
    let archive = Arc::new(ContentAddressedTranscriptStore::new(
        &config.transcript_root,
        config.max_transcript_object_bytes,
    )?);
    let event_port: Arc<dyn RuntimeEventPort> = ledger.clone();
    let backend: Arc<dyn RuntimeBackend> =
        Arc::new(NativePtyRuntime::new(redactor, archive, event_port));
    let ledger_port: Arc<dyn RuntimeLedgerPort> = ledger;
    Ok(Arc::new(NativeRuntimeService::new(
        backend,
        ledger_port,
        sandbox,
        policy_revocation,
        trusted_clock,
    )))
}

impl std::fmt::Debug for NativeRuntimeService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeRuntimeService")
            .finish_non_exhaustive()
    }
}

impl NativeRuntimeService {
    #[must_use]
    pub fn new(
        backend: Arc<dyn RuntimeBackend>,
        ledger: Arc<dyn RuntimeLedgerPort>,
        sandbox: Arc<dyn InteractiveSandboxPort>,
        policy_revocation: Arc<dyn PolicyRevocationPort>,
        trusted_clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            backend,
            ledger,
            sandbox,
            policy_revocation,
            trusted_clock,
        }
    }

    #[allow(clippy::too_many_lines)]
    pub async fn start(
        &self,
        request: NativeRuntimeStartRequest,
    ) -> Result<NativeRuntimeView, NativeRuntimeError> {
        let observed_at = self.trusted_now()?;
        self.validate_start(&request, observed_at).await?;
        let resume_reference = self
            .resume_reference(&request.registration.session_id)
            .await?;
        let provider_command = RuntimeProviderAdapter::command(
            &request.provider_command,
            resume_reference.as_deref(),
        )?;
        let sandbox_command = self
            .sandbox
            .interactive_command(&RuntimeSandboxCommandRequest {
                admission: request.admission.clone(),
                sandbox: request.sandbox.clone(),
                argv: std::iter::once(provider_command.program.clone())
                    .chain(provider_command.arguments.iter().cloned())
                    .collect(),
                environment: provider_command.environment.clone(),
                working_directory: provider_command.working_directory.clone(),
                observed_at,
            })
            .await?;
        let session = self
            .ledger
            .register_runtime_session(&request.registration)
            .await?;
        if session.current_attempt_id.as_ref() == Some(&request.attempt_id) {
            if let Some(snapshot) = self.backend.snapshot(&request.attempt_id).await? {
                return self
                    .view(&request.registration.session_id, Some(snapshot))
                    .await;
            }
            return Err(NativeRuntimeError::Conflict(
                "runtime attempt is durable but not owned by this host; recover it first"
                    .to_string(),
            ));
        }
        self.ledger
            .begin_runtime_attempt(
                &request.registration.session_id,
                &request.attempt_id,
                &request.worker_incarnation_id,
                &request.host_instance_id,
                observed_at,
            )
            .await?;
        let native_session_ref = session.native_session_ref.clone();
        let handle = match self
            .backend
            .launch(RuntimeLaunchRequest {
                session_id: request.registration.session_id.clone(),
                attempt_id: request.attempt_id.clone(),
                worker_incarnation_id: request.worker_incarnation_id,
                provider: request.registration.provider,
                command: sandbox_command,
                dimensions: request.dimensions,
                sandbox: request.registration.sandbox.clone(),
                native_session_ref: native_session_ref.clone(),
                max_capture_bytes: usize::try_from(request.registration.max_capture_bytes)
                    .map_err(|_| {
                        NativeRuntimeError::Invalid(
                            "runtime capture bound exceeds this host".to_string(),
                        )
                    })?,
                max_transcript_bytes: request.registration.max_transcript_bytes,
                idle_timeout_ms: request.registration.idle_timeout_ms,
                requested_at: observed_at,
            })
            .await
        {
            Ok(handle) => handle,
            Err(error) => {
                self.ledger
                    .mark_runtime_attempt_gone(
                        &request.registration.session_id,
                        &request.attempt_id,
                        native_session_ref.as_deref(),
                        observed_at,
                    )
                    .await?;
                return Err(error);
            }
        };
        if let Err(error) = self.ledger.mark_runtime_attempt_running(&handle).await {
            let _ = self
                .backend
                .shutdown(RuntimeShutdownRequest {
                    attempt_id: request.attempt_id.clone(),
                    idempotency_key: format!("ledger-start-failure:{observed_at}"),
                    graceful_timeout_ms: 0,
                    interrupt_timeout_ms: 0,
                    reason: RuntimeTerminalReason::Failed,
                    observed_at,
                })
                .await;
            return Err(error);
        }
        tokio::spawn(reconcile_runtime_attempt(
            Arc::clone(&self.backend),
            Arc::clone(&self.ledger),
            request.attempt_id,
        ));
        self.view(&request.registration.session_id, None).await
    }

    pub async fn send_text(
        &self,
        admission: &CapabilityAdmission,
        mut request: RuntimeTextInput,
    ) -> Result<agentd_core::ports::RuntimeInputAck, NativeRuntimeError> {
        request.observed_at = self.guard_attempt(admission, &request.attempt_id).await?;
        let ack = self.backend.send_text(request).await?;
        self.synchronize_attempt(&ack.attempt_id).await?;
        Ok(ack)
    }

    pub async fn send_key(
        &self,
        admission: &CapabilityAdmission,
        mut request: RuntimeKeyInput,
    ) -> Result<agentd_core::ports::RuntimeInputAck, NativeRuntimeError> {
        request.observed_at = self.guard_attempt(admission, &request.attempt_id).await?;
        let ack = self.backend.send_key(request).await?;
        self.synchronize_attempt(&ack.attempt_id).await?;
        Ok(ack)
    }

    pub async fn resize(
        &self,
        admission: &CapabilityAdmission,
        mut request: RuntimeResizeRequest,
    ) -> Result<RuntimeSnapshot, NativeRuntimeError> {
        request.observed_at = self.guard_attempt(admission, &request.attempt_id).await?;
        let snapshot = self.backend.resize(request).await?;
        self.synchronize_snapshot(&snapshot).await?;
        Ok(snapshot)
    }

    pub async fn interrupt(
        &self,
        admission: &CapabilityAdmission,
        mut request: RuntimeKeyInput,
    ) -> Result<agentd_core::ports::RuntimeInputAck, NativeRuntimeError> {
        request.observed_at = self.guard_attempt(admission, &request.attempt_id).await?;
        let ack = self.backend.interrupt(request).await?;
        self.synchronize_attempt(&ack.attempt_id).await?;
        Ok(ack)
    }

    pub async fn shutdown(
        &self,
        admission: &CapabilityAdmission,
        mut request: RuntimeShutdownRequest,
    ) -> Result<RuntimeShutdownReport, NativeRuntimeError> {
        request.observed_at = self.guard_attempt(admission, &request.attempt_id).await?;
        let report = self.backend.shutdown(request).await?;
        self.ledger.finish_runtime_attempt(&report).await?;
        Ok(report)
    }

    pub async fn snapshot(
        &self,
        session_id: &RuntimeSessionId,
    ) -> Result<NativeRuntimeView, NativeRuntimeError> {
        self.view(session_id, None).await
    }

    pub async fn wait(
        &self,
        request: RuntimeWaitRequest,
    ) -> Result<NativeRuntimeView, NativeRuntimeError> {
        let snapshot = self.backend.wait(request).await?;
        self.synchronize_snapshot(&snapshot).await?;
        let session_id = snapshot.session_id.clone();
        self.view(&session_id, Some(snapshot)).await
    }

    pub async fn events_after(
        &self,
        session_id: &RuntimeSessionId,
        after_event_index: u64,
        limit: u32,
    ) -> Result<Vec<RuntimeEvent>, NativeRuntimeError> {
        self.ledger
            .runtime_events_after(session_id, after_event_index, limit)
            .await
    }

    pub async fn recover_startup(
        &self,
        limit: u32,
    ) -> Result<Vec<RuntimeRecoveryRecord>, NativeRuntimeError> {
        let observed_at = self.trusted_now()?;
        let sessions = self.ledger.recoverable_runtime_sessions(limit).await?;
        let mut records = Vec::with_capacity(sessions.len());
        for session in sessions {
            let attempt_id = session.current_attempt_id.clone().ok_or_else(|| {
                NativeRuntimeError::Unavailable(format!(
                    "recoverable runtime session {} has no current attempt",
                    session.registration.session_id
                ))
            })?;
            let attempt = self
                .ledger
                .load_runtime_attempt(&attempt_id)
                .await?
                .ok_or_else(|| {
                    NativeRuntimeError::Unavailable(format!(
                        "recoverable runtime attempt {attempt_id} is missing"
                    ))
                })?;
            let disposition = self
                .backend
                .recover(&RuntimeRecoveryRequest {
                    session_id: session.registration.session_id.clone(),
                    attempt_id: attempt_id.clone(),
                    provider: session.registration.provider,
                    pid: attempt.pid,
                    native_session_ref: session
                        .native_session_ref
                        .clone()
                        .or(attempt.native_session_ref.clone()),
                    observed_at,
                })
                .await?;
            match &disposition {
                RuntimeRecoveryDisposition::Live { snapshot } => {
                    self.synchronize_snapshot(snapshot).await?;
                }
                RuntimeRecoveryDisposition::Resumable { native_session_ref } => {
                    if session.status != RuntimeSessionStatus::ResumePending {
                        self.ledger
                            .mark_runtime_attempt_gone(
                                &session.registration.session_id,
                                &attempt_id,
                                Some(native_session_ref),
                                observed_at,
                            )
                            .await?;
                    }
                }
                RuntimeRecoveryDisposition::RuntimeGone => {
                    self.ledger
                        .mark_runtime_attempt_gone(
                            &session.registration.session_id,
                            &attempt_id,
                            None,
                            observed_at,
                        )
                        .await?;
                }
            }
            let record = RuntimeRecoveryRecord {
                session_id: session.registration.session_id.clone(),
                previous_attempt_id: attempt_id,
                next_attempt_id: None,
                disposition,
                observed_at,
            };
            self.ledger.record_runtime_recovery(&record).await?;
            self.append_recovery_event(&record).await?;
            records.push(record);
        }
        Ok(records)
    }

    pub async fn reap_idle(&self) -> Result<Vec<RuntimeShutdownReport>, NativeRuntimeError> {
        let observed_at = self.trusted_now()?;
        let reports = self.backend.reap_idle(observed_at).await?;
        for report in &reports {
            self.ledger.finish_runtime_attempt(report).await?;
        }
        Ok(reports)
    }

    async fn validate_start(
        &self,
        request: &NativeRuntimeStartRequest,
        observed_at: i64,
    ) -> Result<(), NativeRuntimeError> {
        let profile_bytes = serde_json::to_vec(&request.sandbox.profile).map_err(|error| {
            NativeRuntimeError::Invalid(format!("sandbox profile is invalid: {error}"))
        })?;
        if request.registration.provider != request.provider_command.provider
            || provider_command_sha256(&request.provider_command)
                != request.registration.command_sha256
            || request.registration.snapshot_ref != request.admission.scope.execution_snapshot_ref
            || request.registration.snapshot_content_sha256
                != request
                    .admission
                    .scope
                    .audit_context
                    .snapshot_content_sha256
            || request.registration.execution_task_id
                != request.admission.scope.task_lease_claim.execution_task_id
            || request.worker_incarnation_id != request.admission.scope.worker_incarnation_id
            || request.worker_incarnation_id
                != request
                    .admission
                    .scope
                    .task_lease_claim
                    .worker_incarnation_id
            || request.registration.sandbox.sandbox_id != request.sandbox.sandbox_id
            || request.registration.sandbox.profile_sha256 != sha256(&profile_bytes)
            || request.registration.sandbox.expires_at != request.sandbox.expires_at
            || request.registration.created_at > observed_at
        {
            return Err(NativeRuntimeError::Denied(
                "runtime launch is not bound to the admitted execution snapshot and sandbox"
                    .to_string(),
            ));
        }
        self.check_admission(
            &request.admission,
            observed_at,
            SecurityCheckpoint::Dispatch,
        )
        .await
    }

    async fn guard_attempt(
        &self,
        admission: &CapabilityAdmission,
        attempt_id: &RuntimeAttemptId,
    ) -> Result<i64, NativeRuntimeError> {
        let observed_at = self.trusted_now()?;
        let attempt = self
            .ledger
            .load_runtime_attempt(attempt_id)
            .await?
            .ok_or_else(|| NativeRuntimeError::NotFound(format!("runtime attempt {attempt_id}")))?;
        let session = self
            .ledger
            .load_runtime_session(&attempt.session_id)
            .await?
            .ok_or_else(|| {
                NativeRuntimeError::NotFound(format!("runtime session {}", attempt.session_id))
            })?;
        if !attempt.is_current
            || admission.scope.worker_incarnation_id != attempt.worker_incarnation_id
            || admission.scope.execution_snapshot_ref != session.registration.snapshot_ref
            || admission.scope.task_lease_claim.execution_task_id
                != session.registration.execution_task_id
            || session.registration.sandbox.expires_at <= observed_at
        {
            return Err(NativeRuntimeError::Denied(
                "runtime mutation is outside the current admitted attempt".to_string(),
            ));
        }
        self.check_admission(admission, observed_at, SecurityCheckpoint::LeaseRenewal)
            .await?;
        Ok(observed_at)
    }

    async fn check_admission(
        &self,
        admission: &CapabilityAdmission,
        observed_at: i64,
        checkpoint: SecurityCheckpoint,
    ) -> Result<(), NativeRuntimeError> {
        if admission.action != ProtectedAction::SandboxExecute
            || observed_at < admission.issued_at
            || observed_at >= admission.expires_at
            || observed_at >= admission.scope.valid_until
        {
            return Err(NativeRuntimeError::Denied(
                "runtime capability is not valid at trusted time".to_string(),
            ));
        }
        admission
            .scope
            .authorize_resource(&admission.resource)
            .map_err(|reason| NativeRuntimeError::Denied(reason.to_string()))?;
        let epoch_request = SecurityEpochRequest {
            checkpoint,
            organization_ref: admission.scope.organization_ref.clone(),
            project_ref: admission.scope.project_ref.clone(),
            execution_snapshot_ref: admission.scope.execution_snapshot_ref.clone(),
            pinned_epoch: admission.scope.policy_revocation_epoch,
            observed_at,
        };
        let status = self
            .policy_revocation
            .check_security_epoch(&epoch_request)
            .await
            .map_err(native_security_error)?;
        status
            .validate_request(&epoch_request)
            .and_then(|()| status.validate_pinned_epoch(epoch_request.pinned_epoch))
            .map_err(|reason| NativeRuntimeError::Denied(reason.to_string()))
    }

    async fn resume_reference(
        &self,
        session_id: &RuntimeSessionId,
    ) -> Result<Option<String>, NativeRuntimeError> {
        match self.ledger.load_runtime_session(session_id).await? {
            Some(session) if session.status == RuntimeSessionStatus::ResumePending => {
                session.native_session_ref.map(Some).ok_or_else(|| {
                    NativeRuntimeError::Unavailable(
                        "resume-pending runtime session has no provider reference".to_string(),
                    )
                })
            }
            Some(session) if session.status != RuntimeSessionStatus::Requested => {
                Err(NativeRuntimeError::Conflict(format!(
                    "runtime session cannot start from {}",
                    session.status
                )))
            }
            _ => Ok(None),
        }
    }

    async fn synchronize_attempt(
        &self,
        attempt_id: &RuntimeAttemptId,
    ) -> Result<(), NativeRuntimeError> {
        if let Some(snapshot) = self.backend.snapshot(attempt_id).await? {
            self.synchronize_snapshot(&snapshot).await?;
        }
        Ok(())
    }

    async fn synchronize_snapshot(
        &self,
        snapshot: &RuntimeSnapshot,
    ) -> Result<(), NativeRuntimeError> {
        synchronize_runtime_snapshot(self.ledger.as_ref(), snapshot).await
    }

    async fn view(
        &self,
        session_id: &RuntimeSessionId,
        known_snapshot: Option<RuntimeSnapshot>,
    ) -> Result<NativeRuntimeView, NativeRuntimeError> {
        let mut session = self
            .ledger
            .load_runtime_session(session_id)
            .await?
            .ok_or_else(|| NativeRuntimeError::NotFound(format!("runtime session {session_id}")))?;
        let attempt = match session.current_attempt_id.as_ref() {
            Some(attempt_id) => self.ledger.load_runtime_attempt(attempt_id).await?,
            None => None,
        };
        let live = match (known_snapshot, attempt.as_ref()) {
            (Some(snapshot), _) => Some(snapshot),
            (None, Some(attempt)) => self.backend.snapshot(&attempt.attempt_id).await?,
            (None, None) => None,
        };
        if let Some(snapshot) = &live {
            self.synchronize_snapshot(snapshot).await?;
            session = self
                .ledger
                .load_runtime_session(session_id)
                .await?
                .ok_or_else(|| {
                    NativeRuntimeError::Unavailable("runtime session disappeared".to_string())
                })?;
        }
        let attempt = match session.current_attempt_id.as_ref() {
            Some(attempt_id) => self.ledger.load_runtime_attempt(attempt_id).await?,
            None => None,
        };
        Ok(RuntimeView {
            session,
            attempt,
            live,
        })
    }

    async fn append_recovery_event(
        &self,
        record: &RuntimeRecoveryRecord,
    ) -> Result<(), NativeRuntimeError> {
        let disposition = match record.disposition {
            RuntimeRecoveryDisposition::Live { .. } => "live",
            RuntimeRecoveryDisposition::Resumable { .. } => "resumable",
            RuntimeRecoveryDisposition::RuntimeGone => "runtime_gone",
        };
        let payload = RuntimeEventPayload::Recovery {
            disposition: disposition.to_string(),
        };
        let payload_sha256 = sha256(&serde_json::to_vec(&payload).map_err(|error| {
            NativeRuntimeError::Invalid(format!("runtime recovery event is invalid: {error}"))
        })?);
        self.ledger
            .append_runtime_event(&RuntimeEvent {
                id: RuntimeEventId::new(),
                session_id: record.session_id.clone(),
                attempt_id: record.previous_attempt_id.clone(),
                event_index: 1,
                kind: if matches!(record.disposition, RuntimeRecoveryDisposition::RuntimeGone) {
                    RuntimeEventKind::RuntimeGone
                } else {
                    RuntimeEventKind::Recovered
                },
                payload,
                payload_sha256,
                occurred_at: record.observed_at,
            })
            .await?;
        Ok(())
    }

    fn trusted_now(&self) -> Result<i64, NativeRuntimeError> {
        let observed_at = self.trusted_clock.now_unix();
        if observed_at < 0 {
            Err(NativeRuntimeError::Unavailable(
                "trusted clock returned an invalid timestamp".to_string(),
            ))
        } else {
            Ok(observed_at)
        }
    }
}

async fn reconcile_runtime_attempt(
    backend: Arc<dyn RuntimeBackend>,
    ledger: Arc<dyn RuntimeLedgerPort>,
    attempt_id: RuntimeAttemptId,
) {
    let mut after_event_index = 0;
    loop {
        let snapshot = match backend
            .wait(RuntimeWaitRequest {
                attempt_id: attempt_id.clone(),
                after_event_index,
                timeout_ms: 300_000,
            })
            .await
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                tracing::warn!(
                    runtime_attempt_id = %attempt_id,
                    %error,
                    "native runtime terminal reconciler stopped"
                );
                return;
            }
        };
        after_event_index = after_event_index.max(snapshot.event_index);
        let terminal = snapshot.status.is_terminal();
        if let Err(error) = synchronize_runtime_snapshot(ledger.as_ref(), &snapshot).await {
            tracing::warn!(
                runtime_attempt_id = %attempt_id,
                %error,
                "native runtime terminal snapshot could not be persisted"
            );
            return;
        }
        if terminal {
            return;
        }
    }
}

async fn synchronize_runtime_snapshot(
    ledger: &dyn RuntimeLedgerPort,
    snapshot: &RuntimeSnapshot,
) -> Result<(), NativeRuntimeError> {
    if let Some(reference) = snapshot.native_session_ref.as_deref() {
        let attempt = ledger
            .load_runtime_attempt(&snapshot.attempt_id)
            .await?
            .ok_or_else(|| {
                NativeRuntimeError::NotFound(format!("runtime attempt {}", snapshot.attempt_id))
            })?;
        if attempt.native_session_ref.is_none() {
            ledger
                .update_runtime_native_ref(
                    &snapshot.session_id,
                    &snapshot.attempt_id,
                    reference,
                    snapshot.last_output_at,
                )
                .await?;
        }
    }
    if snapshot.status.is_terminal() {
        let transcript = snapshot.transcript.clone().ok_or_else(|| {
            NativeRuntimeError::Unavailable(
                "terminal runtime snapshot has no archived transcript".to_string(),
            )
        })?;
        let finished_at = snapshot.finished_at.ok_or_else(|| {
            NativeRuntimeError::Unavailable(
                "terminal runtime snapshot has no completion timestamp".to_string(),
            )
        })?;
        ledger
            .finish_runtime_attempt(&RuntimeShutdownReport {
                session_id: snapshot.session_id.clone(),
                attempt_id: snapshot.attempt_id.clone(),
                method: RuntimeShutdownMethod::AlreadyExited,
                terminal_reason: if snapshot.exit_code == Some(0) {
                    RuntimeTerminalReason::Completed
                } else {
                    RuntimeTerminalReason::Failed
                },
                exit_code: snapshot.exit_code,
                transcript,
                finished_at,
            })
            .await?;
    }
    Ok(())
}

/// Stable digest used by registration without persisting environment values.
#[must_use]
pub fn provider_command_sha256(command: &ProviderCommand) -> String {
    let mut hasher = Sha256::new();
    hash_field(&mut hasher, command.provider.as_str().as_bytes());
    hash_field(&mut hasher, command.program.as_bytes());
    for argument in &command.arguments {
        hash_field(&mut hasher, argument.as_bytes());
    }
    for (key, value) in &command.environment {
        hash_field(&mut hasher, key.as_bytes());
        hash_field(&mut hasher, value.as_bytes());
    }
    hash_field(
        &mut hasher,
        command.working_directory.to_string_lossy().as_bytes(),
    );
    if let Some(arguments) = &command.custom_resume_arguments {
        for argument in arguments {
            hash_field(&mut hasher, argument.as_bytes());
        }
    }
    hex::encode(hasher.finalize())
}

fn hash_field(hasher: &mut Sha256, bytes: &[u8]) {
    hasher.update((bytes.len() as u64).to_be_bytes());
    hasher.update(bytes);
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn native_security_error(error: agentd_core::ports::SecurityError) -> NativeRuntimeError {
    match error {
        agentd_core::ports::SecurityError::Denied(reason) => {
            NativeRuntimeError::Denied(reason.to_string())
        }
        agentd_core::ports::SecurityError::Invalid(message) => NativeRuntimeError::Invalid(message),
        agentd_core::ports::SecurityError::Unavailable(message) => {
            NativeRuntimeError::Unavailable(message)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use agentd_core::ports::{
        DurableRuntimeAttempt, DurableRuntimeSession, RuntimeHandle, RuntimeInputAck,
        RuntimeProvider, RuntimeRecoveryRequest, RuntimeSandboxRef, RuntimeTranscriptRef,
    };
    use agentd_core::types::{
        AgentProfileId, AuthorityKey, ProjectExecutionSnapshotRef, RuntimeAttemptStatus,
        RuntimeSessionStatus, RuntimeTranscriptId, TaskRunId,
    };

    use super::*;

    #[derive(Debug)]
    struct ReconcileBackend {
        snapshots: Mutex<VecDeque<RuntimeSnapshot>>,
        cursors: Mutex<Vec<u64>>,
    }

    #[async_trait::async_trait]
    impl RuntimeBackend for ReconcileBackend {
        async fn launch(
            &self,
            _request: RuntimeLaunchRequest,
        ) -> Result<RuntimeHandle, NativeRuntimeError> {
            unreachable!("launch is not used by the reconciler test")
        }

        async fn send_text(
            &self,
            _request: RuntimeTextInput,
        ) -> Result<RuntimeInputAck, NativeRuntimeError> {
            unreachable!("send_text is not used by the reconciler test")
        }

        async fn send_key(
            &self,
            _request: RuntimeKeyInput,
        ) -> Result<RuntimeInputAck, NativeRuntimeError> {
            unreachable!("send_key is not used by the reconciler test")
        }

        async fn resize(
            &self,
            _request: RuntimeResizeRequest,
        ) -> Result<RuntimeSnapshot, NativeRuntimeError> {
            unreachable!("resize is not used by the reconciler test")
        }

        async fn interrupt(
            &self,
            _request: RuntimeKeyInput,
        ) -> Result<RuntimeInputAck, NativeRuntimeError> {
            unreachable!("interrupt is not used by the reconciler test")
        }

        async fn shutdown(
            &self,
            _request: RuntimeShutdownRequest,
        ) -> Result<RuntimeShutdownReport, NativeRuntimeError> {
            unreachable!("shutdown is not used by the reconciler test")
        }

        async fn snapshot(
            &self,
            _attempt_id: &RuntimeAttemptId,
        ) -> Result<Option<RuntimeSnapshot>, NativeRuntimeError> {
            unreachable!("snapshot is not used by the reconciler test")
        }

        async fn events_after(
            &self,
            _attempt_id: &RuntimeAttemptId,
            _after_event_index: u64,
            _limit: u32,
        ) -> Result<Vec<RuntimeEvent>, NativeRuntimeError> {
            unreachable!("events_after is not used by the reconciler test")
        }

        async fn wait(
            &self,
            request: RuntimeWaitRequest,
        ) -> Result<RuntimeSnapshot, NativeRuntimeError> {
            self.cursors
                .lock()
                .expect("cursor lock")
                .push(request.after_event_index);
            self.snapshots
                .lock()
                .expect("snapshot lock")
                .pop_front()
                .ok_or_else(|| NativeRuntimeError::Unavailable("no test snapshot".to_string()))
        }

        async fn recover(
            &self,
            _request: &RuntimeRecoveryRequest,
        ) -> Result<RuntimeRecoveryDisposition, NativeRuntimeError> {
            unreachable!("recover is not used by the reconciler test")
        }

        async fn reap_idle(
            &self,
            _observed_at: i64,
        ) -> Result<Vec<RuntimeShutdownReport>, NativeRuntimeError> {
            unreachable!("reap_idle is not used by the reconciler test")
        }
    }

    #[derive(Debug, Default)]
    struct RecordingLedger {
        reports: Mutex<Vec<RuntimeShutdownReport>>,
    }

    #[async_trait::async_trait]
    impl RuntimeEventPort for RecordingLedger {
        async fn append_runtime_event(
            &self,
            _event: &RuntimeEvent,
        ) -> Result<RuntimeEvent, NativeRuntimeError> {
            unreachable!("append_runtime_event is not used by the reconciler test")
        }

        async fn runtime_events_after(
            &self,
            _session_id: &RuntimeSessionId,
            _after_event_index: u64,
            _limit: u32,
        ) -> Result<Vec<RuntimeEvent>, NativeRuntimeError> {
            unreachable!("runtime_events_after is not used by the reconciler test")
        }
    }

    #[async_trait::async_trait]
    impl RuntimeLedgerPort for RecordingLedger {
        async fn register_runtime_session(
            &self,
            _registration: &RuntimeSessionRegistration,
        ) -> Result<DurableRuntimeSession, NativeRuntimeError> {
            unreachable!("register_runtime_session is not used by the reconciler test")
        }

        async fn begin_runtime_attempt(
            &self,
            _session_id: &RuntimeSessionId,
            _attempt_id: &RuntimeAttemptId,
            _worker_incarnation_id: &WorkerIncarnationId,
            _host_instance_id: &str,
            _started_at: i64,
        ) -> Result<DurableRuntimeAttempt, NativeRuntimeError> {
            unreachable!("begin_runtime_attempt is not used by the reconciler test")
        }

        async fn mark_runtime_attempt_running(
            &self,
            _handle: &RuntimeHandle,
        ) -> Result<DurableRuntimeAttempt, NativeRuntimeError> {
            unreachable!("mark_runtime_attempt_running is not used by the reconciler test")
        }

        async fn update_runtime_native_ref(
            &self,
            _session_id: &RuntimeSessionId,
            _attempt_id: &RuntimeAttemptId,
            _native_session_ref: &str,
            _observed_at: i64,
        ) -> Result<DurableRuntimeAttempt, NativeRuntimeError> {
            unreachable!("update_runtime_native_ref is not used by the reconciler test")
        }

        async fn mark_runtime_attempt_gone(
            &self,
            _session_id: &RuntimeSessionId,
            _attempt_id: &RuntimeAttemptId,
            _native_session_ref: Option<&str>,
            _observed_at: i64,
        ) -> Result<DurableRuntimeSession, NativeRuntimeError> {
            unreachable!("mark_runtime_attempt_gone is not used by the reconciler test")
        }

        async fn finish_runtime_attempt(
            &self,
            report: &RuntimeShutdownReport,
        ) -> Result<DurableRuntimeSession, NativeRuntimeError> {
            self.reports
                .lock()
                .expect("report lock")
                .push(report.clone());
            Ok(DurableRuntimeSession {
                registration: RuntimeSessionRegistration {
                    session_id: report.session_id.clone(),
                    execution_task_id: TaskRunId::new(),
                    agent_profile_id: AgentProfileId::new(),
                    snapshot_ref: ProjectExecutionSnapshotRef::new(
                        AuthorityKey::new("test-authority").expect("authority key"),
                        "snapshot",
                        "version-1",
                    )
                    .expect("snapshot ref"),
                    snapshot_content_sha256: "b".repeat(64),
                    provider: RuntimeProvider::Codex,
                    command_sha256: "c".repeat(64),
                    sandbox: RuntimeSandboxRef {
                        sandbox_id: "sb_test".to_string(),
                        profile_sha256: "d".repeat(64),
                        expires_at: 100,
                    },
                    max_capture_bytes: 1024,
                    max_transcript_bytes: 4096,
                    idle_timeout_ms: 1000,
                    created_at: 10,
                },
                status: if report.terminal_reason == RuntimeTerminalReason::Completed {
                    RuntimeSessionStatus::Completed
                } else {
                    RuntimeSessionStatus::Failed
                },
                current_attempt_id: Some(report.attempt_id.clone()),
                native_session_ref: None,
                transcript: Some(report.transcript.clone()),
                terminal_reason: Some(report.terminal_reason),
                record_version: 3,
                updated_at: report.finished_at,
            })
        }

        async fn record_runtime_recovery(
            &self,
            _record: &RuntimeRecoveryRecord,
        ) -> Result<(), NativeRuntimeError> {
            unreachable!("record_runtime_recovery is not used by the reconciler test")
        }

        async fn load_runtime_session(
            &self,
            _session_id: &RuntimeSessionId,
        ) -> Result<Option<DurableRuntimeSession>, NativeRuntimeError> {
            unreachable!("load_runtime_session is not used by the reconciler test")
        }

        async fn load_runtime_attempt(
            &self,
            _attempt_id: &RuntimeAttemptId,
        ) -> Result<Option<DurableRuntimeAttempt>, NativeRuntimeError> {
            unreachable!("terminal snapshots without provider refs do not load attempts")
        }

        async fn recoverable_runtime_sessions(
            &self,
            _limit: u32,
        ) -> Result<Vec<DurableRuntimeSession>, NativeRuntimeError> {
            unreachable!("recoverable_runtime_sessions is not used by the reconciler test")
        }
    }

    fn snapshot(
        session_id: &RuntimeSessionId,
        attempt_id: &RuntimeAttemptId,
        status: RuntimeAttemptStatus,
        event_index: u64,
        terminal: bool,
    ) -> RuntimeSnapshot {
        RuntimeSnapshot {
            session_id: session_id.clone(),
            attempt_id: attempt_id.clone(),
            provider: RuntimeProvider::Codex,
            status,
            pid: Some(42),
            dimensions: RuntimeDimensions {
                rows: 24,
                columns: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
            event_index,
            output_tail: String::new(),
            output_truncated: false,
            native_session_ref: None,
            transcript: terminal.then(|| RuntimeTranscriptRef {
                id: RuntimeTranscriptId::new(),
                content_sha256: "a".repeat(64),
                storage_ref: "sha256/aa/archive".to_string(),
                size_bytes: 17,
                truncated: false,
                archived_at: 12,
            }),
            exit_code: terminal.then_some(0),
            started_at: 10,
            last_output_at: 11,
            finished_at: terminal.then_some(12),
        }
    }

    #[tokio::test]
    async fn background_reconciler_waits_for_terminal_snapshot_and_persists_it() {
        let session_id = RuntimeSessionId::new();
        let attempt_id = RuntimeAttemptId::new();
        let backend = Arc::new(ReconcileBackend {
            snapshots: Mutex::new(VecDeque::from([
                snapshot(
                    &session_id,
                    &attempt_id,
                    RuntimeAttemptStatus::Running,
                    4,
                    false,
                ),
                snapshot(
                    &session_id,
                    &attempt_id,
                    RuntimeAttemptStatus::Exited,
                    7,
                    true,
                ),
            ])),
            cursors: Mutex::new(Vec::new()),
        });
        let ledger = Arc::new(RecordingLedger::default());

        reconcile_runtime_attempt(backend.clone(), ledger.clone(), attempt_id.clone()).await;

        assert_eq!(*backend.cursors.lock().expect("cursor lock"), [0, 4]);
        let reports = ledger.reports.lock().expect("report lock");
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].attempt_id, attempt_id);
        assert_eq!(reports[0].terminal_reason, RuntimeTerminalReason::Completed);
        assert_eq!(reports[0].exit_code, Some(0));
    }
}
