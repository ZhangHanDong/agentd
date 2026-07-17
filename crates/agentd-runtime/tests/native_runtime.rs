use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use agentd_core::ports::{
    ContentRedactionPort, NativeRuntimeError, RuntimeArchivePort, RuntimeBackend,
    RuntimeDimensions, RuntimeEvent, RuntimeEventPort, RuntimeLaunchRequest, RuntimeProvider,
    RuntimeRecoveryDisposition, RuntimeRecoveryRequest, RuntimeSandboxRef, RuntimeShutdownRequest,
    RuntimeTerminalReason, RuntimeTextInput,
};
use agentd_core::types::{RuntimeAttemptId, RuntimeSessionId, WorkerIncarnationId};
use agentd_runtime::{
    ContentAddressedTranscriptStore, NativePtyRuntime, RuntimeChildControl, RuntimeProcessHost,
    RuntimePtyControl, SpawnedRuntimeProcess,
};

#[derive(Debug)]
struct FakeRedactor;

#[async_trait::async_trait]
impl ContentRedactionPort for FakeRedactor {
    async fn redact_content(
        &self,
        content: &[u8],
    ) -> Result<Vec<u8>, agentd_core::ports::SecurityError> {
        Ok(String::from_utf8_lossy(content)
            .replace("secret", "[REDACTED]")
            .into_bytes())
    }
}

#[derive(Debug, Default)]
struct FakeEventPort {
    events: Mutex<Vec<RuntimeEvent>>,
}

#[async_trait::async_trait]
impl RuntimeEventPort for FakeEventPort {
    async fn append_runtime_event(
        &self,
        event: &RuntimeEvent,
    ) -> Result<RuntimeEvent, NativeRuntimeError> {
        self.events.lock().expect("event lock").push(event.clone());
        Ok(event.clone())
    }

    async fn runtime_events_after(
        &self,
        session_id: &RuntimeSessionId,
        after_event_index: u64,
        limit: u32,
    ) -> Result<Vec<RuntimeEvent>, NativeRuntimeError> {
        Ok(self
            .events
            .lock()
            .expect("event lock")
            .iter()
            .filter(|event| {
                event.session_id == *session_id && event.event_index > after_event_index
            })
            .take(limit as usize)
            .cloned()
            .collect())
    }
}

#[derive(Debug)]
struct FakeProcessHost {
    exit_code: Arc<AtomicI32>,
    writes: Arc<Mutex<Vec<u8>>>,
    dimensions: Arc<Mutex<RuntimeDimensions>>,
}

impl RuntimeProcessHost for FakeProcessHost {
    fn spawn(
        &self,
        _command: &agentd_core::ports::RuntimeCommand,
        dimensions: RuntimeDimensions,
    ) -> Result<SpawnedRuntimeProcess, NativeRuntimeError> {
        *self.dimensions.lock().expect("dimension lock") = dimensions;
        Ok(SpawnedRuntimeProcess {
            pid: 4242,
            child: Box::new(FakeChild(Arc::clone(&self.exit_code))),
            controller: Box::new(FakeController(Arc::clone(&self.dimensions))),
            reader: Box::new(ChunkedReader::new([
                b"{\"thread_id\":\"codex-session-1\"}\n".to_vec(),
                b"sec".to_vec(),
                b"ret\n".to_vec(),
            ])),
            writer: Box::new(FakeWriter(Arc::clone(&self.writes))),
        })
    }
}

#[derive(Debug)]
struct ChunkedReader {
    chunks: VecDeque<Vec<u8>>,
}

impl ChunkedReader {
    fn new(chunks: impl IntoIterator<Item = Vec<u8>>) -> Self {
        Self {
            chunks: chunks.into_iter().collect(),
        }
    }
}

impl Read for ChunkedReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let Some(chunk) = self.chunks.pop_front() else {
            return Ok(0);
        };
        assert!(
            chunk.len() <= buffer.len(),
            "fake read chunk exceeds buffer"
        );
        buffer[..chunk.len()].copy_from_slice(&chunk);
        Ok(chunk.len())
    }
}

#[derive(Debug)]
struct FakeChild(Arc<AtomicI32>);

impl RuntimeChildControl for FakeChild {
    fn try_wait(&mut self) -> Result<Option<i32>, NativeRuntimeError> {
        let code = self.0.load(Ordering::Acquire);
        Ok((code >= 0).then_some(code))
    }

    fn kill(&mut self) -> Result<(), NativeRuntimeError> {
        self.0.store(137, Ordering::Release);
        Ok(())
    }
}

#[derive(Debug)]
struct FakeController(Arc<Mutex<RuntimeDimensions>>);

impl RuntimePtyControl for FakeController {
    fn resize(&self, dimensions: RuntimeDimensions) -> Result<(), NativeRuntimeError> {
        *self.0.lock().expect("dimension lock") = dimensions;
        Ok(())
    }
}

#[derive(Debug)]
struct FakeWriter(Arc<Mutex<Vec<u8>>>);

impl Write for FakeWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.expect_lock().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

trait ExpectLock<T> {
    fn expect_lock(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> ExpectLock<T> for Mutex<T> {
    fn expect_lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().expect("fake lock")
    }
}

#[tokio::test]
async fn fake_pty_proves_redacted_lifecycle_input_and_archive() {
    let temporary = tempfile::tempdir().expect("temporary archive");
    let archive: Arc<dyn RuntimeArchivePort> = Arc::new(
        ContentAddressedTranscriptStore::new(temporary.path(), 1024 * 1024).expect("archive store"),
    );
    let event_port = Arc::new(FakeEventPort::default());
    let writes = Arc::new(Mutex::new(Vec::new()));
    let dimensions = Arc::new(Mutex::new(RuntimeDimensions {
        rows: 24,
        columns: 80,
        pixel_width: 0,
        pixel_height: 0,
    }));
    let process_host = Arc::new(FakeProcessHost {
        exit_code: Arc::new(AtomicI32::new(-1)),
        writes: Arc::clone(&writes),
        dimensions,
    });
    let runtime = NativePtyRuntime::with_process_host(
        Arc::new(FakeRedactor),
        archive,
        event_port,
        process_host,
    );
    let session_id = RuntimeSessionId::new();
    let attempt_id = RuntimeAttemptId::new();
    let handle = runtime
        .launch(RuntimeLaunchRequest {
            session_id: session_id.clone(),
            attempt_id: attempt_id.clone(),
            worker_incarnation_id: WorkerIncarnationId::new(),
            provider: RuntimeProvider::Codex,
            command: agentd_core::ports::RuntimeCommand {
                program: "fake-codex".to_string(),
                arguments: Vec::new(),
                environment: BTreeMap::new(),
                working_directory: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            },
            dimensions: RuntimeDimensions {
                rows: 24,
                columns: 80,
                pixel_width: 0,
                pixel_height: 0,
            },
            sandbox: RuntimeSandboxRef {
                sandbox_id: "sb_fake".to_string(),
                profile_sha256: "a".repeat(64),
                expires_at: 10_000,
            },
            native_session_ref: None,
            max_capture_bytes: 64 * 1024,
            max_transcript_bytes: 1024 * 1024,
            idle_timeout_ms: 60_000,
            requested_at: 1,
        })
        .await
        .expect("launch");
    assert_eq!(handle.pid, 4242);

    let mut snapshot = runtime
        .snapshot(&attempt_id)
        .await
        .expect("snapshot")
        .expect("live attempt");
    for _ in 0..20 {
        if snapshot.native_session_ref.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        snapshot = runtime
            .snapshot(&attempt_id)
            .await
            .expect("snapshot")
            .expect("live attempt");
    }
    assert_eq!(
        snapshot.native_session_ref.as_deref(),
        Some("codex-session-1")
    );
    assert!(snapshot.output_tail.contains("[REDACTED]"));
    assert!(!snapshot.output_tail.contains("secret"));

    runtime
        .send_text(RuntimeTextInput {
            attempt_id: attempt_id.clone(),
            idempotency_key: "prompt-1".to_string(),
            text: "finish the task".to_string(),
            submit: true,
            observed_at: 2,
        })
        .await
        .expect("input");
    assert!(writes.expect_lock().ends_with(b"finish the task\r"));

    let report = runtime
        .shutdown(RuntimeShutdownRequest {
            attempt_id,
            idempotency_key: "stop-1".to_string(),
            graceful_timeout_ms: 0,
            interrupt_timeout_ms: 0,
            reason: RuntimeTerminalReason::Cancelled,
            observed_at: 3,
        })
        .await
        .expect("shutdown");
    assert_eq!(report.session_id, session_id);
    assert_eq!(report.transcript.storage_ref.len(), 71);
}

#[tokio::test]
async fn recovery_distinguishes_resumable_from_runtime_gone() {
    let temporary = tempfile::tempdir().expect("temporary archive");
    let runtime = NativePtyRuntime::with_process_host(
        Arc::new(FakeRedactor),
        Arc::new(
            ContentAddressedTranscriptStore::new(temporary.path(), 1024).expect("archive store"),
        ),
        Arc::new(FakeEventPort::default()),
        Arc::new(FakeProcessHost {
            exit_code: Arc::new(AtomicI32::new(-1)),
            writes: Arc::new(Mutex::new(Vec::new())),
            dimensions: Arc::new(Mutex::new(RuntimeDimensions {
                rows: 24,
                columns: 80,
                pixel_width: 0,
                pixel_height: 0,
            })),
        }),
    );
    let base = RuntimeRecoveryRequest {
        session_id: RuntimeSessionId::new(),
        attempt_id: RuntimeAttemptId::new(),
        provider: RuntimeProvider::Codex,
        pid: None,
        native_session_ref: Some("codex-session-1".to_string()),
        observed_at: 1,
    };
    assert!(matches!(
        runtime.recover(&base).await.expect("recover"),
        RuntimeRecoveryDisposition::Resumable { .. }
    ));
    assert_eq!(
        runtime
            .recover(&RuntimeRecoveryRequest {
                native_session_ref: None,
                ..base
            })
            .await
            .expect("recover gone"),
        RuntimeRecoveryDisposition::RuntimeGone
    );
}
