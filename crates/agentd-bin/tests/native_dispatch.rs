use agentd_bin::cli::DaemonConfig;
use agentd_bin::daemon::{DispatchRoute, dispatch_task_to_fleet, production_dispatch_route};
use agentd_core::types::{NativeExecutionSpec, NodeId, RunId};
use agentd_store::{SqliteStore, run_repo, task_repo};

fn config_with_native_dispatch(native: bool) -> DaemonConfig {
    let mut config = DaemonConfig::for_test();
    config.native_dispatch = native;
    config
}

#[tokio::test]
async fn default_route_is_tmux_and_switch_selects_native_queue() {
    assert_eq!(
        production_dispatch_route(&config_with_native_dispatch(false)),
        DispatchRoute::Tmux
    );
    assert_eq!(
        production_dispatch_route(&config_with_native_dispatch(true)),
        DispatchRoute::NativeQueue
    );
}

#[tokio::test]
async fn dispatch_task_to_fleet_enqueues_a_queued_row_with_spec() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let run_id = RunId::new();
    run_repo::insert_run(store.pool(), &run_id, "workflow-sha")
        .await
        .expect("run");
    let task_id = task_repo::insert_task_run(store.pool(), &run_id, &NodeId::parsed("impl"))
        .await
        .expect("task");
    let spec = NativeExecutionSpec {
        version: 1,
        provider: "codex".into(),
        program: "/usr/bin/codex".into(),
        args: vec![],
        cwd: None,
        env: vec![],
    };

    dispatch_task_to_fleet(&store, &task_id, &spec, 100)
        .await
        .expect("dispatch");

    let (status, provider): (String, Option<String>) = sqlx::query_as(
        "SELECT q.status, json_extract(t.execution_spec_json, '$.provider') \
         FROM execution_task_queue q JOIN task_runs t ON t.id = q.execution_task_id \
         WHERE q.execution_task_id = ?",
    )
    .bind(task_id.as_str())
    .fetch_one(store.pool())
    .await
    .expect("queue row");
    assert_eq!(status, "queued");
    assert_eq!(provider.as_deref(), Some("codex"));
}
