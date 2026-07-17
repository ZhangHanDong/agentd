use agentd_store::SqliteStore;
use agentd_store::native_agent_binding::{
    ensure_native_agent_profile, ensure_native_execution_task, ensure_native_runtime_authority,
};

#[tokio::test]
async fn local_native_authority_is_stable_and_system_tasks_are_explicit() {
    let directory = tempfile::tempdir().expect("native authority directory");
    let store = SqliteStore::connect(directory.path().join("agentd.db"))
        .await
        .expect("store");
    let first = ensure_native_runtime_authority(store.pool(), "host-one")
        .await
        .expect("first authority");
    let second = ensure_native_runtime_authority(store.pool(), "host-two")
        .await
        .expect("reused authority");
    assert_eq!(first.worker_id, second.worker_id);
    assert_eq!(first.worker_incarnation_id, second.worker_incarnation_id);

    let first_profile = ensure_native_agent_profile(store.pool(), "codex-worker", "codex")
        .await
        .expect("profile");
    let second_profile = ensure_native_agent_profile(store.pool(), "codex-worker", "codex")
        .await
        .expect("profile replay");
    assert_eq!(first_profile, second_profile);

    let (task, synthetic) = ensure_native_execution_task(store.pool(), None)
        .await
        .expect("system task");
    assert!(synthetic);
    let (same, synthetic) = ensure_native_execution_task(store.pool(), Some(&task))
        .await
        .expect("existing task");
    assert_eq!(same, task);
    assert!(!synthetic);
}
