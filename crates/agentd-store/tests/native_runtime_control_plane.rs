use agentd_core::ports::{
    RuntimeEvent, RuntimeEventKind, RuntimeEventPayload, RuntimeEventPort, RuntimeHandle,
    RuntimeLedgerPort, RuntimeProvider, RuntimeSandboxRef, RuntimeSessionRegistration,
    RuntimeShutdownMethod, RuntimeShutdownReport, RuntimeTerminalReason, RuntimeTranscriptRef,
};
use agentd_core::types::{
    AgentProfileId, AuthorityKey, NodeId, ProjectExecutionSnapshotRef, RunId, RuntimeAttemptId,
    RuntimeEventId, RuntimeSessionId, RuntimeSessionStatus, RuntimeTranscriptId, TaskRunId,
    WorkerId, WorkerIncarnationId,
};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteNativeRuntimeControlPlane, SqliteStore, run_repo, task_repo};
use serde_json::json;
use sha2::{Digest, Sha256};

struct Fixture {
    _directory: tempfile::TempDir,
    control_plane: SqliteNativeRuntimeControlPlane,
    task_id: TaskRunId,
    profile_id: AgentProfileId,
    incarnation_id: WorkerIncarnationId,
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("temporary database");
    let store = SqliteStore::connect(&directory.path().join("agentd.db"))
        .await
        .expect("connect");
    let run_id = RunId::new();
    run_repo::insert_run(store.pool(), &run_id, "workflow-sha")
        .await
        .expect("run");
    let task_id = task_repo::insert_task_run(store.pool(), &run_id, &NodeId::parsed("impl"))
        .await
        .expect("task");
    let profile_id = AgentProfileId::new();
    agent_profile_repo::create_profile(
        store.pool(),
        AgentProfileCreate {
            id: profile_id.clone(),
            role: "implementer".to_string(),
            capability: Some("implementation".to_string()),
            runtime: "codex".to_string(),
            model: Some("gpt-5".to_string()),
            prompt_profile: Some("default".to_string()),
        },
    )
    .await
    .expect("profile");
    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "corp-coding".to_string(),
            labels: json!({}),
        },
    )
    .await
    .expect("worker");
    let incarnation_id = WorkerIncarnationId::new();
    worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        WorkerRegistration {
            id: incarnation_id.clone(),
            daemon_version: "0.0.0-ad-e5".to_string(),
            host_name: "runtime-host-a".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
        },
    )
    .await
    .expect("incarnation");
    Fixture {
        _directory: directory,
        control_plane: SqliteNativeRuntimeControlPlane::new(store.pool().clone()),
        task_id,
        profile_id,
        incarnation_id,
    }
}

fn registration(fixture: &Fixture, session_id: RuntimeSessionId) -> RuntimeSessionRegistration {
    RuntimeSessionRegistration {
        session_id,
        execution_task_id: fixture.task_id.clone(),
        agent_profile_id: fixture.profile_id.clone(),
        snapshot_ref: ProjectExecutionSnapshotRef::new(
            AuthorityKey::new("specify:corp").expect("authority"),
            "snapshot-42",
            "7",
        )
        .expect("snapshot"),
        snapshot_content_sha256: "a".repeat(64),
        provider: RuntimeProvider::Codex,
        command_sha256: "b".repeat(64),
        sandbox: RuntimeSandboxRef {
            sandbox_id: "sb_native".to_string(),
            profile_sha256: "c".repeat(64),
            expires_at: 100,
        },
        max_capture_bytes: 64 * 1024,
        max_transcript_bytes: 1024 * 1024,
        idle_timeout_ms: 60_000,
        created_at: 10,
    }
}

#[tokio::test]
async fn native_runtime_ledger_keeps_session_identity_global_events_and_transcript() {
    let fixture = fixture().await;
    let session_id = RuntimeSessionId::new();
    let attempt_id = RuntimeAttemptId::new();
    let created = fixture
        .control_plane
        .register_runtime_session(&registration(&fixture, session_id.clone()))
        .await
        .expect("register");
    assert_eq!(created.status, RuntimeSessionStatus::Requested);
    fixture
        .control_plane
        .begin_runtime_attempt(
            &session_id,
            &attempt_id,
            &fixture.incarnation_id,
            "host-a/boot-1",
            11,
        )
        .await
        .expect("attempt");
    fixture
        .control_plane
        .mark_runtime_attempt_running(&RuntimeHandle {
            session_id: session_id.clone(),
            attempt_id: attempt_id.clone(),
            provider: RuntimeProvider::Codex,
            pid: 4242,
            native_session_ref: None,
            started_at: 11,
        })
        .await
        .expect("running");

    let input_payload = RuntimeEventPayload::Input {
        idempotency_key: "prompt-1".to_string(),
        input_sha256: hex::encode(Sha256::digest(b"prompt")),
        byte_count: 6,
    };
    let first = fixture
        .control_plane
        .append_runtime_event(&event(
            &session_id,
            &attempt_id,
            RuntimeEventKind::InputAccepted,
            input_payload,
            12,
        ))
        .await
        .expect("input event");
    let second = fixture
        .control_plane
        .append_runtime_event(&event(
            &session_id,
            &attempt_id,
            RuntimeEventKind::Output,
            RuntimeEventPayload::Output {
                text: "redacted output".to_string(),
                byte_count: 15,
            },
            13,
        ))
        .await
        .expect("output event");
    assert_eq!(first.event_index, 1);
    assert_eq!(second.event_index, 2);

    let transcript = RuntimeTranscriptRef {
        id: RuntimeTranscriptId::new(),
        content_sha256: hex::encode(Sha256::digest(b"redacted output")),
        storage_ref: format!("sha256:{}", hex::encode(Sha256::digest(b"redacted output"))),
        size_bytes: 15,
        truncated: false,
        archived_at: 14,
    };
    let terminal = fixture
        .control_plane
        .finish_runtime_attempt(&RuntimeShutdownReport {
            session_id: session_id.clone(),
            attempt_id,
            method: RuntimeShutdownMethod::AlreadyExited,
            terminal_reason: RuntimeTerminalReason::Completed,
            exit_code: Some(0),
            transcript: transcript.clone(),
            finished_at: 14,
        })
        .await
        .expect("finish");
    assert_eq!(terminal.status, RuntimeSessionStatus::Completed);
    assert_eq!(terminal.transcript, Some(transcript));
    assert_eq!(
        fixture
            .control_plane
            .runtime_events_after(&session_id, 0, 100)
            .await
            .expect("events")
            .len(),
        2
    );
}

fn event(
    session_id: &RuntimeSessionId,
    attempt_id: &RuntimeAttemptId,
    kind: RuntimeEventKind,
    payload: RuntimeEventPayload,
    occurred_at: i64,
) -> RuntimeEvent {
    let payload_sha256 = hex::encode(Sha256::digest(
        serde_json::to_vec(&payload).expect("payload"),
    ));
    RuntimeEvent {
        id: RuntimeEventId::new(),
        session_id: session_id.clone(),
        attempt_id: attempt_id.clone(),
        event_index: 1,
        kind,
        payload,
        payload_sha256,
        occurred_at,
    }
}
