//! Native PTY process ownership and bounded semantic capture.

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentd_core::ports::{
    ContentRedactionPort, NativeRuntimeError, RuntimeArchivePort, RuntimeBackend,
    RuntimeDimensions, RuntimeEvent, RuntimeEventKind, RuntimeEventPayload, RuntimeEventPort,
    RuntimeHandle, RuntimeInputAck, RuntimeKey, RuntimeKeyInput, RuntimeLaunchRequest,
    RuntimeProvider, RuntimeRecoveryDisposition, RuntimeRecoveryRequest, RuntimeResizeRequest,
    RuntimeShutdownMethod, RuntimeShutdownReport, RuntimeShutdownRequest, RuntimeSnapshot,
    RuntimeTerminalReason, RuntimeTextInput, RuntimeTranscriptRef, RuntimeWaitRequest,
};
use agentd_core::types::{
    RuntimeAttemptId, RuntimeAttemptStatus, RuntimeEventId, RuntimeSessionId,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, Notify, RwLock, mpsc};

use crate::RuntimeProviderAdapter;

const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_CAPTURE_BYTES: usize = 16 * 1024 * 1024;
const MAX_TRANSCRIPT_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_LOCAL_EVENTS: usize = 4_096;
const READ_CHUNK_BYTES: usize = 16 * 1024;
const MAX_REDACTION_RECORD_BYTES: usize = 256 * 1024;

/// Native process runtime. Logical session and attempt durability remain in the ledger port.
#[derive(Clone)]
pub struct NativePtyRuntime {
    inner: Arc<RuntimeInner>,
}

impl std::fmt::Debug for NativePtyRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativePtyRuntime")
            .finish_non_exhaustive()
    }
}

struct RuntimeInner {
    redactor: Arc<dyn ContentRedactionPort>,
    archive: Arc<dyn RuntimeArchivePort>,
    event_port: Arc<dyn RuntimeEventPort>,
    process_host: Arc<dyn RuntimeProcessHost>,
    attempts: RwLock<HashMap<RuntimeAttemptId, Arc<AttemptState>>>,
}

struct AttemptState {
    session_id: RuntimeSessionId,
    attempt_id: RuntimeAttemptId,
    provider: RuntimeProvider,
    pid: u32,
    started_at: i64,
    idle_timeout_ms: u64,
    max_capture_bytes: usize,
    max_transcript_bytes: usize,
    child: Mutex<Box<dyn RuntimeChildControl>>,
    master: StdMutex<Box<dyn RuntimePtyControl>>,
    writer: Mutex<Box<dyn Write + Send>>,
    dimensions: RwLock<RuntimeDimensions>,
    mutable: RwLock<MutableAttempt>,
    transcript_bytes: Mutex<Vec<u8>>,
    transcript_truncated: AtomicBool,
    event_index: AtomicU64,
    events: RwLock<VecDeque<RuntimeEvent>>,
    action_digests: Mutex<HashMap<String, String>>,
    terminal_started: AtomicBool,
    output_complete: AtomicBool,
    output_notify: Notify,
    notify: Notify,
}

struct MutableAttempt {
    status: RuntimeAttemptStatus,
    output_tail: Vec<u8>,
    output_truncated: bool,
    native_session_ref: Option<String>,
    transcript: Option<RuntimeTranscriptRef>,
    exit_code: Option<i32>,
    last_output_at: i64,
    last_activity_at: i64,
    finished_at: Option<i64>,
    terminal_report: Option<RuntimeShutdownReport>,
}

impl NativePtyRuntime {
    #[must_use]
    pub fn new(
        redactor: Arc<dyn ContentRedactionPort>,
        archive: Arc<dyn RuntimeArchivePort>,
        event_port: Arc<dyn RuntimeEventPort>,
    ) -> Self {
        Self::with_process_host(redactor, archive, event_port, Arc::new(NativeProcessHost))
    }

    #[must_use]
    pub fn with_process_host(
        redactor: Arc<dyn ContentRedactionPort>,
        archive: Arc<dyn RuntimeArchivePort>,
        event_port: Arc<dyn RuntimeEventPort>,
        process_host: Arc<dyn RuntimeProcessHost>,
    ) -> Self {
        Self {
            inner: Arc::new(RuntimeInner {
                redactor,
                archive,
                event_port,
                process_host,
                attempts: RwLock::new(HashMap::new()),
            }),
        }
    }
}

/// Spawn result owned by [`NativePtyRuntime`].
pub struct SpawnedRuntimeProcess {
    pub pid: u32,
    pub child: Box<dyn RuntimeChildControl>,
    pub controller: Box<dyn RuntimePtyControl>,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
}

impl std::fmt::Debug for SpawnedRuntimeProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpawnedRuntimeProcess")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

pub trait RuntimeProcessHost: Send + Sync + std::fmt::Debug {
    fn spawn(
        &self,
        command: &agentd_core::ports::RuntimeCommand,
        dimensions: RuntimeDimensions,
    ) -> Result<SpawnedRuntimeProcess, NativeRuntimeError>;
}

pub trait RuntimeChildControl: Send + Sync + std::fmt::Debug {
    fn try_wait(&mut self) -> Result<Option<i32>, NativeRuntimeError>;
    fn kill(&mut self) -> Result<(), NativeRuntimeError>;
}

pub trait RuntimePtyControl: Send + std::fmt::Debug {
    fn resize(&self, dimensions: RuntimeDimensions) -> Result<(), NativeRuntimeError>;
}

#[derive(Debug)]
struct NativeProcessHost;

impl RuntimeProcessHost for NativeProcessHost {
    fn spawn(
        &self,
        command: &agentd_core::ports::RuntimeCommand,
        dimensions: RuntimeDimensions,
    ) -> Result<SpawnedRuntimeProcess, NativeRuntimeError> {
        let pair = native_pty_system()
            .openpty(to_pty_size(dimensions))
            .map_err(pty_unavailable)?;
        let reader = pair.master.try_clone_reader().map_err(pty_unavailable)?;
        let writer = pair.master.take_writer().map_err(pty_unavailable)?;
        let mut builder = CommandBuilder::new(&command.program);
        builder.args(&command.arguments);
        builder.cwd(&command.working_directory);
        for (key, value) in &command.environment {
            builder.env(key, value);
        }
        let child = pair.slave.spawn_command(builder).map_err(pty_unavailable)?;
        let pid = child.process_id().ok_or_else(|| {
            NativeRuntimeError::Unavailable(
                "native runtime did not expose a process id".to_string(),
            )
        })?;
        drop(pair.slave);
        Ok(SpawnedRuntimeProcess {
            pid,
            child: Box::new(NativeChild(child)),
            controller: Box::new(NativePtyControl(pair.master)),
            reader,
            writer,
        })
    }
}

struct NativeChild(Box<dyn Child + Send + Sync>);

impl std::fmt::Debug for NativeChild {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeChild")
            .finish_non_exhaustive()
    }
}

impl RuntimeChildControl for NativeChild {
    fn try_wait(&mut self) -> Result<Option<i32>, NativeRuntimeError> {
        self.0
            .try_wait()
            .map_err(pty_unavailable)?
            .map(|status| {
                i32::try_from(status.exit_code()).map_err(|_| {
                    NativeRuntimeError::Unavailable("runtime exit code is out of range".to_string())
                })
            })
            .transpose()
    }

    fn kill(&mut self) -> Result<(), NativeRuntimeError> {
        self.0.kill().map_err(pty_unavailable)
    }
}

struct NativePtyControl(Box<dyn MasterPty + Send>);

impl std::fmt::Debug for NativePtyControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativePtyControl")
            .finish_non_exhaustive()
    }
}

impl RuntimePtyControl for NativePtyControl {
    fn resize(&self, dimensions: RuntimeDimensions) -> Result<(), NativeRuntimeError> {
        self.0
            .resize(to_pty_size(dimensions))
            .map_err(pty_unavailable)
    }
}

#[allow(clippy::too_many_lines)]
#[async_trait::async_trait]
impl RuntimeBackend for NativePtyRuntime {
    async fn launch(
        &self,
        request: RuntimeLaunchRequest,
    ) -> Result<RuntimeHandle, NativeRuntimeError> {
        validate_launch(&request)?;
        if self
            .inner
            .attempts
            .read()
            .await
            .contains_key(&request.attempt_id)
        {
            return Err(NativeRuntimeError::Conflict(
                "runtime attempt is already owned by this host".to_string(),
            ));
        }

        let spawned = self
            .inner
            .process_host
            .spawn(&request.command, request.dimensions)?;
        let pid = spawned.pid;

        let state = Arc::new(AttemptState {
            session_id: request.session_id.clone(),
            attempt_id: request.attempt_id.clone(),
            provider: request.provider,
            pid,
            started_at: request.requested_at,
            idle_timeout_ms: request.idle_timeout_ms,
            max_capture_bytes: request.max_capture_bytes,
            max_transcript_bytes: usize::try_from(request.max_transcript_bytes).map_err(|_| {
                NativeRuntimeError::Invalid("transcript bound exceeds this host".to_string())
            })?,
            child: Mutex::new(spawned.child),
            master: StdMutex::new(spawned.controller),
            writer: Mutex::new(spawned.writer),
            dimensions: RwLock::new(request.dimensions),
            mutable: RwLock::new(MutableAttempt {
                status: RuntimeAttemptStatus::Starting,
                output_tail: Vec::new(),
                output_truncated: false,
                native_session_ref: request.native_session_ref.clone(),
                transcript: None,
                exit_code: None,
                last_output_at: request.requested_at,
                last_activity_at: request.requested_at,
                finished_at: None,
                terminal_report: None,
            }),
            transcript_bytes: Mutex::new(Vec::new()),
            transcript_truncated: AtomicBool::new(false),
            event_index: AtomicU64::new(0),
            events: RwLock::new(VecDeque::new()),
            action_digests: Mutex::new(HashMap::new()),
            terminal_started: AtomicBool::new(false),
            output_complete: AtomicBool::new(false),
            output_notify: Notify::new(),
            notify: Notify::new(),
        });
        self.inner
            .attempts
            .write()
            .await
            .insert(request.attempt_id.clone(), Arc::clone(&state));

        if let Err(error) = emit_event(
            &self.inner,
            &state,
            RuntimeEventKind::Starting,
            RuntimeEventPayload::State {
                status: RuntimeAttemptStatus::Starting,
            },
            request.requested_at,
        )
        .await
        {
            let _ = state.child.lock().await.kill();
            self.inner
                .attempts
                .write()
                .await
                .remove(&request.attempt_id);
            return Err(error);
        }
        {
            state.mutable.write().await.status = RuntimeAttemptStatus::Running;
        }
        if let Err(error) = emit_event(
            &self.inner,
            &state,
            RuntimeEventKind::Started,
            RuntimeEventPayload::Process { pid },
            request.requested_at,
        )
        .await
        {
            let _ = state.child.lock().await.kill();
            self.inner
                .attempts
                .write()
                .await
                .remove(&request.attempt_id);
            return Err(error);
        }

        spawn_reader(Arc::clone(&self.inner), Arc::clone(&state), spawned.reader);
        spawn_exit_monitor(Arc::clone(&self.inner), Arc::clone(&state));
        Ok(RuntimeHandle {
            session_id: request.session_id,
            attempt_id: request.attempt_id,
            provider: request.provider,
            pid,
            native_session_ref: request.native_session_ref,
            started_at: request.requested_at,
        })
    }

    async fn send_text(
        &self,
        request: RuntimeTextInput,
    ) -> Result<RuntimeInputAck, NativeRuntimeError> {
        if request.text.is_empty()
            || request.text.len() > MAX_INPUT_BYTES
            || request.text.contains('\0')
            || request.idempotency_key.trim().is_empty()
            || request.observed_at < 0
        {
            return Err(NativeRuntimeError::Invalid(
                "runtime text input is invalid or exceeds bounds".to_string(),
            ));
        }
        let state = self.state(&request.attempt_id).await?;
        ensure_running(&state).await?;
        let mut bytes = request.text.into_bytes();
        if request.submit {
            bytes.push(b'\r');
        }
        accept_input(
            &self.inner,
            &state,
            &request.idempotency_key,
            &bytes,
            request.observed_at,
        )
        .await
    }

    async fn send_key(
        &self,
        request: RuntimeKeyInput,
    ) -> Result<RuntimeInputAck, NativeRuntimeError> {
        send_key_request(
            &self.inner,
            self.state(&request.attempt_id).await?,
            request,
            false,
        )
        .await
    }

    async fn resize(
        &self,
        request: RuntimeResizeRequest,
    ) -> Result<RuntimeSnapshot, NativeRuntimeError> {
        request.dimensions.validate()?;
        if request.idempotency_key.trim().is_empty() || request.observed_at < 0 {
            return Err(NativeRuntimeError::Invalid(
                "runtime resize request is invalid".to_string(),
            ));
        }
        let state = self.state(&request.attempt_id).await?;
        ensure_running(&state).await?;
        let digest = hash_bytes(
            format!(
                "resize:{}:{}:{}:{}",
                request.dimensions.rows,
                request.dimensions.columns,
                request.dimensions.pixel_width,
                request.dimensions.pixel_height
            )
            .as_bytes(),
        );
        if !register_action(&state, &request.idempotency_key, &digest).await? {
            return snapshot_for(&state).await;
        }
        state
            .master
            .lock()
            .map_err(|_| NativeRuntimeError::Unavailable("PTY lock poisoned".to_string()))?
            .resize(request.dimensions)?;
        *state.dimensions.write().await = request.dimensions;
        state.mutable.write().await.last_activity_at = request.observed_at;
        emit_event(
            &self.inner,
            &state,
            RuntimeEventKind::Resized,
            RuntimeEventPayload::Resize {
                dimensions: request.dimensions,
            },
            request.observed_at,
        )
        .await?;
        snapshot_for(&state).await
    }

    async fn interrupt(
        &self,
        request: RuntimeKeyInput,
    ) -> Result<RuntimeInputAck, NativeRuntimeError> {
        if request.key != RuntimeKey::CtrlC {
            return Err(NativeRuntimeError::Invalid(
                "runtime interrupt requires ctrl_c".to_string(),
            ));
        }
        send_key_request(
            &self.inner,
            self.state(&request.attempt_id).await?,
            request,
            true,
        )
        .await
    }

    async fn shutdown(
        &self,
        request: RuntimeShutdownRequest,
    ) -> Result<RuntimeShutdownReport, NativeRuntimeError> {
        if request.idempotency_key.trim().is_empty()
            || request.observed_at < 0
            || request.graceful_timeout_ms > 300_000
            || request.interrupt_timeout_ms > 300_000
        {
            return Err(NativeRuntimeError::Invalid(
                "runtime shutdown request is invalid".to_string(),
            ));
        }
        let state = self.state(&request.attempt_id).await?;
        if let Some(report) = terminal_report(&state).await {
            return Ok(report);
        }
        let digest = hash_bytes(format!("shutdown:{:?}", request.reason).as_bytes());
        if !register_action(&state, &request.idempotency_key, &digest).await? {
            return wait_for_report(
                &state,
                request
                    .graceful_timeout_ms
                    .saturating_add(request.interrupt_timeout_ms)
                    .saturating_add(1_000),
            )
            .await
            .ok_or_else(|| {
                NativeRuntimeError::Unavailable("idempotent shutdown is still pending".to_string())
            });
        }

        write_runtime_bytes(&state, b"\x04").await?;
        if let Some(report) = wait_for_report(&state, request.graceful_timeout_ms).await {
            return Ok(report);
        }
        write_runtime_bytes(&state, b"\x03").await?;
        if let Some(report) = wait_for_report(&state, request.interrupt_timeout_ms).await {
            return Ok(report);
        }
        state.child.lock().await.kill()?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let exit_code = poll_exit_code(&state).await?;
        finish_state(
            &self.inner,
            &state,
            RuntimeShutdownMethod::Kill,
            request.reason,
            exit_code,
            request.observed_at,
        )
        .await
    }

    async fn snapshot(
        &self,
        attempt_id: &RuntimeAttemptId,
    ) -> Result<Option<RuntimeSnapshot>, NativeRuntimeError> {
        let state = self.inner.attempts.read().await.get(attempt_id).cloned();
        match state {
            Some(state) => snapshot_for(&state).await.map(Some),
            None => Ok(None),
        }
    }

    async fn events_after(
        &self,
        attempt_id: &RuntimeAttemptId,
        after_event_index: u64,
        limit: u32,
    ) -> Result<Vec<RuntimeEvent>, NativeRuntimeError> {
        if limit == 0 || limit > 1_000 {
            return Err(NativeRuntimeError::Invalid(
                "runtime event limit must be between 1 and 1000".to_string(),
            ));
        }
        let state = self.state(attempt_id).await?;
        Ok(state
            .events
            .read()
            .await
            .iter()
            .filter(|event| event.event_index > after_event_index)
            .take(limit as usize)
            .cloned()
            .collect())
    }

    async fn wait(
        &self,
        request: RuntimeWaitRequest,
    ) -> Result<RuntimeSnapshot, NativeRuntimeError> {
        if request.timeout_ms > 300_000 {
            return Err(NativeRuntimeError::Invalid(
                "runtime wait timeout exceeds the bound".to_string(),
            ));
        }
        let state = self.state(&request.attempt_id).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(request.timeout_ms);
        loop {
            let snapshot = snapshot_for(&state).await?;
            if snapshot.event_index > request.after_event_index || snapshot.status.is_terminal() {
                return Ok(snapshot);
            }
            if tokio::time::timeout_at(deadline, state.notify.notified())
                .await
                .is_err()
            {
                return snapshot_for(&state).await;
            }
        }
    }

    async fn recover(
        &self,
        request: &RuntimeRecoveryRequest,
    ) -> Result<RuntimeRecoveryDisposition, NativeRuntimeError> {
        if let Some(state) = self
            .inner
            .attempts
            .read()
            .await
            .get(&request.attempt_id)
            .cloned()
        {
            if state.session_id != request.session_id || state.provider != request.provider {
                return Err(NativeRuntimeError::Conflict(
                    "runtime recovery identity does not match live process".to_string(),
                ));
            }
            return Ok(RuntimeRecoveryDisposition::Live {
                snapshot: Box::new(snapshot_for(&state).await?),
            });
        }
        match request.native_session_ref.as_deref() {
            Some(reference)
                if !reference.is_empty()
                    && reference.len() <= 512
                    && reference.chars().all(|character| !character.is_control()) =>
            {
                Ok(RuntimeRecoveryDisposition::Resumable {
                    native_session_ref: reference.to_string(),
                })
            }
            _ => Ok(RuntimeRecoveryDisposition::RuntimeGone),
        }
    }

    async fn reap_idle(
        &self,
        observed_at: i64,
    ) -> Result<Vec<RuntimeShutdownReport>, NativeRuntimeError> {
        if observed_at < 0 {
            return Err(NativeRuntimeError::Invalid(
                "runtime idle reap time must be non-negative".to_string(),
            ));
        }
        let states = self
            .inner
            .attempts
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut reports = Vec::new();
        for state in states {
            let mutable = state.mutable.read().await;
            let idle_seconds = observed_at.saturating_sub(mutable.last_activity_at).max(0);
            let idle_ms = u64::try_from(idle_seconds)
                .unwrap_or(0)
                .saturating_mul(1_000);
            let should_reap =
                mutable.status == RuntimeAttemptStatus::Running && idle_ms >= state.idle_timeout_ms;
            drop(mutable);
            if should_reap {
                reports.push(
                    self.shutdown(RuntimeShutdownRequest {
                        attempt_id: state.attempt_id.clone(),
                        idempotency_key: format!("idle-reap:{observed_at}"),
                        graceful_timeout_ms: 1_000,
                        interrupt_timeout_ms: 1_000,
                        reason: RuntimeTerminalReason::IdleTimeout,
                        observed_at,
                    })
                    .await?,
                );
            }
        }
        Ok(reports)
    }
}

impl NativePtyRuntime {
    async fn state(
        &self,
        attempt_id: &RuntimeAttemptId,
    ) -> Result<Arc<AttemptState>, NativeRuntimeError> {
        self.inner
            .attempts
            .read()
            .await
            .get(attempt_id)
            .cloned()
            .ok_or_else(|| NativeRuntimeError::NotFound(format!("runtime attempt {attempt_id}")))
    }
}

fn validate_launch(request: &RuntimeLaunchRequest) -> Result<(), NativeRuntimeError> {
    request.dimensions.validate()?;
    if request.command.program.trim().is_empty()
        || request.command.program.contains('\0')
        || request
            .command
            .arguments
            .iter()
            .any(|argument| argument.contains('\0'))
        || !request.command.working_directory.is_dir()
        || request.max_capture_bytes == 0
        || request.max_capture_bytes > MAX_CAPTURE_BYTES
        || request.max_transcript_bytes == 0
        || request.max_transcript_bytes > MAX_TRANSCRIPT_BYTES
        || request.idle_timeout_ms == 0
        || request.idle_timeout_ms > 7 * 24 * 60 * 60 * 1_000
        || request.requested_at < 0
        || request.sandbox.expires_at <= request.requested_at
    {
        return Err(NativeRuntimeError::Invalid(
            "native runtime launch request is invalid or exceeds bounds".to_string(),
        ));
    }
    Ok(())
}

fn spawn_reader(
    inner: Arc<RuntimeInner>,
    state: Arc<AttemptState>,
    mut reader: Box<dyn Read + Send>,
) {
    let (sender, mut receiver) = mpsc::channel::<Vec<u8>>(64);
    std::thread::spawn(move || {
        let mut buffer = vec![0_u8; READ_CHUNK_BYTES];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(size) => {
                    if sender.blocking_send(buffer[..size].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    tokio::spawn(async move {
        let mut pending = Vec::new();
        while let Some(raw) = receiver.recv().await {
            pending.extend_from_slice(&raw);
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let record = pending.drain(..=end).collect::<Vec<_>>();
                if redact_and_append(&inner, &state, &record).await.is_err() {
                    fail_closed_output(&inner, &state).await;
                    return;
                }
            }
            if pending.len() > MAX_REDACTION_RECORD_BYTES {
                fail_closed_output(&inner, &state).await;
                return;
            }
        }
        if !pending.is_empty() && redact_and_append(&inner, &state, &pending).await.is_err() {
            fail_closed_output(&inner, &state).await;
            return;
        }
        mark_output_complete(&state);
    });
}

async fn redact_and_append(
    inner: &RuntimeInner,
    state: &AttemptState,
    raw_record: &[u8],
) -> Result<(), NativeRuntimeError> {
    if raw_record.len() > MAX_REDACTION_RECORD_BYTES {
        return Err(NativeRuntimeError::Denied(
            "PTY output record exceeds the redaction bound".to_string(),
        ));
    }
    let redacted = inner
        .redactor
        .redact_content(raw_record)
        .await
        .map_err(|_| {
            NativeRuntimeError::Unavailable("required PTY content redaction failed".to_string())
        })?;
    append_output(inner, state, &redacted, now_unix()).await
}

async fn fail_closed_output(inner: &RuntimeInner, state: &AttemptState) {
    mark_output_complete(state);
    let _ = state.child.lock().await.kill();
    let _ = finish_state(
        inner,
        state,
        RuntimeShutdownMethod::Kill,
        RuntimeTerminalReason::Failed,
        None,
        now_unix(),
    )
    .await;
}

fn mark_output_complete(state: &AttemptState) {
    state.output_complete.store(true, Ordering::Release);
    state.output_notify.notify_waiters();
}

async fn wait_for_output_complete(state: &AttemptState) {
    if state.output_complete.load(Ordering::Acquire) {
        return;
    }
    let notified = state.output_notify.notified();
    if state.output_complete.load(Ordering::Acquire) {
        return;
    }
    let _ = tokio::time::timeout(Duration::from_millis(500), notified).await;
}

fn spawn_exit_monitor(inner: Arc<RuntimeInner>, state: Arc<AttemptState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let Ok(exit_code) = poll_exit_code(&state).await else {
                let _ = finish_state(
                    &inner,
                    &state,
                    RuntimeShutdownMethod::AlreadyExited,
                    RuntimeTerminalReason::Failed,
                    None,
                    now_unix(),
                )
                .await;
                return;
            };
            if let Some(exit_code) = exit_code {
                let reason = if exit_code == 0 {
                    RuntimeTerminalReason::Completed
                } else {
                    RuntimeTerminalReason::Failed
                };
                let _ = finish_state(
                    &inner,
                    &state,
                    RuntimeShutdownMethod::AlreadyExited,
                    reason,
                    Some(exit_code),
                    now_unix(),
                )
                .await;
                return;
            }
            if state.terminal_started.load(Ordering::Acquire) {
                return;
            }
        }
    });
}

async fn append_output(
    inner: &RuntimeInner,
    state: &AttemptState,
    redacted: &[u8],
    observed_at: i64,
) -> Result<(), NativeRuntimeError> {
    if redacted.is_empty() {
        return Ok(());
    }
    let mut native_ref = None;
    {
        let mut mutable = state.mutable.write().await;
        mutable.output_tail.extend_from_slice(redacted);
        if mutable.output_tail.len() > state.max_capture_bytes {
            let overflow = mutable.output_tail.len() - state.max_capture_bytes;
            mutable.output_tail.drain(..overflow);
            mutable.output_truncated = true;
        }
        mutable.last_output_at = observed_at;
        mutable.last_activity_at = observed_at;
        if mutable.native_session_ref.is_none() {
            native_ref = RuntimeProviderAdapter::extract_native_session_ref(
                state.provider,
                &mutable.output_tail,
            );
            mutable.native_session_ref.clone_from(&native_ref);
        }
    }
    {
        let mut transcript = state.transcript_bytes.lock().await;
        let remaining = state.max_transcript_bytes.saturating_sub(transcript.len());
        let accepted = remaining.min(redacted.len());
        transcript.extend_from_slice(&redacted[..accepted]);
        if accepted < redacted.len() {
            state.transcript_truncated.store(true, Ordering::Release);
        }
    }
    emit_event(
        inner,
        state,
        RuntimeEventKind::Output,
        RuntimeEventPayload::Output {
            text: String::from_utf8_lossy(redacted).into_owned(),
            byte_count: redacted.len() as u64,
        },
        observed_at,
    )
    .await?;
    if let Some(reference) = native_ref {
        emit_event(
            inner,
            state,
            RuntimeEventKind::NativeSessionRef,
            RuntimeEventPayload::NativeSession { reference },
            observed_at,
        )
        .await?;
    }
    Ok(())
}

async fn send_key_request(
    inner: &RuntimeInner,
    state: Arc<AttemptState>,
    request: RuntimeKeyInput,
    interrupted: bool,
) -> Result<RuntimeInputAck, NativeRuntimeError> {
    if request.idempotency_key.trim().is_empty()
        || request.repeat == 0
        || request.repeat > 1_000
        || request.observed_at < 0
    {
        return Err(NativeRuntimeError::Invalid(
            "runtime key input is invalid or exceeds bounds".to_string(),
        ));
    }
    ensure_running(&state).await?;
    let sequence = key_bytes(request.key);
    let mut bytes = Vec::with_capacity(sequence.len() * request.repeat as usize);
    for _ in 0..request.repeat {
        bytes.extend_from_slice(sequence);
    }
    let ack = accept_input(
        inner,
        &state,
        &request.idempotency_key,
        &bytes,
        request.observed_at,
    )
    .await?;
    if interrupted {
        emit_event(
            inner,
            &state,
            RuntimeEventKind::Interrupted,
            RuntimeEventPayload::Input {
                idempotency_key: request.idempotency_key.clone(),
                input_sha256: ack.input_sha256.clone(),
                byte_count: bytes.len() as u64,
            },
            request.observed_at,
        )
        .await?;
    }
    Ok(ack)
}

async fn accept_input(
    inner: &RuntimeInner,
    state: &AttemptState,
    idempotency_key: &str,
    bytes: &[u8],
    observed_at: i64,
) -> Result<RuntimeInputAck, NativeRuntimeError> {
    let digest = hash_bytes(bytes);
    if !register_action(state, idempotency_key, &digest).await? {
        return Ok(RuntimeInputAck {
            attempt_id: state.attempt_id.clone(),
            idempotency_key: idempotency_key.to_string(),
            input_sha256: digest,
            accepted_at: observed_at,
        });
    }
    write_runtime_bytes(state, bytes).await?;
    state.mutable.write().await.last_activity_at = observed_at;
    let ack = RuntimeInputAck {
        attempt_id: state.attempt_id.clone(),
        idempotency_key: idempotency_key.to_string(),
        input_sha256: digest.clone(),
        accepted_at: observed_at,
    };
    emit_event(
        inner,
        state,
        RuntimeEventKind::InputAccepted,
        RuntimeEventPayload::Input {
            idempotency_key: idempotency_key.to_string(),
            input_sha256: digest,
            byte_count: bytes.len() as u64,
        },
        observed_at,
    )
    .await?;
    Ok(ack)
}

async fn register_action(
    state: &AttemptState,
    idempotency_key: &str,
    digest: &str,
) -> Result<bool, NativeRuntimeError> {
    let mut actions = state.action_digests.lock().await;
    match actions.get(idempotency_key) {
        Some(existing) if existing == digest => Ok(false),
        Some(_) => Err(NativeRuntimeError::Conflict(
            "runtime idempotency key was reused for different input".to_string(),
        )),
        None => {
            actions.insert(idempotency_key.to_string(), digest.to_string());
            Ok(true)
        }
    }
}

async fn write_runtime_bytes(state: &AttemptState, bytes: &[u8]) -> Result<(), NativeRuntimeError> {
    let mut writer = state.writer.lock().await;
    writer.write_all(bytes).map_err(pty_unavailable)?;
    writer.flush().map_err(pty_unavailable)
}

async fn ensure_running(state: &AttemptState) -> Result<(), NativeRuntimeError> {
    if state.mutable.read().await.status == RuntimeAttemptStatus::Running {
        Ok(())
    } else {
        Err(NativeRuntimeError::Conflict(
            "runtime attempt is not running".to_string(),
        ))
    }
}

async fn emit_event(
    inner: &RuntimeInner,
    state: &AttemptState,
    kind: RuntimeEventKind,
    payload: RuntimeEventPayload,
    occurred_at: i64,
) -> Result<RuntimeEvent, NativeRuntimeError> {
    let payload_bytes = serde_json::to_vec(&payload).map_err(|error| {
        NativeRuntimeError::Invalid(format!("runtime event payload is invalid: {error}"))
    })?;
    let event = RuntimeEvent {
        id: RuntimeEventId::new(),
        session_id: state.session_id.clone(),
        attempt_id: state.attempt_id.clone(),
        event_index: state.event_index.fetch_add(1, Ordering::AcqRel) + 1,
        kind,
        payload,
        payload_sha256: hash_bytes(&payload_bytes),
        occurred_at,
    };
    let event = inner.event_port.append_runtime_event(&event).await?;
    state
        .event_index
        .fetch_max(event.event_index, Ordering::AcqRel);
    let mut events = state.events.write().await;
    events.push_back(event.clone());
    while events.len() > MAX_LOCAL_EVENTS {
        events.pop_front();
    }
    drop(events);
    state.notify.notify_waiters();
    Ok(event)
}

async fn finish_state(
    inner: &RuntimeInner,
    state: &AttemptState,
    method: RuntimeShutdownMethod,
    reason: RuntimeTerminalReason,
    exit_code: Option<i32>,
    observed_at: i64,
) -> Result<RuntimeShutdownReport, NativeRuntimeError> {
    if state
        .terminal_started
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return wait_for_report(state, 5_000).await.ok_or_else(|| {
            NativeRuntimeError::Unavailable("runtime terminal transition is pending".to_string())
        });
    }
    wait_for_output_complete(state).await;
    let transcript_bytes = state.transcript_bytes.lock().await.clone();
    let transcript = inner
        .archive
        .archive_runtime_transcript(
            &state.session_id,
            &state.attempt_id,
            &transcript_bytes,
            state.transcript_truncated.load(Ordering::Acquire),
            observed_at,
        )
        .await?;
    emit_event(
        inner,
        state,
        RuntimeEventKind::TranscriptArchived,
        RuntimeEventPayload::Transcript {
            reference: transcript.clone(),
        },
        observed_at,
    )
    .await?;
    emit_event(
        inner,
        state,
        RuntimeEventKind::Exited,
        RuntimeEventPayload::Exit { exit_code },
        observed_at,
    )
    .await?;
    let report = RuntimeShutdownReport {
        session_id: state.session_id.clone(),
        attempt_id: state.attempt_id.clone(),
        method,
        terminal_reason: reason,
        exit_code,
        transcript: transcript.clone(),
        finished_at: observed_at,
    };
    {
        let mut mutable = state.mutable.write().await;
        mutable.status = RuntimeAttemptStatus::Exited;
        mutable.transcript = Some(transcript);
        mutable.exit_code = exit_code;
        mutable.finished_at = Some(observed_at);
        mutable.terminal_report = Some(report.clone());
    }
    emit_event(
        inner,
        state,
        RuntimeEventKind::Shutdown,
        RuntimeEventPayload::Terminal { reason, method },
        observed_at,
    )
    .await?;
    state.notify.notify_waiters();
    Ok(report)
}

async fn snapshot_for(state: &AttemptState) -> Result<RuntimeSnapshot, NativeRuntimeError> {
    let dimensions = *state.dimensions.read().await;
    let mutable = state.mutable.read().await;
    Ok(RuntimeSnapshot {
        session_id: state.session_id.clone(),
        attempt_id: state.attempt_id.clone(),
        provider: state.provider,
        status: mutable.status,
        pid: Some(state.pid),
        dimensions,
        event_index: state.event_index.load(Ordering::Acquire),
        output_tail: String::from_utf8_lossy(&mutable.output_tail).into_owned(),
        output_truncated: mutable.output_truncated,
        native_session_ref: mutable.native_session_ref.clone(),
        transcript: mutable.transcript.clone(),
        exit_code: mutable.exit_code,
        started_at: state.started_at,
        last_output_at: mutable.last_output_at,
        finished_at: mutable.finished_at,
    })
}

async fn terminal_report(state: &AttemptState) -> Option<RuntimeShutdownReport> {
    state.mutable.read().await.terminal_report.clone()
}

async fn wait_for_report(state: &AttemptState, timeout_ms: u64) -> Option<RuntimeShutdownReport> {
    if let Some(report) = terminal_report(state).await {
        return Some(report);
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if tokio::time::timeout_at(deadline, state.notify.notified())
            .await
            .is_err()
        {
            return terminal_report(state).await;
        }
        if let Some(report) = terminal_report(state).await {
            return Some(report);
        }
    }
}

async fn poll_exit_code(state: &AttemptState) -> Result<Option<i32>, NativeRuntimeError> {
    state.child.lock().await.try_wait()
}

fn key_bytes(key: RuntimeKey) -> &'static [u8] {
    match key {
        RuntimeKey::Enter => b"\r",
        RuntimeKey::Tab => b"\t",
        RuntimeKey::Escape => b"\x1b",
        RuntimeKey::Backspace => b"\x7f",
        RuntimeKey::ArrowUp => b"\x1b[A",
        RuntimeKey::ArrowDown => b"\x1b[B",
        RuntimeKey::ArrowLeft => b"\x1b[D",
        RuntimeKey::ArrowRight => b"\x1b[C",
        RuntimeKey::CtrlC => b"\x03",
        RuntimeKey::CtrlD => b"\x04",
    }
}

const fn to_pty_size(dimensions: RuntimeDimensions) -> PtySize {
    PtySize {
        rows: dimensions.rows,
        cols: dimensions.columns,
        pixel_width: dimensions.pixel_width,
        pixel_height: dimensions.pixel_height,
    }
}

fn hash_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs()).unwrap_or(i64::MAX)
        })
}

fn pty_unavailable(error: impl std::fmt::Display) -> NativeRuntimeError {
    NativeRuntimeError::Unavailable(format!("native PTY unavailable: {error}"))
}
