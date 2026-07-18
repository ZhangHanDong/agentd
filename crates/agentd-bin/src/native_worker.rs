//! Composition-root wiring for the native worker.
//!
//! `agentd-tmux::native` owns disposable PTY resources. This module binds that
//! resource lifecycle to the durable runtime session/attempt repositories.

use std::sync::Arc;
use std::time::Duration;

use agentd_core::types::{RuntimeAttemptId, RuntimeSessionId, WorkerIncarnationId};
use agentd_store::runtime_session_repo::{self, RuntimeAttemptCreate};
use agentd_store::{SqliteStore, StoreError};
use agentd_tmux::native::{
    NativeProcessConfig, NativeProcessEvent, NativeRuntime, NativeRuntimeError,
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
