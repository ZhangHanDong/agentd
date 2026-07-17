//! Native interactive runtime contracts, independent of legacy spawn/tmux backends.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::types::{
    AgentProfileId, CapabilityAdmission, PreparedSandbox, ProjectExecutionSnapshotRef,
    RuntimeAttemptId, RuntimeAttemptStatus, RuntimeEventId, RuntimeSessionId, RuntimeSessionStatus,
    RuntimeTranscriptId, TaskRunId, WorkerIncarnationId,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProvider {
    Codex,
    ClaudeCode,
    Custom,
}

impl RuntimeProvider {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::ClaudeCode => "claude_code",
            Self::Custom => "custom",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeCommand {
    pub program: String,
    pub arguments: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub working_directory: PathBuf,
}

impl std::fmt::Debug for RuntimeCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeCommand")
            .field("program", &self.program)
            .field("argument_count", &self.arguments.len())
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .field("working_directory", &self.working_directory)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDimensions {
    pub rows: u16,
    pub columns: u16,
    pub pixel_width: u16,
    pub pixel_height: u16,
}

impl RuntimeDimensions {
    pub fn validate(self) -> Result<(), NativeRuntimeError> {
        if self.rows == 0 || self.columns == 0 || self.rows > 1_000 || self.columns > 1_000 {
            return Err(NativeRuntimeError::Invalid(
                "runtime dimensions are outside bounds".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSandboxRef {
    pub sandbox_id: String,
    pub profile_sha256: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLaunchRequest {
    pub session_id: RuntimeSessionId,
    pub attempt_id: RuntimeAttemptId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub provider: RuntimeProvider,
    pub command: RuntimeCommand,
    pub dimensions: RuntimeDimensions,
    pub sandbox: RuntimeSandboxRef,
    pub native_session_ref: Option<String>,
    pub max_capture_bytes: usize,
    pub max_transcript_bytes: u64,
    pub idle_timeout_ms: u64,
    pub requested_at: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeSandboxCommandRequest {
    pub admission: CapabilityAdmission,
    pub sandbox: PreparedSandbox,
    pub argv: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub working_directory: PathBuf,
    pub observed_at: i64,
}

impl std::fmt::Debug for RuntimeSandboxCommandRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeSandboxCommandRequest")
            .field("sandbox_id", &self.sandbox.sandbox_id)
            .field("argument_count", &self.argv.len())
            .field(
                "environment_keys",
                &self.environment.keys().collect::<Vec<_>>(),
            )
            .field("working_directory", &self.working_directory)
            .field("observed_at", &self.observed_at)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeHandle {
    pub session_id: RuntimeSessionId,
    pub attempt_id: RuntimeAttemptId,
    pub provider: RuntimeProvider,
    pub pid: u32,
    pub native_session_ref: Option<String>,
    pub started_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTextInput {
    pub attempt_id: RuntimeAttemptId,
    pub idempotency_key: String,
    pub text: String,
    pub submit: bool,
    pub observed_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKey {
    Enter,
    Tab,
    Escape,
    Backspace,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    CtrlC,
    CtrlD,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeKeyInput {
    pub attempt_id: RuntimeAttemptId,
    pub idempotency_key: String,
    pub key: RuntimeKey,
    pub repeat: u16,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeInputAck {
    pub attempt_id: RuntimeAttemptId,
    pub idempotency_key: String,
    pub input_sha256: String,
    pub accepted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeResizeRequest {
    pub attempt_id: RuntimeAttemptId,
    pub idempotency_key: String,
    pub dimensions: RuntimeDimensions,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeWaitRequest {
    pub attempt_id: RuntimeAttemptId,
    pub after_event_index: u64,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeShutdownMethod {
    Graceful,
    Interrupt,
    Kill,
    AlreadyExited,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeTerminalReason {
    Completed,
    Failed,
    Cancelled,
    IdleTimeout,
    RuntimeGone,
    WorkerLost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeShutdownRequest {
    pub attempt_id: RuntimeAttemptId,
    pub idempotency_key: String,
    pub graceful_timeout_ms: u64,
    pub interrupt_timeout_ms: u64,
    pub reason: RuntimeTerminalReason,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTranscriptRef {
    pub id: RuntimeTranscriptId,
    pub content_sha256: String,
    pub storage_ref: String,
    pub size_bytes: u64,
    pub truncated: bool,
    pub archived_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeShutdownReport {
    pub session_id: RuntimeSessionId,
    pub attempt_id: RuntimeAttemptId,
    pub method: RuntimeShutdownMethod,
    pub terminal_reason: RuntimeTerminalReason,
    pub exit_code: Option<i32>,
    pub transcript: RuntimeTranscriptRef,
    pub finished_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEventKind {
    Starting,
    Started,
    Output,
    InputAccepted,
    Resized,
    Interrupted,
    NativeSessionRef,
    Exited,
    RuntimeGone,
    Recovered,
    TranscriptArchived,
    Shutdown,
}

impl RuntimeEventKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Started => "started",
            Self::Output => "output",
            Self::InputAccepted => "input_accepted",
            Self::Resized => "resized",
            Self::Interrupted => "interrupted",
            Self::NativeSessionRef => "native_session_ref",
            Self::Exited => "exited",
            Self::RuntimeGone => "runtime_gone",
            Self::Recovered => "recovered",
            Self::TranscriptArchived => "transcript_archived",
            Self::Shutdown => "shutdown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEventPayload {
    State {
        status: RuntimeAttemptStatus,
    },
    Process {
        pid: u32,
    },
    Output {
        text: String,
        byte_count: u64,
    },
    Input {
        idempotency_key: String,
        input_sha256: String,
        byte_count: u64,
    },
    Resize {
        dimensions: RuntimeDimensions,
    },
    NativeSession {
        reference: String,
    },
    Exit {
        exit_code: Option<i32>,
    },
    Recovery {
        disposition: String,
    },
    Transcript {
        reference: RuntimeTranscriptRef,
    },
    Terminal {
        reason: RuntimeTerminalReason,
        method: RuntimeShutdownMethod,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    pub id: RuntimeEventId,
    pub session_id: RuntimeSessionId,
    pub attempt_id: RuntimeAttemptId,
    pub event_index: u64,
    pub kind: RuntimeEventKind,
    pub payload: RuntimeEventPayload,
    pub payload_sha256: String,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub session_id: RuntimeSessionId,
    pub attempt_id: RuntimeAttemptId,
    pub provider: RuntimeProvider,
    pub status: RuntimeAttemptStatus,
    pub pid: Option<u32>,
    pub dimensions: RuntimeDimensions,
    pub event_index: u64,
    pub output_tail: String,
    pub output_truncated: bool,
    pub native_session_ref: Option<String>,
    pub transcript: Option<RuntimeTranscriptRef>,
    pub exit_code: Option<i32>,
    pub started_at: i64,
    pub last_output_at: i64,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRecoveryRequest {
    pub session_id: RuntimeSessionId,
    pub attempt_id: RuntimeAttemptId,
    pub provider: RuntimeProvider,
    pub pid: Option<u32>,
    pub native_session_ref: Option<String>,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
pub enum RuntimeRecoveryDisposition {
    Live { snapshot: RuntimeSnapshot },
    Resumable { native_session_ref: String },
    RuntimeGone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeSessionRegistration {
    pub session_id: RuntimeSessionId,
    pub execution_task_id: TaskRunId,
    pub agent_profile_id: AgentProfileId,
    pub snapshot_ref: ProjectExecutionSnapshotRef,
    pub snapshot_content_sha256: String,
    pub provider: RuntimeProvider,
    pub command_sha256: String,
    pub sandbox: RuntimeSandboxRef,
    pub max_capture_bytes: u64,
    pub max_transcript_bytes: u64,
    pub idle_timeout_ms: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableRuntimeSession {
    pub registration: RuntimeSessionRegistration,
    pub status: RuntimeSessionStatus,
    pub current_attempt_id: Option<RuntimeAttemptId>,
    pub native_session_ref: Option<String>,
    pub transcript: Option<RuntimeTranscriptRef>,
    pub terminal_reason: Option<RuntimeTerminalReason>,
    pub record_version: u64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DurableRuntimeAttempt {
    pub attempt_id: RuntimeAttemptId,
    pub session_id: RuntimeSessionId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub host_instance_id: String,
    pub status: RuntimeAttemptStatus,
    pub pid: Option<u32>,
    pub native_session_ref: Option<String>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub is_current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeView {
    pub session: DurableRuntimeSession,
    pub attempt: Option<DurableRuntimeAttempt>,
    pub live: Option<RuntimeSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeRecoveryRecord {
    pub session_id: RuntimeSessionId,
    pub previous_attempt_id: RuntimeAttemptId,
    pub next_attempt_id: Option<RuntimeAttemptId>,
    pub disposition: RuntimeRecoveryDisposition,
    pub observed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NativeRuntimeError {
    #[error("invalid native runtime request: {0}")]
    Invalid(String),
    #[error("native runtime denied: {0}")]
    Denied(String),
    #[error("native runtime resource not found: {0}")]
    NotFound(String),
    #[error("native runtime conflict: {0}")]
    Conflict(String),
    #[error("native runtime unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait RuntimeBackend: Send + Sync {
    async fn launch(
        &self,
        request: RuntimeLaunchRequest,
    ) -> Result<RuntimeHandle, NativeRuntimeError>;
    async fn send_text(
        &self,
        request: RuntimeTextInput,
    ) -> Result<RuntimeInputAck, NativeRuntimeError>;
    async fn send_key(
        &self,
        request: RuntimeKeyInput,
    ) -> Result<RuntimeInputAck, NativeRuntimeError>;
    async fn resize(
        &self,
        request: RuntimeResizeRequest,
    ) -> Result<RuntimeSnapshot, NativeRuntimeError>;
    async fn interrupt(
        &self,
        request: RuntimeKeyInput,
    ) -> Result<RuntimeInputAck, NativeRuntimeError>;
    async fn shutdown(
        &self,
        request: RuntimeShutdownRequest,
    ) -> Result<RuntimeShutdownReport, NativeRuntimeError>;
    async fn snapshot(
        &self,
        attempt_id: &RuntimeAttemptId,
    ) -> Result<Option<RuntimeSnapshot>, NativeRuntimeError>;
    async fn events_after(
        &self,
        attempt_id: &RuntimeAttemptId,
        after_event_index: u64,
        limit: u32,
    ) -> Result<Vec<RuntimeEvent>, NativeRuntimeError>;
    async fn wait(
        &self,
        request: RuntimeWaitRequest,
    ) -> Result<RuntimeSnapshot, NativeRuntimeError>;
    async fn recover(
        &self,
        request: &RuntimeRecoveryRequest,
    ) -> Result<RuntimeRecoveryDisposition, NativeRuntimeError>;
    async fn reap_idle(
        &self,
        observed_at: i64,
    ) -> Result<Vec<RuntimeShutdownReport>, NativeRuntimeError>;
}

#[async_trait::async_trait]
pub trait InteractiveSandboxPort: Send + Sync {
    async fn interactive_command(
        &self,
        request: &RuntimeSandboxCommandRequest,
    ) -> Result<RuntimeCommand, NativeRuntimeError>;
}

#[async_trait::async_trait]
pub trait RuntimeEventPort: Send + Sync {
    async fn append_runtime_event(
        &self,
        event: &RuntimeEvent,
    ) -> Result<RuntimeEvent, NativeRuntimeError>;

    async fn runtime_events_after(
        &self,
        session_id: &RuntimeSessionId,
        after_event_index: u64,
        limit: u32,
    ) -> Result<Vec<RuntimeEvent>, NativeRuntimeError>;
}

#[async_trait::async_trait]
pub trait RuntimeArchivePort: Send + Sync {
    async fn archive_runtime_transcript(
        &self,
        session_id: &RuntimeSessionId,
        attempt_id: &RuntimeAttemptId,
        redacted_transcript: &[u8],
        truncated: bool,
        observed_at: i64,
    ) -> Result<RuntimeTranscriptRef, NativeRuntimeError>;
}

#[async_trait::async_trait]
pub trait RuntimeLedgerPort: RuntimeEventPort + Send + Sync {
    async fn register_runtime_session(
        &self,
        registration: &RuntimeSessionRegistration,
    ) -> Result<DurableRuntimeSession, NativeRuntimeError>;

    async fn begin_runtime_attempt(
        &self,
        session_id: &RuntimeSessionId,
        attempt_id: &RuntimeAttemptId,
        worker_incarnation_id: &WorkerIncarnationId,
        host_instance_id: &str,
        started_at: i64,
    ) -> Result<DurableRuntimeAttempt, NativeRuntimeError>;

    async fn mark_runtime_attempt_running(
        &self,
        handle: &RuntimeHandle,
    ) -> Result<DurableRuntimeAttempt, NativeRuntimeError>;

    async fn update_runtime_native_ref(
        &self,
        session_id: &RuntimeSessionId,
        attempt_id: &RuntimeAttemptId,
        native_session_ref: &str,
        observed_at: i64,
    ) -> Result<DurableRuntimeAttempt, NativeRuntimeError>;

    async fn mark_runtime_attempt_gone(
        &self,
        session_id: &RuntimeSessionId,
        attempt_id: &RuntimeAttemptId,
        native_session_ref: Option<&str>,
        observed_at: i64,
    ) -> Result<DurableRuntimeSession, NativeRuntimeError>;

    async fn finish_runtime_attempt(
        &self,
        report: &RuntimeShutdownReport,
    ) -> Result<DurableRuntimeSession, NativeRuntimeError>;

    async fn record_runtime_recovery(
        &self,
        record: &RuntimeRecoveryRecord,
    ) -> Result<(), NativeRuntimeError>;

    async fn load_runtime_session(
        &self,
        session_id: &RuntimeSessionId,
    ) -> Result<Option<DurableRuntimeSession>, NativeRuntimeError>;

    async fn load_runtime_attempt(
        &self,
        attempt_id: &RuntimeAttemptId,
    ) -> Result<Option<DurableRuntimeAttempt>, NativeRuntimeError>;

    async fn recoverable_runtime_sessions(
        &self,
        limit: u32,
    ) -> Result<Vec<DurableRuntimeSession>, NativeRuntimeError>;
}
