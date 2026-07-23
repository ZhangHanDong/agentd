//! Control-plane port used by native workers.
//!
//! Native process resources remain worker-local, while runtime identity and
//! attempt state remain owned by the control plane. Implementations may be
//! `SQLite` (daemon) or authenticated HTTP (remote worker).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ports::execution_evidence::ExecutionSnapshotLink;
use crate::types::{
    RunId, RuntimeAttemptId, RuntimeAttemptStatus, RuntimeSessionId, TaskRunId, WorkerIncarnationId,
};

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NativeRuntimeControlError {
    #[error("native runtime control-plane input is invalid: {0}")]
    Invalid(String),
    #[error("native runtime control-plane resource not found: {0}")]
    NotFound(String),
    #[error("native runtime control-plane conflict: {0}")]
    Conflict(String),
    #[error("native runtime control-plane unavailable: {0}")]
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRuntimeAttemptStart {
    pub attempt_id: RuntimeAttemptId,
    pub session_id: RuntimeSessionId,
    pub task_id: TaskRunId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRuntimeAttemptState {
    pub attempt_id: RuntimeAttemptId,
    pub session_id: RuntimeSessionId,
    pub status: RuntimeAttemptStatus,
    pub native_session_ref: Option<String>,
    /// Process exit code for a terminal `Exited` reconciliation. `None` for
    /// non-terminal updates or when the code is unavailable; a terminal
    /// reconciliation with `Some(0)` completes the session, otherwise it fails.
    #[serde(default)]
    pub exit_code: Option<i32>,
    pub observed_at: i64,
}

/// Wire request for session/task ownership validation shared by the HTTP
/// transport and its client adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRuntimeSessionValidate {
    pub session_id: RuntimeSessionId,
    pub task_id: TaskRunId,
}

/// Control-plane view of one runtime session for worker resume decisions.
///
/// `latest_native_session_ref` carries the provider-native thread reference of
/// the most recent attempt so a worker can build `codex exec resume` without
/// reading the daemon database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRuntimeSessionView {
    pub session_id: RuntimeSessionId,
    pub task_id: TaskRunId,
    pub run_id: RunId,
    pub status: crate::types::RuntimeSessionStatus,
    pub latest_native_session_ref: Option<String>,
    /// The session's immutable authority snapshot reference, carried so a
    /// remote worker can populate `ExecutionEvidenceLinks.snapshot` on an
    /// artifact acknowledgement without opening the daemon database.
    #[serde(default)]
    pub snapshot: ExecutionSnapshotLink,
}

#[async_trait::async_trait]
pub trait NativeRuntimeControlPort: Send + Sync {
    /// Confirm that the control-plane session owns the requested task.
    async fn validate_session_task(
        &self,
        session_id: &RuntimeSessionId,
        task_id: &TaskRunId,
    ) -> Result<(), NativeRuntimeControlError>;

    /// Read the session status and latest provider-native session reference.
    /// Returns `None` when the control plane does not know the session.
    async fn session_view(
        &self,
        session_id: &RuntimeSessionId,
    ) -> Result<Option<NativeRuntimeSessionView>, NativeRuntimeControlError>;

    /// Resolve the runtime session the control plane bound to a task, if any.
    async fn session_for_task(
        &self,
        task_id: &TaskRunId,
    ) -> Result<Option<NativeRuntimeSessionView>, NativeRuntimeControlError>;

    /// Create the worker-bound attempt under the control-plane lease.
    async fn start_attempt(
        &self,
        request: &NativeRuntimeAttemptStart,
    ) -> Result<NativeRuntimeAttemptState, NativeRuntimeControlError>;

    async fn update_attempt(
        &self,
        state: &NativeRuntimeAttemptState,
    ) -> Result<(), NativeRuntimeControlError>;

    async fn mark_attempt_terminal(
        &self,
        state: &NativeRuntimeAttemptState,
    ) -> Result<(), NativeRuntimeControlError>;
}
