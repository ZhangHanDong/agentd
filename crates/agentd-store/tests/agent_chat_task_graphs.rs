use std::collections::BTreeMap;

use agentd_store::{SqliteStore, agent_chat_task_graph_repo, agent_repo, message_repo};
use serde_json::json;

async fn open_store() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("target dir");
    let db = dir.path().join("agentd.db");
    let store = SqliteStore::connect(&db).await.expect("connect store");
    (store, dir)
}

fn node(
    assignee: &str,
    description: &str,
    depends_on: &[&str],
) -> agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
    agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
        id: None,
        assignee: Some(assignee.to_string()),
        role: None,
        capability: None,
        description: description.to_string(),
        depends_on: depends_on
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        condition: None,
    }
}

fn scheduled_node(
    role: &str,
    capability: &str,
    description: &str,
    depends_on: &[&str],
) -> agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
    agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
        id: None,
        assignee: None,
        role: Some(role.to_string()),
        capability: Some(capability.to_string()),
        description: description.to_string(),
        depends_on: depends_on
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        condition: None,
    }
}

async fn register_online_coding_agent(store: &SqliteStore, name: &str) {
    agent_repo::register_agent(
        store.pool(),
        agent_repo::RegisterAgent {
            name: name.to_string(),
            role: Some("coding".to_string()),
            capability: Some("medium".to_string()),
            runtime: Some("codex".to_string()),
            model: None,
            tmux_target: Some(format!("{name}:0.0")),
            home_dir: None,
            workdir: Some(format!("/tmp/agentd/{name}")),
            state_dir: None,
            server: None,
            runtime_profile: json!({}),
        },
    )
    .await
    .expect("register agent");
}

async fn scalar_count(store: &SqliteStore, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(store.pool())
        .await
        .expect("count query")
}

fn chain_nodes() -> BTreeMap<String, agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput> {
    let mut nodes = BTreeMap::new();
    nodes.insert("a".to_string(), node("codex-a", "Do A", &[]));
    nodes.insert("b".to_string(), node("codex-b", "Do B", &["a"]));
    let mut conditional = node("codex-c", "Do C", &["a"]);
    conditional.condition = Some(json!({
        "dep": "a",
        "path": "ok",
        "eq": false
    }));
    nodes.insert("c".to_string(), conditional);
    nodes
}

async fn create_live_graph(
    store: &SqliteStore,
) -> agent_chat_task_graph_repo::AgentChatTaskGraphRecord {
    let created = agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_live".to_string()),
            owner: "orchestrator".to_string(),
            label: "Live graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");
    assert_eq!(created.status, "active");
    assert_eq!(created.nodes["a"].status, "pending");
    created
}

async fn assert_root_dispatch(
    store: &SqliteStore,
) -> agent_chat_task_graph_repo::AgentChatTaskGraphRecord {
    let advanced = agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_live")
        .await
        .expect("advance graph")
        .expect("graph exists");
    assert_eq!(advanced.nodes["a"].status, "dispatched");
    assert!(advanced.nodes["a"].message_id.is_some());
    assert_eq!(advanced.nodes["b"].status, "pending");
    let a_inbox = message_repo::read_direct_inbox(
        store.pool(),
        "codex-a",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("codex-a inbox");
    assert_eq!(a_inbox.len(), 1);
    assert_eq!(a_inbox[0].from, "orchestrator");
    assert_eq!(a_inbox[0].to, "codex-a");
    assert_eq!(a_inbox[0].message_type, "request");
    assert_eq!(a_inbox[0].priority, "high");
    let a_schema = a_inbox[0].schema.as_ref().expect("dispatch schema");
    assert_eq!(a_schema["kind"], "task_graph_dispatch");
    assert_eq!(a_schema["payload"]["graphId"], "graph_live");
    assert_eq!(a_schema["payload"]["nodeId"], "a");

    let advanced_again = agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_live")
        .await
        .expect("advance graph again")
        .expect("graph exists");
    assert_eq!(
        advanced_again.nodes["a"].message_id,
        advanced.nodes["a"].message_id
    );
    let a_inbox_again = message_repo::read_direct_inbox(
        store.pool(),
        "codex-a",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("codex-a inbox again");
    assert_eq!(a_inbox_again.len(), 1, "dispatch is idempotent");
    advanced
}

#[tokio::test]
async fn agent_chat_task_graph_repo_dispatch_advance_conditions_and_cancel() {
    let (store, _dir) = open_store().await;
    create_live_graph(&store).await;
    assert_root_dispatch(&store).await;

    let (after_a, node_a) = agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_live",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("complete".to_string()),
            result: Some(json!({ "ok": true })),
            error: None,
        },
    )
    .await
    .expect("complete node a")
    .expect("node exists");
    assert_eq!(node_a.status, "complete");
    assert_eq!(after_a.nodes["b"].status, "dispatched");
    assert_eq!(after_a.nodes["c"].status, "skipped");
    let b_inbox = message_repo::read_direct_inbox(
        store.pool(),
        "codex-b",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("codex-b inbox");
    assert_eq!(b_inbox.len(), 1);
    let b_schema = b_inbox[0].schema.as_ref().expect("dependency schema");
    assert_eq!(b_schema["payload"]["dependencyResults"][0]["nodeId"], "a");

    let (complete_graph, node_b) = agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_live",
        "b",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("complete".to_string()),
            result: Some(json!({ "ok": true })),
            error: None,
        },
    )
    .await
    .expect("complete node b")
    .expect("node exists");
    assert_eq!(node_b.status, "complete");
    assert_eq!(complete_graph.status, "complete");

    let cancelled = agent_chat_task_graph_repo::delete_graph(store.pool(), "graph_live")
        .await
        .expect("delete graph")
        .expect("graph exists");
    assert_eq!(cancelled.status, "cancelled");
    assert_eq!(cancelled.nodes["a"].status, "complete");
    assert_eq!(cancelled.nodes["b"].status, "complete");
    assert_eq!(cancelled.nodes["c"].status, "skipped");
}

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn agent_chat_task_graph_scheduler_routes_queues_and_drains_nodes() {
    let (store, _dir) = open_store().await;
    register_online_coding_agent(&store, "cod1").await;

    let mut nodes = BTreeMap::new();
    nodes.insert(
        "a".to_string(),
        scheduled_node("coding", "medium", "Do scheduled A", &[]),
    );
    nodes.insert(
        "b".to_string(),
        scheduled_node("coding", "medium", "Do scheduled B", &[]),
    );
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_sched".to_string()),
            owner: "orchestrator".to_string(),
            label: "Scheduled graph".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");

    let advanced = agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_sched")
        .await
        .expect("advance graph")
        .expect("graph exists");
    let node_a = &advanced.nodes["a"];
    let node_b = &advanced.nodes["b"];
    assert_eq!(node_a.status, "dispatched");
    assert_eq!(node_a.assignee, "cod1");
    assert_eq!(node_a.role.as_deref(), Some("coding"));
    assert_eq!(node_a.tier.as_deref(), Some("medium"));
    assert_eq!(node_a.scheduler_status.as_deref(), Some("routed"));
    assert!(node_a.scheduler_reservation_id.is_some());
    assert_eq!(node_b.status, "pending");
    assert_eq!(node_b.scheduler_status.as_deref(), Some("queued"));
    assert!(node_b.scheduler_ticket.is_some());

    let inbox = message_repo::read_direct_inbox(
        store.pool(),
        "cod1",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("cod1 inbox");
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].to, "cod1");
    let first_message_id = inbox[0].id.clone();
    let schema = inbox[0].schema.as_ref().expect("dispatch schema");
    assert_eq!(schema["payload"]["nodeId"], "a");
    assert_eq!(
        schema["payload"]["schedulerReservationId"],
        node_a.scheduler_reservation_id.as_deref().unwrap()
    );

    let handled = agent_chat_task_graph_repo::handle_result_message(
        store.pool(),
        "cod1",
        Some(first_message_id.as_str()),
        Some(&json!({
            "kind": "task_graph_result",
            "version": 1,
            "payload": {
                "graphId": "graph_sched",
                "nodeId": "a",
                "result": {"ok": true}
            }
        })),
    )
    .await
    .expect("handle result")
    .expect("result handled");
    assert_eq!(handled.status, "complete");
    let final_graph = handled.graph;
    assert_eq!(final_graph.nodes["a"].status, "complete");
    assert_eq!(final_graph.nodes["b"].status, "dispatched");
    assert_eq!(final_graph.nodes["b"].assignee, "cod1");
    assert_eq!(
        final_graph.nodes["b"].scheduler_status.as_deref(),
        Some("drained")
    );
    assert!(final_graph.nodes["b"].scheduler_reservation_id.is_some());

    let inbox = message_repo::read_direct_inbox(
        store.pool(),
        "cod1",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("cod1 inbox after drain");
    assert_eq!(
        inbox.len(),
        2,
        "queued graph node should dispatch exactly once"
    );
    assert_eq!(inbox[1].schema.as_ref().unwrap()["payload"]["nodeId"], "b");
    assert_eq!(
        scalar_count(
            &store,
            "SELECT COUNT(*) FROM agent_scheduler_reservations WHERE status IN ('routed', 'drained')"
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &store,
            "SELECT COUNT(*) FROM agent_scheduler_reservations WHERE status = 'released'"
        )
        .await,
        1
    );
    assert_eq!(
        scalar_count(
            &store,
            "SELECT COUNT(*) FROM agent_scheduler_queue WHERE status = 'drained'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn agent_chat_task_graph_direct_assignee_nodes_do_not_create_scheduler_reservations() {
    let (store, _dir) = open_store().await;
    let mut nodes = BTreeMap::new();
    nodes.insert("a".to_string(), node("codex-a", "Do direct A", &[]));
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_direct".to_string()),
            owner: "orchestrator".to_string(),
            label: "Direct graph".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");

    let advanced = agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_direct")
        .await
        .expect("advance graph")
        .expect("graph exists");
    assert_eq!(advanced.nodes["a"].status, "dispatched");
    assert_eq!(advanced.nodes["a"].assignee, "codex-a");
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM agent_scheduler_reservations").await,
        0
    );
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM agent_scheduler_queue").await,
        0
    );
}
