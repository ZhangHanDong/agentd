//! The daemon maintenance tick must fence agents that stopped heartbeating,
//! not only workers. Without this an agent that dies silently stays `online`
//! in the registry forever.

use agentd_bin::daemon::{AGENT_HEARTBEAT_TIMEOUT_SECS, agent_registry_tick};
use agentd_store::{SqliteStore, agent_repo};

#[tokio::test]
async fn the_maintenance_tick_fences_agents_that_stopped_heartbeating() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");

    agent_repo::heartbeat_agent(
        store.pool(),
        "codex-dev",
        agent_repo::HeartbeatAgent {
            server: Some("local".to_string()),
            tmux_target: Some("codex-dev:0.0".to_string()),
            workspace_path: None,
        },
    )
    .await
    .expect("heartbeat");

    let observed_at = 1_000_000_i64;
    sqlx::query("UPDATE agents SET last_seen_at = ? WHERE name = ?")
        .bind(observed_at - AGENT_HEARTBEAT_TIMEOUT_SECS - 1)
        .bind("codex-dev")
        .execute(store.pool())
        .await
        .expect("backdate");

    let swept = agent_registry_tick(store.pool(), observed_at).await;
    assert_eq!(swept, 1);

    let agent = agent_repo::get_agent(store.pool(), "codex-dev")
        .await
        .expect("get")
        .expect("agent exists");
    assert_eq!(agent.status, "offline");
    assert_eq!(agent.offline_reason.as_deref(), Some("heartbeat-timeout"));

    // An agent inside the window is left alone.
    agent_repo::heartbeat_agent(
        store.pool(),
        "codex-dev",
        agent_repo::HeartbeatAgent {
            server: None,
            tmux_target: None,
            workspace_path: None,
        },
    )
    .await
    .expect("re-heartbeat");
    assert_eq!(agent_registry_tick(store.pool(), observed_at).await, 0);
}
