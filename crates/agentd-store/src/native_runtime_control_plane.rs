//! `SQLite` adapter for the core native runtime control-plane port.

use agentd_core::ports::{
    NativeRuntimeAttemptStart, NativeRuntimeAttemptState, NativeRuntimeControlError,
    NativeRuntimeControlPort,
};
use agentd_core::types::RuntimeAttemptStatus;
use async_trait::async_trait;
use sqlx::SqlitePool;

use crate::runtime_session_repo::{self, RuntimeAttemptCreate};

#[derive(Debug, Clone)]
pub struct SqliteNativeRuntimeControlPlane {
    pool: SqlitePool,
}

impl SqliteNativeRuntimeControlPlane {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn map_error(error: crate::StoreError) -> NativeRuntimeControlError {
    match error {
        crate::StoreError::NotFound => NativeRuntimeControlError::NotFound("runtime record".into()),
        crate::StoreError::Conflict(message) => NativeRuntimeControlError::Conflict(message),
        other => NativeRuntimeControlError::Unavailable(other.to_string()),
    }
}

fn state(record: runtime_session_repo::RuntimeAttemptRecord) -> NativeRuntimeAttemptState {
    NativeRuntimeAttemptState {
        attempt_id: record.id,
        session_id: record.runtime_session_id,
        status: record.status,
        native_session_ref: record.native_session_ref,
        observed_at: record.started_at,
    }
}

#[async_trait]
impl NativeRuntimeControlPort for SqliteNativeRuntimeControlPlane {
    async fn validate_session_task(
        &self,
        session_id: &agentd_core::types::RuntimeSessionId,
        task_id: &agentd_core::types::TaskRunId,
    ) -> Result<(), NativeRuntimeControlError> {
        runtime_session_repo::get_session(&self.pool, session_id)
            .await
            .map_err(map_error)?
            .filter(|session| session.execution_task_id == *task_id)
            .map(|_| ())
            .ok_or_else(|| NativeRuntimeControlError::Conflict("session/task mismatch".into()))
    }

    async fn session_view(
        &self,
        session_id: &agentd_core::types::RuntimeSessionId,
    ) -> Result<Option<agentd_core::ports::NativeRuntimeSessionView>, NativeRuntimeControlError>
    {
        let Some(session) = runtime_session_repo::get_session(&self.pool, session_id)
            .await
            .map_err(map_error)?
        else {
            return Ok(None);
        };
        let latest_native_session_ref =
            runtime_session_repo::latest_attempt(&self.pool, session_id)
                .await
                .map_err(map_error)?
                .and_then(|attempt| attempt.native_session_ref);
        Ok(Some(agentd_core::ports::NativeRuntimeSessionView {
            session_id: session.id,
            task_id: session.execution_task_id,
            status: session.status,
            latest_native_session_ref,
        }))
    }

    async fn start_attempt(
        &self,
        request: &NativeRuntimeAttemptStart,
    ) -> Result<NativeRuntimeAttemptState, NativeRuntimeControlError> {
        self.validate_session_task(&request.session_id, &request.task_id)
            .await?;
        let record = runtime_session_repo::start_attempt(
            &self.pool,
            &request.session_id,
            RuntimeAttemptCreate {
                id: request.attempt_id.clone(),
                worker_incarnation_id: request.worker_incarnation_id.clone(),
                backend_target: Some("native".into()),
                session_name: None,
                pane_id: None,
                pid: None,
                native_session_ref: None,
                workdir: None,
            },
        )
        .await
        .map_err(map_error)?;
        Ok(state(record))
    }

    async fn update_attempt(
        &self,
        state: &NativeRuntimeAttemptState,
    ) -> Result<(), NativeRuntimeControlError> {
        if let Some(native_ref) = &state.native_session_ref {
            runtime_session_repo::set_attempt_native_session_ref(
                &self.pool,
                &state.session_id,
                &state.attempt_id,
                native_ref,
            )
            .await
            .map_err(map_error)?;
        }
        if state.status == RuntimeAttemptStatus::Running {
            runtime_session_repo::mark_attempt_running(
                &self.pool,
                &state.session_id,
                &state.attempt_id,
                None,
            )
            .await
            .map_err(map_error)?;
        }
        Ok(())
    }

    async fn mark_attempt_terminal(
        &self,
        state: &NativeRuntimeAttemptState,
    ) -> Result<(), NativeRuntimeControlError> {
        match state.status {
            RuntimeAttemptStatus::Exited => runtime_session_repo::mark_attempt_exited(
                &self.pool,
                &state.session_id,
                &state.attempt_id,
                None,
                Some("native_exit"),
            )
            .await
            .map(|_| ())
            .map_err(map_error)?,
            RuntimeAttemptStatus::Gone => runtime_session_repo::mark_attempt_gone(
                &self.pool,
                &state.session_id,
                &state.attempt_id,
            )
            .await
            .map(|_| ())
            .map_err(map_error)?,
            _ => {
                return Err(NativeRuntimeControlError::Invalid(
                    "terminal state must be exited or gone".into(),
                ));
            }
        }
        Ok(())
    }
}
