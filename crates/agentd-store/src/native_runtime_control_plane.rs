//! Durable AD-E5 native runtime session, attempt, event, and recovery control plane.

use agentd_core::ports::{
    DurableRuntimeAttempt, DurableRuntimeSession, NativeRuntimeError, RuntimeEvent,
    RuntimeEventKind, RuntimeEventPayload, RuntimeEventPort, RuntimeHandle, RuntimeLedgerPort,
    RuntimeProvider, RuntimeRecoveryDisposition, RuntimeRecoveryRecord, RuntimeSessionRegistration,
    RuntimeShutdownReport, RuntimeTerminalReason, RuntimeTranscriptRef,
};
use agentd_core::types::{
    AgentProfileId, AuthorityKey, ProjectExecutionSnapshotRef, RuntimeAttemptId,
    RuntimeAttemptStatus, RuntimeEventId, RuntimeSessionId, RuntimeSessionStatus,
    RuntimeTranscriptId, TaskRunId, WorkerIncarnationId,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::util::SqliteImmediateTransaction;

/// `SQLite` implementation of the native runtime durable contract.
#[derive(Debug, Clone)]
pub struct SqliteNativeRuntimeControlPlane {
    pool: SqlitePool,
}

impl SqliteNativeRuntimeControlPlane {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

#[async_trait::async_trait]
impl RuntimeEventPort for SqliteNativeRuntimeControlPlane {
    async fn append_runtime_event(
        &self,
        event: &RuntimeEvent,
    ) -> Result<RuntimeEvent, NativeRuntimeError> {
        validate_event(event)?;
        if let Some(existing) = load_event_by_id(&self.pool, &event.id).await? {
            return if same_event_identity(&existing, event) {
                Ok(existing)
            } else {
                Err(NativeRuntimeError::Conflict(
                    "runtime event id replay differs".to_string(),
                ))
            };
        }
        let payload_json = serde_json::to_string(&event.payload).map_err(invalid_json)?;
        let mut transaction = SqliteImmediateTransaction::begin(&self.pool)
            .await
            .map_err(storage_error)?;
        require_attempt_session(&mut transaction, &event.attempt_id, &event.session_id).await?;
        let event_index: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(event_index), 0) + 1 FROM native_runtime_events \
             WHERE runtime_session_id = ?",
        )
        .bind(event.session_id.as_str())
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let persisted = RuntimeEvent {
            event_index: u64::try_from(event_index).map_err(|_| {
                NativeRuntimeError::Unavailable("runtime event cursor is out of range".to_string())
            })?,
            ..event.clone()
        };
        sqlx::query(
            "INSERT INTO native_runtime_events (id, runtime_session_id, runtime_attempt_id, \
             event_index, kind, payload_sha256, payload_json, occurred_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(persisted.id.as_str())
        .bind(persisted.session_id.as_str())
        .bind(persisted.attempt_id.as_str())
        .bind(event_index)
        .bind(persisted.kind.as_str())
        .bind(&persisted.payload_sha256)
        .bind(payload_json)
        .bind(persisted.occurred_at)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if persisted.kind == RuntimeEventKind::InputAccepted {
            if let RuntimeEventPayload::Input {
                idempotency_key,
                input_sha256,
                byte_count,
            } = &persisted.payload
            {
                sqlx::query(
                    "INSERT INTO native_runtime_input_actions (runtime_attempt_id, \
                     idempotency_key, input_sha256, byte_count, accepted_at, runtime_event_id) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                )
                .bind(persisted.attempt_id.as_str())
                .bind(idempotency_key)
                .bind(input_sha256)
                .bind(i64::try_from(*byte_count).map_err(|_| {
                    NativeRuntimeError::Invalid("runtime input byte count is too large".to_string())
                })?)
                .bind(persisted.occurred_at)
                .bind(persisted.id.as_str())
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
            }
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(persisted)
    }

    async fn runtime_events_after(
        &self,
        session_id: &RuntimeSessionId,
        after_event_index: u64,
        limit: u32,
    ) -> Result<Vec<RuntimeEvent>, NativeRuntimeError> {
        if limit == 0 || limit > 1_000 {
            return Err(NativeRuntimeError::Invalid(
                "runtime event limit must be between 1 and 1000".to_string(),
            ));
        }
        let rows = sqlx::query(
            "SELECT id, runtime_session_id, runtime_attempt_id, event_index, kind, \
             payload_sha256, payload_json, occurred_at FROM native_runtime_events \
             WHERE runtime_session_id = ? AND event_index > ? ORDER BY event_index LIMIT ?",
        )
        .bind(session_id.as_str())
        .bind(i64::try_from(after_event_index).map_err(|_| {
            NativeRuntimeError::Invalid("runtime event cursor is too large".to_string())
        })?)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        rows.iter().map(event_from_row).collect()
    }
}

#[allow(clippy::too_many_lines)]
#[async_trait::async_trait]
impl RuntimeLedgerPort for SqliteNativeRuntimeControlPlane {
    async fn register_runtime_session(
        &self,
        registration: &RuntimeSessionRegistration,
    ) -> Result<DurableRuntimeSession, NativeRuntimeError> {
        validate_registration(registration)?;
        if let Some(existing) = load_session(&self.pool, &registration.session_id).await? {
            return if existing.registration == *registration {
                Ok(existing)
            } else {
                Err(NativeRuntimeError::Conflict(
                    "runtime session id already has different registration".to_string(),
                ))
            };
        }
        let snapshot = registration.snapshot_ref.as_resource_ref();
        sqlx::query(
            "INSERT INTO runtime_sessions (id, execution_task_id, agent_profile_id, \
             snapshot_authority_key, snapshot_resource_kind, snapshot_resource_id, \
             snapshot_resource_version, snapshot_content_sha256, status, record_version, \
             terminal_reason, created_at, updated_at, provider, command_sha256, sandbox_id, \
             sandbox_profile_sha256, sandbox_expires_at, max_capture_bytes, \
             max_transcript_bytes, idle_timeout_ms, current_attempt_id, native_session_ref, \
             transcript_id) VALUES (?, ?, ?, ?, 'execution_snapshot', ?, ?, ?, 'requested', \
             1, NULL, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, NULL)",
        )
        .bind(registration.session_id.as_str())
        .bind(registration.execution_task_id.as_str())
        .bind(registration.agent_profile_id.as_str())
        .bind(snapshot.authority_key().as_str())
        .bind(snapshot.resource_id())
        .bind(snapshot.resource_version())
        .bind(&registration.snapshot_content_sha256)
        .bind(registration.created_at)
        .bind(registration.created_at)
        .bind(registration.provider.as_str())
        .bind(&registration.command_sha256)
        .bind(&registration.sandbox.sandbox_id)
        .bind(&registration.sandbox.profile_sha256)
        .bind(registration.sandbox.expires_at)
        .bind(to_i64(registration.max_capture_bytes, "capture bound")?)
        .bind(to_i64(
            registration.max_transcript_bytes,
            "transcript bound",
        )?)
        .bind(to_i64(registration.idle_timeout_ms, "idle timeout")?)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        load_session(&self.pool, &registration.session_id)
            .await?
            .ok_or_else(|| {
                NativeRuntimeError::Unavailable("runtime session disappeared".to_string())
            })
    }

    async fn begin_runtime_attempt(
        &self,
        session_id: &RuntimeSessionId,
        attempt_id: &RuntimeAttemptId,
        worker_incarnation_id: &WorkerIncarnationId,
        host_instance_id: &str,
        started_at: i64,
    ) -> Result<DurableRuntimeAttempt, NativeRuntimeError> {
        validate_attempt_identity(
            attempt_id,
            worker_incarnation_id,
            host_instance_id,
            started_at,
        )?;
        if let Some(existing) = load_attempt(&self.pool, attempt_id).await? {
            return if existing.session_id == *session_id
                && existing.worker_incarnation_id == *worker_incarnation_id
                && existing.host_instance_id == host_instance_id
                && existing.started_at == started_at
            {
                Ok(existing)
            } else {
                Err(NativeRuntimeError::Conflict(
                    "runtime attempt id replay differs".to_string(),
                ))
            };
        }
        let mut transaction = SqliteImmediateTransaction::begin(&self.pool)
            .await
            .map_err(storage_error)?;
        let session = sqlx::query(
            "SELECT status, record_version, current_attempt_id FROM runtime_sessions \
             WHERE id = ? AND provider IS NOT NULL",
        )
        .bind(session_id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| NativeRuntimeError::NotFound(format!("runtime session {session_id}")))?;
        let status = parse_session_status(&session.get::<String, _>("status"))?;
        if !matches!(
            status,
            RuntimeSessionStatus::Requested | RuntimeSessionStatus::ResumePending
        ) {
            return Err(NativeRuntimeError::Conflict(format!(
                "runtime session cannot start an attempt from {status}"
            )));
        }
        let worker_current: Option<i64> =
            sqlx::query_scalar("SELECT is_current FROM worker_incarnations WHERE id = ?")
                .bind(worker_incarnation_id.as_str())
                .fetch_optional(&mut *transaction)
                .await
                .map_err(storage_error)?;
        if worker_current != Some(1) {
            return Err(NativeRuntimeError::Denied(
                "runtime attempt requires a current worker incarnation".to_string(),
            ));
        }
        if let Some(previous) = session.get::<Option<String>, _>("current_attempt_id") {
            if status != RuntimeSessionStatus::ResumePending {
                return Err(NativeRuntimeError::Conflict(
                    "runtime session already has a current attempt".to_string(),
                ));
            }
            sqlx::query(
                "UPDATE runtime_attempts SET status = 'gone', is_current = 0, \
                 finished_at = COALESCE(finished_at, ?), superseded_at = ? \
                 WHERE id = ? AND is_current = 1",
            )
            .bind(started_at)
            .bind(started_at)
            .bind(previous)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
        }
        sqlx::query(
            "INSERT INTO runtime_attempts (id, runtime_session_id, worker_incarnation_id, \
             status, backend_target, session_name, pane_id, pid, native_session_ref, workdir, \
             is_current, started_at, finished_at, superseded_at, host_instance_id, exit_code, \
             transcript_id) VALUES (?, ?, ?, 'starting', 'native_pty', NULL, NULL, NULL, NULL, \
             NULL, 1, ?, NULL, NULL, ?, NULL, NULL)",
        )
        .bind(attempt_id.as_str())
        .bind(session_id.as_str())
        .bind(worker_incarnation_id.as_str())
        .bind(started_at)
        .bind(host_instance_id)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let updated = sqlx::query(
            "UPDATE runtime_sessions SET status = 'starting', current_attempt_id = ?, \
             record_version = record_version + 1, terminal_reason = NULL, updated_at = ? \
             WHERE id = ? AND status = ? AND record_version = ?",
        )
        .bind(attempt_id.as_str())
        .bind(started_at)
        .bind(session_id.as_str())
        .bind(status.as_str())
        .bind(session.get::<i64, _>("record_version"))
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if updated.rows_affected() != 1 {
            return Err(NativeRuntimeError::Conflict(
                "runtime session changed while starting an attempt".to_string(),
            ));
        }
        transaction.commit().await.map_err(storage_error)?;
        load_attempt(&self.pool, attempt_id).await?.ok_or_else(|| {
            NativeRuntimeError::Unavailable("runtime attempt disappeared".to_string())
        })
    }

    async fn mark_runtime_attempt_running(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<DurableRuntimeAttempt, NativeRuntimeError> {
        if handle.pid == 0 || handle.started_at < 0 {
            return Err(NativeRuntimeError::Invalid(
                "runtime handle is invalid".to_string(),
            ));
        }
        let current = load_attempt(&self.pool, &handle.attempt_id)
            .await?
            .ok_or_else(|| {
                NativeRuntimeError::NotFound(format!("runtime attempt {}", handle.attempt_id))
            })?;
        if current.session_id != handle.session_id || !current.is_current {
            return Err(NativeRuntimeError::Conflict(
                "runtime handle does not name the current attempt".to_string(),
            ));
        }
        if current.status == RuntimeAttemptStatus::Running {
            return if current.pid == Some(handle.pid)
                && current.native_session_ref == handle.native_session_ref
            {
                Ok(current)
            } else {
                Err(NativeRuntimeError::Conflict(
                    "running runtime handle replay differs".to_string(),
                ))
            };
        }
        if current.status != RuntimeAttemptStatus::Starting {
            return Err(NativeRuntimeError::Conflict(
                "runtime attempt cannot become running".to_string(),
            ));
        }
        let mut transaction = SqliteImmediateTransaction::begin(&self.pool)
            .await
            .map_err(storage_error)?;
        let updated_attempt = sqlx::query(
            "UPDATE runtime_attempts SET status = 'running', pid = ?, native_session_ref = ? \
             WHERE id = ? AND runtime_session_id = ? AND status = 'starting' AND is_current = 1",
        )
        .bind(i64::from(handle.pid))
        .bind(&handle.native_session_ref)
        .bind(handle.attempt_id.as_str())
        .bind(handle.session_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let updated_session = sqlx::query(
            "UPDATE runtime_sessions SET status = 'running', native_session_ref = ?, \
             record_version = record_version + 1, updated_at = ? \
             WHERE id = ? AND status = 'starting' AND current_attempt_id = ? AND provider = ?",
        )
        .bind(&handle.native_session_ref)
        .bind(handle.started_at)
        .bind(handle.session_id.as_str())
        .bind(handle.attempt_id.as_str())
        .bind(handle.provider.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if updated_attempt.rows_affected() != 1 || updated_session.rows_affected() != 1 {
            return Err(NativeRuntimeError::Conflict(
                "runtime attempt changed while becoming running".to_string(),
            ));
        }
        transaction.commit().await.map_err(storage_error)?;
        load_attempt(&self.pool, &handle.attempt_id)
            .await?
            .ok_or_else(|| {
                NativeRuntimeError::Unavailable("runtime attempt disappeared".to_string())
            })
    }

    async fn update_runtime_native_ref(
        &self,
        session_id: &RuntimeSessionId,
        attempt_id: &RuntimeAttemptId,
        native_session_ref: &str,
        observed_at: i64,
    ) -> Result<DurableRuntimeAttempt, NativeRuntimeError> {
        validate_native_ref(native_session_ref)?;
        let current = load_attempt(&self.pool, attempt_id)
            .await?
            .ok_or_else(|| NativeRuntimeError::NotFound(format!("runtime attempt {attempt_id}")))?;
        if current.session_id != *session_id || !current.is_current || current.status.is_terminal()
        {
            return Err(NativeRuntimeError::Conflict(
                "native session reference requires the current live attempt".to_string(),
            ));
        }
        if let Some(existing) = &current.native_session_ref {
            return if existing == native_session_ref {
                Ok(current)
            } else {
                Err(NativeRuntimeError::Conflict(
                    "runtime attempt already has a different native session reference".to_string(),
                ))
            };
        }
        let mut transaction = SqliteImmediateTransaction::begin(&self.pool)
            .await
            .map_err(storage_error)?;
        sqlx::query(
            "UPDATE runtime_attempts SET native_session_ref = ? \
             WHERE id = ? AND runtime_session_id = ? AND is_current = 1 \
             AND native_session_ref IS NULL",
        )
        .bind(native_session_ref)
        .bind(attempt_id.as_str())
        .bind(session_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "UPDATE runtime_sessions SET native_session_ref = ?, record_version = record_version + 1, \
             updated_at = ? WHERE id = ? AND current_attempt_id = ?",
        )
        .bind(native_session_ref)
        .bind(observed_at)
        .bind(session_id.as_str())
        .bind(attempt_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        transaction.commit().await.map_err(storage_error)?;
        load_attempt(&self.pool, attempt_id).await?.ok_or_else(|| {
            NativeRuntimeError::Unavailable("runtime attempt disappeared".to_string())
        })
    }

    async fn mark_runtime_attempt_gone(
        &self,
        session_id: &RuntimeSessionId,
        attempt_id: &RuntimeAttemptId,
        native_session_ref: Option<&str>,
        observed_at: i64,
    ) -> Result<DurableRuntimeSession, NativeRuntimeError> {
        if observed_at < 0 {
            return Err(NativeRuntimeError::Invalid(
                "runtime gone timestamp is invalid".to_string(),
            ));
        }
        if let Some(reference) = native_session_ref {
            validate_native_ref(reference)?;
        }
        let session = load_session(&self.pool, session_id)
            .await?
            .ok_or_else(|| NativeRuntimeError::NotFound(format!("runtime session {session_id}")))?;
        if session.status.is_terminal() {
            return if session.current_attempt_id.as_ref() == Some(attempt_id)
                && session.status == RuntimeSessionStatus::Lost
                && native_session_ref.is_none()
            {
                Ok(session)
            } else {
                Err(NativeRuntimeError::Conflict(
                    "terminal runtime session cannot be marked gone".to_string(),
                ))
            };
        }
        if session.current_attempt_id.as_ref() != Some(attempt_id) {
            return Err(NativeRuntimeError::Conflict(
                "runtime gone report does not name the current attempt".to_string(),
            ));
        }
        let target = if native_session_ref.is_some() {
            RuntimeSessionStatus::ResumePending
        } else {
            RuntimeSessionStatus::Lost
        };
        let mut transaction = SqliteImmediateTransaction::begin(&self.pool)
            .await
            .map_err(storage_error)?;
        let updated_attempt = sqlx::query(
            "UPDATE runtime_attempts SET status = 'gone', is_current = 0, \
             native_session_ref = COALESCE(native_session_ref, ?), finished_at = ?, \
             superseded_at = ? WHERE id = ? AND runtime_session_id = ? AND is_current = 1 \
             AND status IN ('starting', 'running')",
        )
        .bind(native_session_ref)
        .bind(observed_at)
        .bind(observed_at)
        .bind(attempt_id.as_str())
        .bind(session_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let updated_session = sqlx::query(
            "UPDATE runtime_sessions SET status = ?, native_session_ref = COALESCE(native_session_ref, ?), \
             terminal_reason = ?, record_version = record_version + 1, updated_at = ? \
             WHERE id = ? AND current_attempt_id = ? AND status IN ('starting', 'running', 'resume_pending')",
        )
        .bind(target.as_str())
        .bind(native_session_ref)
        .bind((target == RuntimeSessionStatus::Lost).then_some("runtime_gone"))
        .bind(observed_at)
        .bind(session_id.as_str())
        .bind(attempt_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if updated_attempt.rows_affected() != 1 || updated_session.rows_affected() != 1 {
            return Err(NativeRuntimeError::Conflict(
                "runtime attempt changed while marking it gone".to_string(),
            ));
        }
        transaction.commit().await.map_err(storage_error)?;
        load_session(&self.pool, session_id).await?.ok_or_else(|| {
            NativeRuntimeError::Unavailable("runtime session disappeared".to_string())
        })
    }

    async fn finish_runtime_attempt(
        &self,
        report: &RuntimeShutdownReport,
    ) -> Result<DurableRuntimeSession, NativeRuntimeError> {
        validate_report(report)?;
        let session = load_session(&self.pool, &report.session_id)
            .await?
            .ok_or_else(|| {
                NativeRuntimeError::NotFound(format!("runtime session {}", report.session_id))
            })?;
        if session.status.is_terminal() {
            return if session.current_attempt_id.as_ref() == Some(&report.attempt_id)
                && session.transcript.as_ref() == Some(&report.transcript)
                && session.terminal_reason == Some(report.terminal_reason)
            {
                Ok(session)
            } else {
                Err(NativeRuntimeError::Conflict(
                    "terminal runtime session replay differs".to_string(),
                ))
            };
        }
        if session.current_attempt_id.as_ref() != Some(&report.attempt_id) {
            return Err(NativeRuntimeError::Conflict(
                "runtime report does not name the current attempt".to_string(),
            ));
        }
        let mut transaction = SqliteImmediateTransaction::begin(&self.pool)
            .await
            .map_err(storage_error)?;
        insert_transcript(&mut transaction, report).await?;
        let updated_attempt = sqlx::query(
            "UPDATE runtime_attempts SET status = 'exited', is_current = 0, finished_at = ?, \
             superseded_at = ?, exit_code = ?, transcript_id = ? \
             WHERE id = ? AND runtime_session_id = ? AND is_current = 1 \
             AND status IN ('starting', 'running')",
        )
        .bind(report.finished_at)
        .bind(report.finished_at)
        .bind(report.exit_code)
        .bind(report.transcript.id.as_str())
        .bind(report.attempt_id.as_str())
        .bind(report.session_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let terminal_status = terminal_status(report.terminal_reason);
        let updated_session = sqlx::query(
            "UPDATE runtime_sessions SET status = ?, transcript_id = ?, terminal_reason = ?, \
             record_version = record_version + 1, updated_at = ? \
             WHERE id = ? AND current_attempt_id = ? AND status IN ('starting', 'running', 'resume_pending')",
        )
        .bind(terminal_status.as_str())
        .bind(report.transcript.id.as_str())
        .bind(terminal_reason_str(report.terminal_reason))
        .bind(report.finished_at)
        .bind(report.session_id.as_str())
        .bind(report.attempt_id.as_str())
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if updated_attempt.rows_affected() != 1 || updated_session.rows_affected() != 1 {
            return Err(NativeRuntimeError::Conflict(
                "runtime attempt changed while finishing".to_string(),
            ));
        }
        let native_binding = sqlx::query(
            "SELECT execution_task_id, synthetic_task FROM native_agent_runtime_bindings \
             WHERE runtime_session_id = ? AND runtime_attempt_id = ? AND status = 'active'",
        )
        .bind(report.session_id.as_str())
        .bind(report.attempt_id.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if let Some(binding) = native_binding {
            let binding_update = sqlx::query(
                "UPDATE native_agent_runtime_bindings SET status = 'finished', finished_at = ? \
                 WHERE runtime_session_id = ? AND runtime_attempt_id = ? AND status = 'active'",
            )
            .bind(report.finished_at)
            .bind(report.session_id.as_str())
            .bind(report.attempt_id.as_str())
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            if binding_update.rows_affected() != 1 {
                return Err(NativeRuntimeError::Conflict(
                    "native agent binding changed while finishing runtime".to_string(),
                ));
            }
            if binding.get::<bool, _>("synthetic_task") {
                let execution_task_id = binding.get::<String, _>("execution_task_id");
                let synthetic_run_id: String =
                    sqlx::query_scalar("SELECT run_id FROM task_runs WHERE id = ?")
                        .bind(&execution_task_id)
                        .fetch_one(&mut *transaction)
                        .await
                        .map_err(storage_error)?;
                let task_update = sqlx::query(
                    "UPDATE task_runs SET finished_at = ?, status = 'finished' WHERE id = ?",
                )
                .bind(report.finished_at)
                .bind(execution_task_id)
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
                if task_update.rows_affected() != 1 {
                    return Err(NativeRuntimeError::Unavailable(
                        "synthetic native agent task disappeared while finishing runtime"
                            .to_string(),
                    ));
                }
                let run_update = sqlx::query(
                    "UPDATE runs SET status = 'finished', finished_at = ?, last_heartbeat = ? \
                     WHERE id = ? AND status = 'running'",
                )
                .bind(report.finished_at)
                .bind(report.finished_at)
                .bind(synthetic_run_id)
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
                if run_update.rows_affected() != 1 {
                    return Err(NativeRuntimeError::Unavailable(
                        "synthetic native agent run disappeared while finishing runtime"
                            .to_string(),
                    ));
                }
            }
        }
        transaction.commit().await.map_err(storage_error)?;
        load_session(&self.pool, &report.session_id)
            .await?
            .ok_or_else(|| {
                NativeRuntimeError::Unavailable("runtime session disappeared".to_string())
            })
    }

    async fn record_runtime_recovery(
        &self,
        record: &RuntimeRecoveryRecord,
    ) -> Result<(), NativeRuntimeError> {
        if record.observed_at < 0 {
            return Err(NativeRuntimeError::Invalid(
                "runtime recovery timestamp is invalid".to_string(),
            ));
        }
        let disposition = recovery_disposition_str(&record.disposition);
        let value = json!({
            "runtime_session_id": record.session_id,
            "previous_attempt_id": record.previous_attempt_id,
            "next_attempt_id": record.next_attempt_id,
            "disposition": disposition,
            "observed_at": record.observed_at,
        });
        let record_json = serde_json::to_string(&value).map_err(invalid_json)?;
        let record_sha256 = sha256(record_json.as_bytes());
        let existing: Option<String> = sqlx::query_scalar(
            "SELECT record_json FROM native_runtime_recovery_history WHERE record_sha256 = ?",
        )
        .bind(&record_sha256)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        if let Some(existing) = existing {
            return if existing == record_json {
                Ok(())
            } else {
                Err(NativeRuntimeError::Conflict(
                    "runtime recovery digest collision".to_string(),
                ))
            };
        }
        sqlx::query(
            "INSERT INTO native_runtime_recovery_history (runtime_session_id, \
             previous_attempt_id, next_attempt_id, disposition, record_sha256, record_json, \
             observed_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(record.session_id.as_str())
        .bind(record.previous_attempt_id.as_str())
        .bind(
            record
                .next_attempt_id
                .as_ref()
                .map(agentd_core::types::RuntimeAttemptId::as_str),
        )
        .bind(disposition)
        .bind(record_sha256)
        .bind(record_json)
        .bind(record.observed_at)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    async fn load_runtime_session(
        &self,
        session_id: &RuntimeSessionId,
    ) -> Result<Option<DurableRuntimeSession>, NativeRuntimeError> {
        load_session(&self.pool, session_id).await
    }

    async fn load_runtime_attempt(
        &self,
        attempt_id: &RuntimeAttemptId,
    ) -> Result<Option<DurableRuntimeAttempt>, NativeRuntimeError> {
        load_attempt(&self.pool, attempt_id).await
    }

    async fn recoverable_runtime_sessions(
        &self,
        limit: u32,
    ) -> Result<Vec<DurableRuntimeSession>, NativeRuntimeError> {
        if limit == 0 || limit > 10_000 {
            return Err(NativeRuntimeError::Invalid(
                "runtime recovery limit must be between 1 and 10000".to_string(),
            ));
        }
        let ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM runtime_sessions WHERE provider IS NOT NULL \
             AND status IN ('starting', 'running', 'resume_pending') \
             ORDER BY updated_at, id LIMIT ?",
        )
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        let mut sessions = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(session) =
                load_session(&self.pool, &RuntimeSessionId::from_string(id)).await?
            {
                sessions.push(session);
            }
        }
        Ok(sessions)
    }
}

async fn load_session(
    pool: &SqlitePool,
    session_id: &RuntimeSessionId,
) -> Result<Option<DurableRuntimeSession>, NativeRuntimeError> {
    let row = sqlx::query(
        "SELECT s.id, s.execution_task_id, s.agent_profile_id, s.snapshot_authority_key, \
         s.snapshot_resource_id, s.snapshot_resource_version, s.snapshot_content_sha256, \
         s.status, s.record_version, s.terminal_reason, s.created_at, s.updated_at, s.provider, \
         s.command_sha256, s.sandbox_id, s.sandbox_profile_sha256, s.sandbox_expires_at, \
         s.max_capture_bytes, s.max_transcript_bytes, s.idle_timeout_ms, s.current_attempt_id, \
         s.native_session_ref, t.id AS transcript_object_id, t.content_sha256 AS transcript_sha256, \
         t.storage_ref AS transcript_storage_ref, t.size_bytes AS transcript_size_bytes, \
         t.truncated AS transcript_truncated, t.archived_at AS transcript_archived_at \
         FROM runtime_sessions s LEFT JOIN runtime_transcript_objects t ON t.id = s.transcript_id \
         WHERE s.id = ? AND s.provider IS NOT NULL",
    )
    .bind(session_id.as_str())
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?;
    row.as_ref().map(session_from_row).transpose()
}

async fn load_attempt(
    pool: &SqlitePool,
    attempt_id: &RuntimeAttemptId,
) -> Result<Option<DurableRuntimeAttempt>, NativeRuntimeError> {
    let row = sqlx::query(
        "SELECT id, runtime_session_id, worker_incarnation_id, host_instance_id, status, pid, \
         native_session_ref, started_at, finished_at, exit_code, is_current \
         FROM runtime_attempts WHERE id = ? AND host_instance_id IS NOT NULL",
    )
    .bind(attempt_id.as_str())
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?;
    row.as_ref().map(attempt_from_row).transpose()
}

fn session_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<DurableRuntimeSession, NativeRuntimeError> {
    let authority = AuthorityKey::new(row.get::<String, _>("snapshot_authority_key"))
        .map_err(|error| NativeRuntimeError::Unavailable(error.to_string()))?;
    let snapshot_ref = ProjectExecutionSnapshotRef::new(
        authority,
        row.get::<String, _>("snapshot_resource_id"),
        row.get::<String, _>("snapshot_resource_version"),
    )
    .map_err(|error| NativeRuntimeError::Unavailable(error.to_string()))?;
    let transcript = row
        .get::<Option<String>, _>("transcript_object_id")
        .map(|id| {
            Ok(RuntimeTranscriptRef {
                id: RuntimeTranscriptId::from_string(id),
                content_sha256: required_column(row, "transcript_sha256")?,
                storage_ref: required_column(row, "transcript_storage_ref")?,
                size_bytes: to_u64(
                    required_i64(row, "transcript_size_bytes")?,
                    "transcript size",
                )?,
                truncated: required_i64(row, "transcript_truncated")? != 0,
                archived_at: required_i64(row, "transcript_archived_at")?,
            })
        })
        .transpose()?;
    let terminal_reason = row
        .get::<Option<String>, _>("terminal_reason")
        .as_deref()
        .map(parse_terminal_reason)
        .transpose()?;
    Ok(DurableRuntimeSession {
        registration: RuntimeSessionRegistration {
            session_id: RuntimeSessionId::from_string(row.get::<String, _>("id")),
            execution_task_id: TaskRunId::from_string(row.get::<String, _>("execution_task_id")),
            agent_profile_id: AgentProfileId::from_string(row.get::<String, _>("agent_profile_id")),
            snapshot_ref,
            snapshot_content_sha256: row.get("snapshot_content_sha256"),
            provider: parse_provider(&required_column(row, "provider")?)?,
            command_sha256: required_column(row, "command_sha256")?,
            sandbox: agentd_core::ports::RuntimeSandboxRef {
                sandbox_id: required_column(row, "sandbox_id")?,
                profile_sha256: required_column(row, "sandbox_profile_sha256")?,
                expires_at: required_i64(row, "sandbox_expires_at")?,
            },
            max_capture_bytes: to_u64(required_i64(row, "max_capture_bytes")?, "capture bound")?,
            max_transcript_bytes: to_u64(
                required_i64(row, "max_transcript_bytes")?,
                "transcript bound",
            )?,
            idle_timeout_ms: to_u64(required_i64(row, "idle_timeout_ms")?, "idle timeout")?,
            created_at: row.get("created_at"),
        },
        status: parse_session_status(&row.get::<String, _>("status"))?,
        current_attempt_id: row
            .get::<Option<String>, _>("current_attempt_id")
            .map(RuntimeAttemptId::from_string),
        native_session_ref: row.get("native_session_ref"),
        transcript,
        terminal_reason,
        record_version: to_u64(row.get("record_version"), "runtime record version")?,
        updated_at: row.get("updated_at"),
    })
}

fn attempt_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<DurableRuntimeAttempt, NativeRuntimeError> {
    Ok(DurableRuntimeAttempt {
        attempt_id: RuntimeAttemptId::from_string(row.get::<String, _>("id")),
        session_id: RuntimeSessionId::from_string(row.get::<String, _>("runtime_session_id")),
        worker_incarnation_id: WorkerIncarnationId::from_string(
            row.get::<String, _>("worker_incarnation_id"),
        ),
        host_instance_id: required_column(row, "host_instance_id")?,
        status: parse_attempt_status(&row.get::<String, _>("status"))?,
        pid: row
            .get::<Option<i64>, _>("pid")
            .map(|pid| {
                u32::try_from(pid).map_err(|_| {
                    NativeRuntimeError::Unavailable("runtime pid is out of range".to_string())
                })
            })
            .transpose()?,
        native_session_ref: row.get("native_session_ref"),
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        exit_code: row.get("exit_code"),
        is_current: row.get::<i64, _>("is_current") != 0,
    })
}

async fn load_event_by_id(
    pool: &SqlitePool,
    event_id: &RuntimeEventId,
) -> Result<Option<RuntimeEvent>, NativeRuntimeError> {
    let row = sqlx::query(
        "SELECT id, runtime_session_id, runtime_attempt_id, event_index, kind, payload_sha256, \
         payload_json, occurred_at FROM native_runtime_events WHERE id = ?",
    )
    .bind(event_id.as_str())
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?;
    row.as_ref().map(event_from_row).transpose()
}

fn event_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<RuntimeEvent, NativeRuntimeError> {
    Ok(RuntimeEvent {
        id: RuntimeEventId::from_string(row.get::<String, _>("id")),
        session_id: RuntimeSessionId::from_string(row.get::<String, _>("runtime_session_id")),
        attempt_id: RuntimeAttemptId::from_string(row.get::<String, _>("runtime_attempt_id")),
        event_index: to_u64(row.get("event_index"), "runtime event cursor")?,
        kind: parse_event_kind(&row.get::<String, _>("kind"))?,
        payload: serde_json::from_str(&row.get::<String, _>("payload_json"))
            .map_err(invalid_json)?,
        payload_sha256: row.get("payload_sha256"),
        occurred_at: row.get("occurred_at"),
    })
}

async fn require_attempt_session(
    transaction: &mut SqliteConnection,
    attempt_id: &RuntimeAttemptId,
    session_id: &RuntimeSessionId,
) -> Result<(), NativeRuntimeError> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM runtime_attempts WHERE id = ? AND runtime_session_id = ? \
         AND host_instance_id IS NOT NULL",
    )
    .bind(attempt_id.as_str())
    .bind(session_id.as_str())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(storage_error)?;
    if found == Some(1) {
        Ok(())
    } else {
        Err(NativeRuntimeError::NotFound(format!(
            "runtime attempt {attempt_id} for session {session_id}"
        )))
    }
}

async fn insert_transcript(
    transaction: &mut SqliteConnection,
    report: &RuntimeShutdownReport,
) -> Result<(), NativeRuntimeError> {
    let existing = sqlx::query(
        "SELECT runtime_session_id, runtime_attempt_id, content_sha256, storage_ref, \
         size_bytes, truncated, archived_at FROM runtime_transcript_objects WHERE id = ?",
    )
    .bind(report.transcript.id.as_str())
    .fetch_optional(&mut *transaction)
    .await
    .map_err(storage_error)?;
    if let Some(existing) = existing {
        let exact = existing.get::<String, _>("runtime_session_id") == report.session_id.as_str()
            && existing.get::<String, _>("runtime_attempt_id") == report.attempt_id.as_str()
            && existing.get::<String, _>("content_sha256") == report.transcript.content_sha256
            && existing.get::<String, _>("storage_ref") == report.transcript.storage_ref
            && to_u64(existing.get("size_bytes"), "transcript size")?
                == report.transcript.size_bytes
            && (existing.get::<i64, _>("truncated") != 0) == report.transcript.truncated
            && existing.get::<i64, _>("archived_at") == report.transcript.archived_at;
        return if exact {
            Ok(())
        } else {
            Err(NativeRuntimeError::Conflict(
                "runtime transcript id replay differs".to_string(),
            ))
        };
    }
    sqlx::query(
        "INSERT INTO runtime_transcript_objects (id, runtime_session_id, runtime_attempt_id, \
         content_sha256, storage_ref, size_bytes, truncated, archived_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(report.transcript.id.as_str())
    .bind(report.session_id.as_str())
    .bind(report.attempt_id.as_str())
    .bind(&report.transcript.content_sha256)
    .bind(&report.transcript.storage_ref)
    .bind(to_i64(report.transcript.size_bytes, "transcript size")?)
    .bind(i64::from(report.transcript.truncated))
    .bind(report.transcript.archived_at)
    .execute(&mut *transaction)
    .await
    .map_err(storage_error)?;
    Ok(())
}

fn validate_registration(
    registration: &RuntimeSessionRegistration,
) -> Result<(), NativeRuntimeError> {
    if !valid_id(registration.session_id.as_str(), "rs_")
        || !valid_id(registration.execution_task_id.as_str(), "tr_")
        || !valid_id(registration.agent_profile_id.as_str(), "ap_")
        || !valid_sha256(&registration.snapshot_content_sha256)
        || !valid_sha256(&registration.command_sha256)
        || registration.sandbox.sandbox_id.trim().is_empty()
        || !valid_sha256(&registration.sandbox.profile_sha256)
        || registration.created_at < 0
        || registration.sandbox.expires_at <= registration.created_at
        || registration.max_capture_bytes == 0
        || registration.max_capture_bytes > 16 * 1024 * 1024
        || registration.max_transcript_bytes == 0
        || registration.max_transcript_bytes > 1024 * 1024 * 1024
        || registration.idle_timeout_ms == 0
    {
        return Err(NativeRuntimeError::Invalid(
            "runtime session registration is invalid or exceeds bounds".to_string(),
        ));
    }
    Ok(())
}

fn validate_attempt_identity(
    attempt_id: &RuntimeAttemptId,
    worker_incarnation_id: &WorkerIncarnationId,
    host_instance_id: &str,
    started_at: i64,
) -> Result<(), NativeRuntimeError> {
    if !valid_id(attempt_id.as_str(), "ra_")
        || !valid_id(worker_incarnation_id.as_str(), "wi_")
        || host_instance_id.trim().is_empty()
        || host_instance_id.len() > 256
        || host_instance_id.contains('\0')
        || started_at < 0
    {
        return Err(NativeRuntimeError::Invalid(
            "runtime attempt identity is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_event(event: &RuntimeEvent) -> Result<(), NativeRuntimeError> {
    let payload = serde_json::to_vec(&event.payload).map_err(invalid_json)?;
    if !valid_id(event.id.as_str(), "re_")
        || !valid_id(event.session_id.as_str(), "rs_")
        || !valid_id(event.attempt_id.as_str(), "ra_")
        || event.event_index == 0
        || event.occurred_at < 0
        || !valid_sha256(&event.payload_sha256)
        || sha256(&payload) != event.payload_sha256
    {
        return Err(NativeRuntimeError::Invalid(
            "runtime event is invalid or has a mismatched payload digest".to_string(),
        ));
    }
    if let RuntimeEventPayload::Input {
        idempotency_key,
        input_sha256,
        byte_count,
    } = &event.payload
    {
        if idempotency_key.trim().is_empty()
            || idempotency_key.len() > 512
            || !valid_sha256(input_sha256)
            || *byte_count == 0
        {
            return Err(NativeRuntimeError::Invalid(
                "runtime input event contains invalid digest metadata".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_report(report: &RuntimeShutdownReport) -> Result<(), NativeRuntimeError> {
    if !valid_id(report.session_id.as_str(), "rs_")
        || !valid_id(report.attempt_id.as_str(), "ra_")
        || !valid_id(report.transcript.id.as_str(), "rx_")
        || !valid_sha256(&report.transcript.content_sha256)
        || report.transcript.storage_ref != format!("sha256:{}", report.transcript.content_sha256)
        || report.transcript.archived_at > report.finished_at
        || report.finished_at < 0
    {
        return Err(NativeRuntimeError::Invalid(
            "runtime shutdown report is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_native_ref(reference: &str) -> Result<(), NativeRuntimeError> {
    if reference.is_empty()
        || reference.len() > 512
        || reference != reference.trim()
        || reference.chars().any(char::is_control)
    {
        Err(NativeRuntimeError::Invalid(
            "provider native session reference is invalid".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn same_event_identity(left: &RuntimeEvent, right: &RuntimeEvent) -> bool {
    left.id == right.id
        && left.session_id == right.session_id
        && left.attempt_id == right.attempt_id
        && left.kind == right.kind
        && left.payload == right.payload
        && left.payload_sha256 == right.payload_sha256
        && left.occurred_at == right.occurred_at
}

fn terminal_status(reason: RuntimeTerminalReason) -> RuntimeSessionStatus {
    match reason {
        RuntimeTerminalReason::Completed => RuntimeSessionStatus::Completed,
        RuntimeTerminalReason::Cancelled | RuntimeTerminalReason::IdleTimeout => {
            RuntimeSessionStatus::Cancelled
        }
        RuntimeTerminalReason::Failed => RuntimeSessionStatus::Failed,
        RuntimeTerminalReason::RuntimeGone | RuntimeTerminalReason::WorkerLost => {
            RuntimeSessionStatus::Lost
        }
    }
}

const fn terminal_reason_str(reason: RuntimeTerminalReason) -> &'static str {
    match reason {
        RuntimeTerminalReason::Completed => "completed",
        RuntimeTerminalReason::Failed => "failed",
        RuntimeTerminalReason::Cancelled => "cancelled",
        RuntimeTerminalReason::IdleTimeout => "idle_timeout",
        RuntimeTerminalReason::RuntimeGone => "runtime_gone",
        RuntimeTerminalReason::WorkerLost => "worker_lost",
    }
}

fn parse_terminal_reason(value: &str) -> Result<RuntimeTerminalReason, NativeRuntimeError> {
    match value {
        "completed" => Ok(RuntimeTerminalReason::Completed),
        "failed" => Ok(RuntimeTerminalReason::Failed),
        "cancelled" => Ok(RuntimeTerminalReason::Cancelled),
        "idle_timeout" => Ok(RuntimeTerminalReason::IdleTimeout),
        "runtime_gone" => Ok(RuntimeTerminalReason::RuntimeGone),
        "worker_lost" => Ok(RuntimeTerminalReason::WorkerLost),
        _ => Err(NativeRuntimeError::Unavailable(
            "stored runtime terminal reason is invalid".to_string(),
        )),
    }
}

const fn recovery_disposition_str(disposition: &RuntimeRecoveryDisposition) -> &'static str {
    match disposition {
        RuntimeRecoveryDisposition::Live { .. } => "live",
        RuntimeRecoveryDisposition::Resumable { .. } => "resumable",
        RuntimeRecoveryDisposition::RuntimeGone => "runtime_gone",
    }
}

fn parse_provider(value: &str) -> Result<RuntimeProvider, NativeRuntimeError> {
    match value {
        "codex" => Ok(RuntimeProvider::Codex),
        "claude_code" => Ok(RuntimeProvider::ClaudeCode),
        "custom" => Ok(RuntimeProvider::Custom),
        _ => Err(NativeRuntimeError::Unavailable(
            "stored runtime provider is invalid".to_string(),
        )),
    }
}

fn parse_session_status(value: &str) -> Result<RuntimeSessionStatus, NativeRuntimeError> {
    RuntimeSessionStatus::try_from(value).map_err(|_| {
        NativeRuntimeError::Unavailable("stored runtime session status is invalid".to_string())
    })
}

fn parse_attempt_status(value: &str) -> Result<RuntimeAttemptStatus, NativeRuntimeError> {
    RuntimeAttemptStatus::try_from(value).map_err(|_| {
        NativeRuntimeError::Unavailable("stored runtime attempt status is invalid".to_string())
    })
}

fn parse_event_kind(value: &str) -> Result<RuntimeEventKind, NativeRuntimeError> {
    match value {
        "starting" => Ok(RuntimeEventKind::Starting),
        "started" => Ok(RuntimeEventKind::Started),
        "output" => Ok(RuntimeEventKind::Output),
        "input_accepted" => Ok(RuntimeEventKind::InputAccepted),
        "resized" => Ok(RuntimeEventKind::Resized),
        "interrupted" => Ok(RuntimeEventKind::Interrupted),
        "native_session_ref" => Ok(RuntimeEventKind::NativeSessionRef),
        "exited" => Ok(RuntimeEventKind::Exited),
        "runtime_gone" => Ok(RuntimeEventKind::RuntimeGone),
        "recovered" => Ok(RuntimeEventKind::Recovered),
        "transcript_archived" => Ok(RuntimeEventKind::TranscriptArchived),
        "shutdown" => Ok(RuntimeEventKind::Shutdown),
        _ => Err(NativeRuntimeError::Unavailable(
            "stored runtime event kind is invalid".to_string(),
        )),
    }
}

fn required_column(
    row: &sqlx::sqlite::SqliteRow,
    column: &str,
) -> Result<String, NativeRuntimeError> {
    row.get::<Option<String>, _>(column).ok_or_else(|| {
        NativeRuntimeError::Unavailable(format!("stored runtime column {column} is missing"))
    })
}

fn required_i64(row: &sqlx::sqlite::SqliteRow, column: &str) -> Result<i64, NativeRuntimeError> {
    row.get::<Option<i64>, _>(column).ok_or_else(|| {
        NativeRuntimeError::Unavailable(format!("stored runtime column {column} is missing"))
    })
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value.len() == 29
        && value.starts_with(prefix)
        && value[prefix.len()..].bytes().all(|byte| {
            byte.is_ascii_digit()
                || matches!(
                    byte,
                    b'A'..=b'H' | b'J'..=b'K' | b'M'..=b'N' | b'P'..=b'T' | b'V'..=b'Z'
                )
        })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn to_i64(value: u64, name: &str) -> Result<i64, NativeRuntimeError> {
    i64::try_from(value)
        .map_err(|_| NativeRuntimeError::Invalid(format!("runtime {name} is too large")))
}

fn to_u64(value: i64, name: &str) -> Result<u64, NativeRuntimeError> {
    u64::try_from(value)
        .map_err(|_| NativeRuntimeError::Unavailable(format!("stored runtime {name} is invalid")))
}

fn invalid_json(error: impl std::fmt::Display) -> NativeRuntimeError {
    NativeRuntimeError::Invalid(format!("runtime JSON is invalid: {error}"))
}

fn storage_error(error: impl std::fmt::Display) -> NativeRuntimeError {
    NativeRuntimeError::Unavailable(format!("native runtime store unavailable: {error}"))
}
