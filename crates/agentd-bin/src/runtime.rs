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
        if let Some(reference) = snapshot.native_session_ref.as_deref() {
            let attempt = self
                .ledger
                .load_runtime_attempt(&snapshot.attempt_id)
                .await?
                .ok_or_else(|| {
                    NativeRuntimeError::NotFound(format!("runtime attempt {}", snapshot.attempt_id))
                })?;
            if attempt.native_session_ref.is_none() {
                self.ledger
                    .update_runtime_native_ref(
                        &snapshot.session_id,
                        &snapshot.attempt_id,
                        reference,
                        self.trusted_now()?,
                    )
                    .await?;
            }
        }
        if snapshot.status.is_terminal() {
            if let (Some(transcript), Some(finished_at)) =
                (snapshot.transcript.clone(), snapshot.finished_at)
            {
                self.ledger
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
        }
        Ok(())
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
