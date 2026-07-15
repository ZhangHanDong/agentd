use agentd_store::{SqliteStore, agent_repo, agent_scheduler_repo};
use serde_json::json;

async fn open_temp() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("agentd.db");
    let store = SqliteStore::connect(&db).await.expect("connect + migrate");
    (store, dir)
}

fn text(value: &str) -> String {
    value.to_string()
}

async fn register_online_coding_agent(store: &SqliteStore, name: &str) {
    agent_repo::register_agent(
        store.pool(),
        agent_repo::RegisterAgent {
            name: text(name),
            role: Some(text("coding")),
            capability: Some(text("medium")),
            runtime: Some(text("codex")),
            model: Some(text("gpt-5")),
            tmux_target: Some(format!("{name}:0.0")),
            home_dir: None,
            workdir: Some(format!("/tmp/agentd/{name}")),
            state_dir: None,
            server: Some(text("local")),
            runtime_profile: json!({"primary": {"framework": "codex", "model": "gpt-5"}}),
        },
    )
    .await
    .expect("register online coding agent");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn agent_scheduler_routes_queues_provisions_and_drains_durably() {
    let (store, _dir) = open_temp().await;
    register_online_coding_agent(&store, "cod1").await;

    let first = agent_scheduler_repo::dispatch(
        store.pool(),
        agent_scheduler_repo::DispatchRequest {
            role: text("coding"),
            capability: Some(text("medium")),
            task: Some(json!("A")),
            room: Some(text("factory")),
        },
        agent_scheduler_repo::SchedulerConfig { max_per_cell: 0 },
    )
    .await
    .expect("first dispatch");
    assert_eq!(first.status, "routed");
    assert_eq!(first.agent.as_deref(), Some("cod1"));
    assert_eq!(first.role, "coding");
    assert_eq!(first.tier, "medium");
    let first_reservation = first.reservation.expect("routed reservation");
    assert_eq!(first_reservation.agent.as_deref(), Some("cod1"));
    assert_eq!(first_reservation.task.as_ref(), Some(&json!("A")));

    let busy = agent_scheduler_repo::pool_snapshot(
        store.pool(),
        agent_scheduler_repo::PoolFilters {
            state: Some(text("busy")),
            ..Default::default()
        },
    )
    .await
    .expect("busy pool");
    assert!(
        busy.agents
            .iter()
            .any(|agent| agent.name == "cod1" && agent.busy),
        "routed agent should be busy: {busy:?}"
    );

    let second = agent_scheduler_repo::dispatch(
        store.pool(),
        agent_scheduler_repo::DispatchRequest {
            role: text("coding"),
            capability: Some(text("medium")),
            task: Some(json!("B")),
            room: Some(text("factory")),
        },
        agent_scheduler_repo::SchedulerConfig { max_per_cell: 0 },
    )
    .await
    .expect("second dispatch");
    assert_eq!(second.status, "queued");
    assert!(second.ticket.is_some(), "queued dispatch returns a ticket");
    assert_eq!(second.queue_depth, Some(1));

    let drained = agent_scheduler_repo::release(
        store.pool(),
        agent_scheduler_repo::ReleaseRequest {
            agent: text("cod1"),
        },
    )
    .await
    .expect("release drains queue");
    assert_eq!(drained.status, "drained");
    assert_eq!(drained.agent, "cod1");
    assert_eq!(drained.task.as_ref(), Some(&json!("B")));
    assert_eq!(drained.room.as_deref(), Some("factory"));
    assert!(
        drained.reservation.is_some(),
        "drained release creates a reservation"
    );

    let still_busy = agent_scheduler_repo::pool_snapshot(
        store.pool(),
        agent_scheduler_repo::PoolFilters {
            state: Some(text("busy")),
            ..Default::default()
        },
    )
    .await
    .expect("busy pool after drain");
    assert!(
        still_busy
            .agents
            .iter()
            .any(|agent| agent.name == "cod1" && agent.busy),
        "drained ticket should reserve cod1 again: {still_busy:?}"
    );

    let provision = agent_scheduler_repo::dispatch(
        store.pool(),
        agent_scheduler_repo::DispatchRequest {
            role: text("documentation"),
            capability: Some(text("lightweight")),
            task: Some(json!({"title": "docs"})),
            room: None,
        },
        agent_scheduler_repo::SchedulerConfig { max_per_cell: 1 },
    )
    .await
    .expect("provision plan");
    assert_eq!(provision.status, "provision");
    assert!(
        provision
            .name
            .as_deref()
            .is_some_and(|name| name.starts_with("mx_documentation_lightweight_")),
        "provision plan names generated agents: {provision:?}"
    );
    assert_eq!(provision.runtime["runtime"], "claude");
    assert_eq!(provision.runtime["model"], "haiku");

    let over_cap = agent_scheduler_repo::dispatch(
        store.pool(),
        agent_scheduler_repo::DispatchRequest {
            role: text("documentation"),
            capability: Some(text("lightweight")),
            task: Some(json!({"title": "docs again"})),
            room: None,
        },
        agent_scheduler_repo::SchedulerConfig { max_per_cell: 1 },
    )
    .await
    .expect("provision cap reached");
    assert_eq!(over_cap.status, "queued");
    assert_eq!(over_cap.queue_depth, Some(1));
}
