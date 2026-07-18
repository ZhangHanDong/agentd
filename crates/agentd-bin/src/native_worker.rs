//! Composition-root wiring for the native worker.
//!
//! `agentd-tmux::native` owns disposable PTY resources. This module binds that
//! resource lifecycle to the durable runtime session/attempt repositories.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agentd_core::ports::{
    ArtifactIndexPort, ExecutionArtifactKind, ExecutionArtifactPublish, ExecutionEvidenceError,
    ExecutionEvidenceLinks, WorkerArtifactAcknowledgement, WorkerArtifactReport,
};
use agentd_core::types::{
    ExecutionArtifactId, RuntimeAttemptId, RuntimeSessionId, RuntimeSessionStatus, TaskLeaseClaim,
    WorkerIncarnationId,
};
use agentd_store::runtime_session_repo::{self, RuntimeAttemptCreate};
use agentd_store::{SqliteStore, StoreError};
use agentd_tmux::native::{
    NativeProcessConfig, NativeProcessEvent, NativeRuntime, NativeRuntimeError, NativeSpoolRecord,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NativeWorkerError {
    #[error("durable runtime state error: {0}")]
    Store(#[from] StoreError),
    #[error("native runtime error: {0}")]
    Native(#[from] NativeRuntimeError),
    #[error("native worker blocking task failed: {0}")]
    Join(String),
    #[error("artifact acknowledgement failed: {0}")]
    Evidence(String),
}

impl From<ExecutionEvidenceError> for NativeWorkerError {
    fn from(error: ExecutionEvidenceError) -> Self {
        Self::Evidence(error.to_string())
    }
}

#[derive(Debug, Clone)]
pub struct AgentdWorker {
    store: SqliteStore,
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

#[derive(Debug)]
pub struct AgentdWorkerHandle {
    store: SqliteStore,
    runtime: Arc<NativeRuntime>,
    session_id: RuntimeSessionId,
    attempt_id: RuntimeAttemptId,
}

impl AgentdWorker {
    #[must_use]
    pub fn new(store: SqliteStore) -> Self {
        Self { store }
    }

    /// Start a native process only after creating its durable attempt record.
    /// A spawn failure is reconciled as `gone` before the error is returned.
    pub async fn start(
        &self,
        session_id: RuntimeSessionId,
        worker_incarnation_id: WorkerIncarnationId,
        config: NativeProcessConfig,
    ) -> Result<AgentdWorkerHandle, NativeWorkerError> {
        let attempt_id = RuntimeAttemptId::new();
        runtime_session_repo::start_attempt(
            self.store.pool(),
            &session_id,
            RuntimeAttemptCreate {
                id: attempt_id.clone(),
                worker_incarnation_id,
                backend_target: Some("native-pty".to_string()),
                session_name: None,
                pane_id: None,
                pid: None,
                native_session_ref: config.native_session_ref.clone(),
                workdir: config.cwd.clone(),
            },
        )
        .await?;

        let spawn_config = config;
        let runtime =
            match tokio::task::spawn_blocking(move || NativeRuntime::spawn(spawn_config)).await {
                Ok(Ok(runtime)) => runtime,
                Ok(Err(error)) => {
                    let _ = runtime_session_repo::mark_attempt_gone(
                        self.store.pool(),
                        &session_id,
                        &attempt_id,
                    )
                    .await;
                    return Err(error.into());
                }
                Err(error) => {
                    let _ = runtime_session_repo::mark_attempt_gone(
                        self.store.pool(),
                        &session_id,
                        &attempt_id,
                    )
                    .await;
                    return Err(NativeWorkerError::Join(error.to_string()));
                }
            };
        let runtime = Arc::new(runtime);
        if let Err(error) = runtime_session_repo::mark_attempt_running(
            self.store.pool(),
            &session_id,
            &attempt_id,
            None,
        )
        .await
        {
            let _ = runtime_session_repo::mark_attempt_gone(
                self.store.pool(),
                &session_id,
                &attempt_id,
            )
            .await;
            return Err(error.into());
        }
        Ok(AgentdWorkerHandle {
            store: self.store.clone(),
            runtime,
            session_id,
            attempt_id,
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
            config.native_session_ref =
                runtime_session_repo::latest_attempt(self.store.pool(), &session_id)
                    .await?
                    .and_then(|attempt| attempt.native_session_ref);
        }
        if config.program.rsplit('/').next() == Some("codex") {
            if let Some(thread_ref) = config.native_session_ref.clone() {
                config = codex_resume_config(config, thread_ref);
            }
        }
        self.start(session_id, worker_incarnation_id, config).await
    }

    /// Recover a session only when durable control state explicitly requests it.
    pub async fn recover_if_pending(
        &self,
        session_id: RuntimeSessionId,
        worker_incarnation_id: WorkerIncarnationId,
        config: NativeProcessConfig,
    ) -> Result<Option<AgentdWorkerHandle>, NativeWorkerError> {
        let Some(session) =
            runtime_session_repo::get_session(self.store.pool(), &session_id).await?
        else {
            return Ok(None);
        };
        if session.status != RuntimeSessionStatus::ResumePending {
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
    pub fn native_session_ref(&self) -> Option<String> {
        self.runtime.native_session_ref()
    }

    /// Request process termination; callers should still call `wait` to
    /// reconcile the durable attempt outcome.
    pub fn terminate(&self) -> Result<(), NativeWorkerError> {
        self.runtime.terminate().map_err(NativeWorkerError::Native)
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
                runtime_session_repo::mark_attempt_exited(
                    self.store.pool(),
                    &self.session_id,
                    &self.attempt_id,
                    *code,
                    Some(if *code == Some(0) {
                        "native_exit"
                    } else {
                        "native_failure"
                    }),
                )
                .await?;
            }
            NativeProcessEvent::Gone { .. } => {
                runtime_session_repo::mark_attempt_gone(
                    self.store.pool(),
                    &self.session_id,
                    &self.attempt_id,
                )
                .await?;
            }
        }
        Ok(event)
    }
}
