use std::time::Duration;

use agentd_bin::native_worker::{
    AgentdWorker, NativeRecoveryRegistry, NativeRecoveryRequest, codex_resume_config,
};
use agentd_core::types::{
    AgentProfileId, NodeId, RunId, RuntimeSessionId, RuntimeSessionStatus, WorkerId,
    WorkerIncarnationId,
};
use agentd_store::agent_profile_repo::{self, AgentProfileCreate};
use agentd_store::runtime_session_repo::{self, ExecutionSnapshotRef, RuntimeSessionCreate};
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use agentd_store::{SqliteStore, run_repo, task_repo};
use agentd_tmux::native::NativeProcessConfig;

#[test]
fn codex_resume_config_uses_persisted_thread_reference() {
    let config = codex_resume_config(NativeProcessConfig::default(), "thread-42".into());
    assert_eq!(config.args, ["exec", "resume", "thread-42"]);
    assert_eq!(config.native_session_ref.as_deref(), Some("thread-42"));
}

#[test]
fn native_recovery_registry_rejects_empty_provider_command() {
    let registry = NativeRecoveryRegistry::new();
    let error = registry
        .register(NativeRecoveryRequest {
            session_id: RuntimeSessionId::new(),
            worker_incarnation_id: WorkerIncarnationId::new(),
            config: NativeProcessConfig::default(),
        })
        .expect_err("empty provider command");
    assert!(error.to_string().contains("provider program is required"));
}

#[test]
fn native_recovery_registry_rejects_arbitrary_shell_command() {
    let registry = NativeRecoveryRegistry::new();
    let error = registry
        .register(NativeRecoveryRequest {
            session_id: RuntimeSessionId::new(),
            worker_incarnation_id: WorkerIncarnationId::new(),
            config: NativeProcessConfig {
                program: "sh".into(),
                ..NativeProcessConfig::default()
            },
        })
        .expect_err("shell command");
    assert!(
        error
            .to_string()
            .contains("unsupported provider executable")
    );
}
use serde_json::json;

#[tokio::test]
async fn agentd_worker_binds_native_process_to_durable_runtime_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
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
            role: "implementer".into(),
            capability: Some("implementation".into()),
            runtime: "codex".into(),
            model: None,
            prompt_profile: None,
        },
    )
    .await
    .expect("profile");
    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "local".into(),
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
            daemon_version: "test".into(),
            host_name: "test-host".into(),
            network_zone: None,
            capabilities: json!({"runtime": ["native"]}),
        },
    )
    .await
    .expect("incarnation");
    let session_id = RuntimeSessionId::new();
    runtime_session_repo::create_session(
        store.pool(),
        RuntimeSessionCreate {
            id: session_id.clone(),
            execution_task_id: task_id,
            agent_profile_id: profile_id,
            snapshot: ExecutionSnapshotRef {
                authority_key: "local".into(),
                resource_kind: "execution_snapshot".into(),
                resource_id: "snapshot".into(),
                resource_version: "1".into(),
                content_sha256: "a".repeat(64),
            },
        },
    )
    .await
    .expect("session");

    let worker = AgentdWorker::new(store.clone());
    let handle = worker
        .start(
            session_id.clone(),
            incarnation_id,
            NativeProcessConfig {
                program: "sh".into(),
                args: vec!["-c".into(), "printf 'native-worker\\n'; exit 0".into()],
                ..NativeProcessConfig::default()
            },
        )
        .await
        .expect("start native worker");
    let event = handle.wait(Duration::from_secs(5)).await.expect("wait");
    assert!(matches!(
        event,
        agentd_tmux::native::NativeProcessEvent::Exited { code: Some(0), .. }
    ));
    assert_eq!(
        runtime_session_repo::get_session(store.pool(), &session_id)
            .await
            .expect("session")
            .expect("exists")
            .status,
        RuntimeSessionStatus::Completed
    );
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn agentd_worker_resume_reuses_persisted_native_session_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
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
            role: "implementer".into(),
            capability: None,
            runtime: "codex".into(),
            model: None,
            prompt_profile: None,
        },
    )
    .await
    .expect("profile");
    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "local".into(),
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
            daemon_version: "test".into(),
            host_name: "test-host".into(),
            network_zone: None,
            capabilities: json!({"runtime": ["native"]}),
        },
    )
    .await
    .expect("incarnation");
    let session_id = RuntimeSessionId::new();
    runtime_session_repo::create_session(
        store.pool(),
        RuntimeSessionCreate {
            id: session_id.clone(),
            execution_task_id: task_id,
            agent_profile_id: profile_id,
            snapshot: ExecutionSnapshotRef {
                authority_key: "local".into(),
                resource_kind: "execution_snapshot".into(),
                resource_id: "snapshot".into(),
                resource_version: "1".into(),
                content_sha256: "b".repeat(64),
            },
        },
    )
    .await
    .expect("session");

    let worker = AgentdWorker::new(store.clone());
    let first = worker
        .start(
            session_id.clone(),
            incarnation_id.clone(),
            NativeProcessConfig {
                program: "sh".into(),
                args: vec!["-c".into(), "sleep 2".into()],
                native_session_ref: Some("codex-thread-42".into()),
                ..NativeProcessConfig::default()
            },
        )
        .await
        .expect("first start");
    runtime_session_repo::mark_attempt_gone(store.pool(), &session_id, first.attempt_id())
        .await
        .expect("mark lost");

    let resumed = worker
        .resume(
            session_id.clone(),
            incarnation_id,
            NativeProcessConfig {
                program: "sh".into(),
                args: vec!["-c".into(), "exit 0".into()],
                ..NativeProcessConfig::default()
            },
        )
        .await
        .expect("resume");
    assert_eq!(
        resumed.native_session_ref().as_deref(),
        Some("codex-thread-42")
    );
    resumed.wait(Duration::from_secs(5)).await.expect("wait");
    assert_eq!(
        runtime_session_repo::get_session(store.pool(), &session_id)
            .await
            .expect("session")
            .expect("exists")
            .status,
        RuntimeSessionStatus::Completed
    );
}
