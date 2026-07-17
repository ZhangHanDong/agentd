use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentd_core::ports::{
    ContentRedactionPort, NativeRuntimeError, RuntimeArchivePort, RuntimeBackend,
    RuntimeDimensions, RuntimeEvent, RuntimeEventPort, RuntimeLaunchRequest, RuntimeProvider,
    RuntimeSandboxRef,
};
use agentd_core::types::{RuntimeAttemptId, RuntimeSessionId, WorkerIncarnationId};
use agentd_runtime::{ContentAddressedTranscriptStore, NativePtyRuntime};

const ENABLE_ENV: &str = "AGENTD_REAL_NATIVE_RUNTIME_SMOKE";
const EXPECTED_OUTPUT: &str = "AGENTD_NATIVE_RUNTIME_OK";

#[derive(Debug)]
struct SmokeRedactor;

#[async_trait::async_trait]
impl ContentRedactionPort for SmokeRedactor {
    async fn redact_content(
        &self,
        content: &[u8],
    ) -> Result<Vec<u8>, agentd_core::ports::SecurityError> {
        Ok(String::from_utf8_lossy(content)
            .replace("AGENTD_SMOKE_CANARY_SECRET", "[REDACTED]")
            .into_bytes())
    }
}

#[derive(Debug, Default)]
struct SmokeEventPort {
    events: Mutex<Vec<RuntimeEvent>>,
}

#[async_trait::async_trait]
impl RuntimeEventPort for SmokeEventPort {
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

#[tokio::test]
async fn real_codex_runs_through_native_pty_and_archives_transcript() {
    if std::env::var(ENABLE_ENV).as_deref() != Ok("1") {
        return;
    }
    assert_eq!(
        std::env::var("AGENTD_NATIVE_RUNTIME_SMOKE_PROVIDER")
            .unwrap_or_else(|_| "codex".to_string()),
        "codex",
        "the native runtime smoke is restricted to Codex"
    );

    let archive_directory = tempfile::tempdir().expect("temporary transcript archive");
    let archive = Arc::new(
        ContentAddressedTranscriptStore::new(archive_directory.path(), 16 * 1024 * 1024)
            .expect("transcript archive"),
    );
    let archive_port: Arc<dyn RuntimeArchivePort> = archive.clone();
    let runtime = NativePtyRuntime::new(
        Arc::new(SmokeRedactor),
        archive_port,
        Arc::new(SmokeEventPort::default()),
    );
    let session_id = RuntimeSessionId::new();
    let attempt_id = RuntimeAttemptId::new();
    let observed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_secs()
        .try_into()
        .unwrap_or(i64::MAX);
    let command = std::env::var("AGENTD_CODEX_BIN").unwrap_or_else(|_| "codex".to_string());
    let working_directory = std::env::var_os("AGENTD_NATIVE_RUNTIME_SMOKE_CWD")
        .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);

    runtime
        .launch(RuntimeLaunchRequest {
            session_id,
            attempt_id: attempt_id.clone(),
            worker_incarnation_id: WorkerIncarnationId::new(),
            provider: RuntimeProvider::Codex,
            command: agentd_core::ports::RuntimeCommand {
                program: command,
                arguments: vec![
                    "exec".to_string(),
                    "--json".to_string(),
                    "--sandbox".to_string(),
                    "read-only".to_string(),
                    "--skip-git-repo-check".to_string(),
                    format!("Reply with exactly {EXPECTED_OUTPUT}. Do not use tools."),
                ],
                environment: BTreeMap::new(),
                working_directory,
            },
            dimensions: RuntimeDimensions {
                rows: 40,
                columns: 120,
                pixel_width: 0,
                pixel_height: 0,
            },
            sandbox: RuntimeSandboxRef {
                sandbox_id: "sb_real_codex_smoke".to_string(),
                profile_sha256: "0".repeat(64),
                expires_at: observed_at + 300,
            },
            native_session_ref: None,
            max_capture_bytes: 2 * 1024 * 1024,
            max_transcript_bytes: 16 * 1024 * 1024,
            idle_timeout_ms: 180_000,
            requested_at: observed_at,
        })
        .await
        .expect("launch Codex through native PTY");

    let timeout_seconds = std::env::var("AGENTD_NATIVE_RUNTIME_SMOKE_TIMEOUT_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(180);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_seconds);
    let snapshot = loop {
        let snapshot = runtime
            .snapshot(&attempt_id)
            .await
            .expect("runtime snapshot")
            .expect("live runtime attempt");
        if snapshot.status.is_terminal() && snapshot.transcript.is_some() {
            break snapshot;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "Codex native runtime smoke timed out"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    assert_eq!(snapshot.exit_code, Some(0));
    assert!(snapshot.output_tail.contains(EXPECTED_OUTPUT));
    assert!(snapshot.native_session_ref.is_some());
    let transcript = snapshot.transcript.expect("archived transcript");
    let transcript_bytes = std::fs::read(archive.object_path(&transcript.content_sha256))
        .expect("read archived transcript");
    assert!(
        String::from_utf8_lossy(&transcript_bytes).contains(EXPECTED_OUTPUT),
        "archived transcript does not contain the Codex response"
    );
}
