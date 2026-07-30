use std::collections::BTreeMap;

use agentd_store::{SqliteStore, StoreError, agent_chat_task_graph_repo, agent_repo, message_repo};
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
        execution: None,
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
        execution: None,
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

async fn scalar_count(store: &SqliteStore, sql: &str) -> i64 {
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

#[tokio::test]
async fn node_updates_on_a_cancelled_graph_are_rejected() {
    let (store, _dir) = open_store().await;
    let graph = agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_cancelled".to_string()),
            owner: "orchestrator".to_string(),
            label: "Cancelled graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");
    assert_eq!(graph.status, "active");

    agent_chat_task_graph_repo::delete_graph(store.pool(), "graph_cancelled")
        .await
        .expect("delete graph")
        .expect("graph present");

    let error = agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_cancelled",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("complete".to_string()),
            result: Some(json!({"ok": true})),
            error: None,
        },
    )
    .await
    .expect_err("cancelled graphs reject node updates");
    assert!(
        matches!(&error, agentd_store::StoreError::Conflict(message) if message.contains("cancelled")),
        "expected a conflict naming the graph status, got: {error}"
    );
}

#[tokio::test]
async fn settled_nodes_reject_further_updates() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_settled".to_string()),
            owner: "orchestrator".to_string(),
            label: "Settled graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");

    agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_settled",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("complete".to_string()),
            result: Some(json!({"ok": true})),
            error: None,
        },
    )
    .await
    .expect("first completion")
    .expect("graph and node");

    let error = agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_settled",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("pending".to_string()),
            result: None,
            error: None,
        },
    )
    .await
    .expect_err("a settled node cannot be resurrected");
    assert!(
        matches!(&error, agentd_store::StoreError::Conflict(message) if message.contains("already complete")),
        "expected a conflict naming the settled status, got: {error}"
    );

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_settled")
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(graph.nodes["a"].status, "complete");
    assert_eq!(graph.nodes["a"].result, Some(json!({"ok": true})));
}

#[tokio::test]
async fn empty_node_patches_are_rejected() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_empty_patch".to_string()),
            owner: "orchestrator".to_string(),
            label: "Empty patch graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");

    let error = agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_empty_patch",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode::default(),
    )
    .await
    .expect_err("an empty patch is invalid");
    assert!(
        matches!(&error, agentd_store::StoreError::Invariant(message)
            if message.contains("status, result, or error")),
        "expected an invariant naming the required fields, got: {error}"
    );
}

#[tokio::test]
async fn late_duplicate_result_messages_are_ignored() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_late".to_string()),
            owner: "orchestrator".to_string(),
            label: "Late result graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");
    // `create_graph` only persists the graph; `advance_graph` is what
    // dispatches the root node and assigns its message id.
    let graph = agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_late")
        .await
        .expect("advance graph")
        .expect("graph present");
    let reply_to = graph.nodes["a"]
        .message_id
        .clone()
        .expect("dispatch message id");
    let schema = json!({
        "kind": "task_graph_result",
        "version": 1,
        "payload": {"graphId": "graph_late", "nodeId": "a", "result": {"attempt": 1}}
    });

    let first = agent_chat_task_graph_repo::handle_result_message(
        store.pool(),
        "codex-a",
        Some(&reply_to),
        Some(&schema),
    )
    .await
    .expect("first result")
    .expect("handled");
    assert_eq!(first.status, "complete");

    let second = agent_chat_task_graph_repo::handle_result_message(
        store.pool(),
        "codex-a",
        Some(&reply_to),
        Some(&json!({
            "kind": "task_graph_failed",
            "version": 1,
            "payload": {"graphId": "graph_late", "nodeId": "a", "error": "too late"}
        })),
    )
    .await
    .expect("a late duplicate is not an error");
    assert!(second.is_none(), "late duplicate results are ignored");

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_late")
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(graph.nodes["a"].status, "complete");
    assert_eq!(graph.nodes["a"].error, None);
}

#[tokio::test]
async fn concurrent_sibling_node_completions_all_land() {
    let (store, _dir) = open_store().await;
    let mut nodes = BTreeMap::new();
    for index in 0..8 {
        nodes.insert(format!("n{index}"), node("codex-a", "Do work", &[]));
    }
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_concurrent".to_string()),
            owner: "orchestrator".to_string(),
            label: "Concurrent graph".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");

    let mut handles = Vec::new();
    for index in 0..8 {
        let pool = store.pool().clone();
        handles.push(tokio::spawn(async move {
            agent_chat_task_graph_repo::update_node_and_advance(
                &pool,
                "graph_concurrent",
                &format!("n{index}"),
                agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
                    status: Some("complete".to_string()),
                    result: Some(json!({"node": index})),
                    error: None,
                },
            )
            .await
        }));
    }
    for handle in handles {
        handle
            .await
            .expect("join")
            .expect("concurrent completion succeeds")
            .expect("graph and node");
    }

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_concurrent")
        .await
        .expect("read graph")
        .expect("graph present");
    for index in 0..8 {
        let node = &graph.nodes[&format!("n{index}")];
        assert_eq!(node.status, "complete", "node n{index} lost its completion");
        assert_eq!(node.result, Some(json!({"node": index})));
    }
    assert_eq!(graph.status, "complete");
}

#[tokio::test]
async fn every_graph_write_bumps_the_record_version() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_versioned".to_string()),
            owner: "orchestrator".to_string(),
            label: "Versioned graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");
    let created_version = scalar_count(
        &store,
        "SELECT record_version FROM agent_chat_task_graphs WHERE id = 'graph_versioned'",
    )
    .await;
    assert_eq!(created_version, 1);

    // A productive advance — the root goes from `pending` to `dispatched` — is
    // still a write.
    agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_versioned")
        .await
        .expect("advance graph")
        .expect("graph exists");
    let dispatched_version = scalar_count(
        &store,
        "SELECT record_version FROM agent_chat_task_graphs WHERE id = 'graph_versioned'",
    )
    .await;
    assert!(
        dispatched_version > created_version,
        "dispatching the root must persist: {created_version} -> {dispatched_version}"
    );

    // A node patch that the advance itself cannot act on — `a` moves to
    // `active`, so no dependent unblocks and the graph is not terminal — is
    // still the caller's durable write and must land.
    agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_versioned",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("active".to_string()),
            result: None,
            error: None,
        },
    )
    .await
    .expect("mark node a active")
    .expect("graph and node");
    let active_version = scalar_count(
        &store,
        "SELECT record_version FROM agent_chat_task_graphs WHERE id = 'graph_versioned'",
    )
    .await;
    assert!(
        active_version > dispatched_version,
        "a patch the advance cannot act on must still persist: \
         {dispatched_version} -> {active_version}"
    );
    let reread = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_versioned")
        .await
        .expect("get graph")
        .expect("graph exists");
    assert_eq!(
        reread.nodes["a"].status, "active",
        "the patch must survive a re-read"
    );

    agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_versioned",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("complete".to_string()),
            result: Some(json!({"ok": true})),
            error: None,
        },
    )
    .await
    .expect("complete node a")
    .expect("graph and node");

    let updated_version = scalar_count(
        &store,
        "SELECT record_version FROM agent_chat_task_graphs WHERE id = 'graph_versioned'",
    )
    .await;
    assert!(
        updated_version > active_version,
        "record_version must advance on every write: {active_version} -> {updated_version}"
    );
}

#[tokio::test]
async fn draining_a_queued_ticket_survives_a_concurrent_graph_write() {
    let (store, _dir) = open_store().await;

    // Each round needs exactly one idle coding agent: the node drained in the
    // previous round still holds its agent's reservation, so a fresh agent
    // joins per round to keep "a routes, b queues" reproducible.
    for round in 0..4 {
        register_online_coding_agent(&store, &format!("cod{round}")).await;
        let graph_id = format!("graph_drain_{round}");

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
                id: Some(graph_id.clone()),
                owner: "orchestrator".to_string(),
                label: "Drain race graph".to_string(),
                nodes,
            },
        )
        .await
        .expect("create graph");

        let advanced = agent_chat_task_graph_repo::advance_graph(store.pool(), &graph_id)
            .await
            .expect("advance graph")
            .expect("graph exists");
        assert_eq!(advanced.nodes["a"].status, "dispatched");
        assert_eq!(advanced.nodes["b"].status, "pending");
        assert_eq!(
            advanced.nodes["b"].scheduler_status.as_deref(),
            Some("queued")
        );

        // Completing "a" releases its agent and drains b's ticket in a second,
        // separately versioned write that runs after a's patch has already
        // committed. Marking the queue row drained is the last statement of
        // that release, so watching for it puts this racer exactly in the
        // drain's read-modify-write window: it then bumps the graph row's
        // version from under the drain a few times — fewer than the drain's own
        // attempt budget, so a drain that retries correctly still converges.
        let racer = {
            let pool = store.pool().clone();
            let graph_id = graph_id.clone();
            let ticket = advanced.nodes["b"]
                .scheduler_ticket
                .clone()
                .expect("queued ticket");
            tokio::spawn(async move {
                for _ in 0..100_000 {
                    let status: Option<String> = sqlx::query_scalar(
                        "SELECT status FROM agent_scheduler_queue WHERE ticket = ?",
                    )
                    .bind(&ticket)
                    .fetch_optional(&pool)
                    .await
                    .expect("queue row");
                    if status.as_deref() == Some("drained") {
                        break;
                    }
                }
                for _ in 0..4 {
                    sqlx::query(
                        "UPDATE agent_chat_task_graphs \
                         SET record_version = record_version + 1 WHERE id = ?",
                    )
                    .bind(&graph_id)
                    .execute(&pool)
                    .await
                    .expect("bump graph version");
                }
            })
        };

        let completed = agent_chat_task_graph_repo::update_node_and_advance(
            store.pool(),
            &graph_id,
            "a",
            agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
                status: Some("complete".to_string()),
                result: Some(json!({"ok": true})),
                error: None,
            },
        )
        .await
        .expect("completing a node whose drain lost a version race must still succeed")
        .expect("graph and node");
        assert_eq!(completed.1.status, "complete");
        racer.await.expect("join racer");

        let graph = agent_chat_task_graph_repo::get_graph(store.pool(), &graph_id)
            .await
            .expect("read graph")
            .expect("graph present");
        assert_eq!(
            graph.nodes["b"].status, "dispatched",
            "drained node must not be stranded pending"
        );
        assert_eq!(
            graph.nodes["b"].scheduler_status.as_deref(),
            Some("drained")
        );
        assert_eq!(graph.nodes["b"].assignee, format!("cod{round}"));
    }
}

#[tokio::test]
async fn duplicate_graph_ids_are_rejected_as_conflicts() {
    let (store, _dir) = open_store().await;
    create_live_graph(&store).await;

    let error = agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_live".to_string()),
            owner: "orchestrator".to_string(),
            label: "Duplicate graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect_err("duplicate graph id is rejected");
    assert!(
        matches!(&error, StoreError::Conflict(message) if message.contains("already exists")),
        "duplicate id must be a conflict, got {error:?}"
    );

    // Concurrent creators can clear the pre-check together and only collide on
    // the insert; the loser is still a conflict, never a raw database error.
    let mut handles = Vec::new();
    for index in 0..4 {
        let pool = store.pool().clone();
        handles.push(tokio::spawn(async move {
            agent_chat_task_graph_repo::create_graph(
                &pool,
                agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
                    id: Some("graph_race".to_string()),
                    owner: "orchestrator".to_string(),
                    label: format!("Race graph {index}"),
                    nodes: chain_nodes(),
                },
            )
            .await
        }));
    }
    let mut created = 0;
    for handle in handles {
        match handle.await.expect("join") {
            Ok(_) => created += 1,
            Err(StoreError::Conflict(message)) => {
                assert!(
                    message.contains("already exists"),
                    "unexpected conflict: {message}"
                );
            }
            Err(other) => panic!("duplicate create must not fail internally: {other:?}"),
        }
    }
    assert_eq!(created, 1, "exactly one concurrent creator wins the id");
}

fn native_node(
    provider: &str,
    program: &str,
    depends_on: &[&str],
) -> agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
    agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
        id: None,
        assignee: None,
        role: None,
        capability: None,
        description: "Run the native step".to_string(),
        depends_on: depends_on
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        condition: None,
        execution: Some(json!({
            "version": 1,
            "provider": provider,
            "program": program,
            "args": [],
            "cwd": null,
            "env": []
        })),
    }
}

#[tokio::test]
async fn a_node_with_an_execution_spec_is_queued_for_a_native_worker() {
    let (store, _dir) = open_store().await;
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "build".to_string(),
        native_node("codex", "/usr/bin/codex", &[]),
    );
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_native".to_string()),
            owner: "orchestrator".to_string(),
            label: "Native graph".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");
    // `create_graph` persists; `advance_graph` dispatches.
    let graph = agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_native")
        .await
        .expect("advance graph")
        .expect("graph present");

    let node = &graph.nodes["build"];
    assert_eq!(node.status, "dispatched");
    let execution_task_id = node
        .execution_task_id
        .clone()
        .expect("dispatched native node records its execution task id");
    assert_eq!(node.message_id, None, "native nodes are not messaged");
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM direct_messages").await,
        0
    );

    let (queue_status, provider): (String, Option<String>) = sqlx::query_as(
        "SELECT q.status, json_extract(t.execution_spec_json, '$.provider') \
         FROM execution_task_queue q JOIN task_runs t ON t.id = q.execution_task_id \
         WHERE q.execution_task_id = ?",
    )
    .bind(&execution_task_id)
    .fetch_one(store.pool())
    .await
    .expect("queue row");
    assert_eq!(queue_status, "queued");
    assert_eq!(provider.as_deref(), Some("codex"));

    assert_eq!(
        scalar_count(
            &store,
            "SELECT COUNT(*) FROM task_graph_node_executions \
             WHERE graph_id = 'graph_native' AND node_id = 'build' AND settled = 0"
        )
        .await,
        1
    );
}

async fn create_single_native_graph(store: &SqliteStore, graph_id: &str) {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "build".to_string(),
        native_node("codex", "/usr/bin/codex", &[]),
    );
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some(graph_id.to_string()),
            owner: "orchestrator".to_string(),
            label: "Native replay graph".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");
}

/// Rewind a node to `pending` behind the repo's back. `advance_graph` only
/// dispatches pending nodes, so without this a re-advance never reaches the
/// dispatch path and an idempotency assertion proves nothing.
async fn rewind_node_to_pending(store: &SqliteStore, graph_id: &str, node_id: &str) {
    sqlx::query(
        "UPDATE agent_chat_task_graphs \
         SET raw_json = json_set(raw_json, '$.nodes.' || ? || '.status', 'pending') WHERE id = ?",
    )
    .bind(node_id)
    .bind(graph_id)
    .execute(store.pool())
    .await
    .expect("rewind node");
}

#[tokio::test]
async fn re_advancing_a_native_node_does_not_enqueue_it_twice() {
    let (store, _dir) = open_store().await;
    create_single_native_graph(&store, "graph_native_replay").await;

    let graph = agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_native_replay")
        .await
        .expect("advance")
        .expect("graph present");
    let execution_task_id = graph.nodes["build"]
        .execution_task_id
        .clone()
        .expect("first advance links an execution task");

    // Replay the dispatch twice with the queue row still in place.
    for _ in 0..2 {
        rewind_node_to_pending(&store, "graph_native_replay", "build").await;
        let replayed =
            agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_native_replay")
                .await
                .expect("advance")
                .expect("graph present");
        assert_eq!(
            replayed.nodes["build"].execution_task_id.as_deref(),
            Some(execution_task_id.as_str()),
            "a replayed dispatch reuses the linked execution task"
        );
    }

    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM execution_task_queue").await,
        1
    );
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM task_graph_node_executions").await,
        1
    );
}

#[tokio::test]
async fn a_native_node_stranded_without_a_queue_row_is_healed_by_the_next_advance() {
    let (store, _dir) = open_store().await;
    create_single_native_graph(&store, "graph_native_strand").await;

    let graph = agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_native_strand")
        .await
        .expect("advance")
        .expect("graph present");
    let execution_task_id = graph.nodes["build"]
        .execution_task_id
        .clone()
        .expect("first advance links an execution task");

    // The link row commits before the enqueue, so a crash or a transient
    // storage failure in between leaves exactly this shape: a linked node with
    // no queue row and nothing to settle it.
    sqlx::query("DELETE FROM execution_task_queue")
        .execute(store.pool())
        .await
        .expect("strand the node");
    rewind_node_to_pending(&store, "graph_native_strand", "build").await;

    let healed = agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_native_strand")
        .await
        .expect("advance")
        .expect("graph present");
    assert_eq!(
        healed.nodes["build"].execution_task_id.as_deref(),
        Some(execution_task_id.as_str()),
        "healing re-enqueues the linked task rather than creating a new one"
    );

    let (queued_task, status): (String, String) =
        sqlx::query_as("SELECT execution_task_id, status FROM execution_task_queue")
            .fetch_one(store.pool())
            .await
            .expect("exactly one queue row after healing");
    assert_eq!(queued_task, execution_task_id);
    assert_eq!(status, "queued");
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM task_graph_node_executions").await,
        1
    );
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM task_runs").await,
        1,
        "healing does not mint a second execution task"
    );
}

#[tokio::test]
async fn a_node_cannot_be_both_scheduler_routed_and_natively_executed() {
    let (store, _dir) = open_store().await;
    let mut conflicting = native_node("codex", "/usr/bin/codex", &[]);
    conflicting.role = Some("coding".to_string());
    let mut nodes = BTreeMap::new();
    nodes.insert("build".to_string(), conflicting);

    let error = agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_native_conflict".to_string()),
            owner: "orchestrator".to_string(),
            label: "Conflicting graph".to_string(),
            nodes,
        },
    )
    .await
    .expect_err("role and execution are mutually exclusive");
    assert!(
        matches!(&error, agentd_store::StoreError::Invariant(message)
            if message.contains("role") && message.contains("execution")),
        "expected an invariant naming both fields, got: {error}"
    );
}

#[tokio::test]
async fn a_node_cannot_be_both_assigned_to_an_agent_and_natively_executed() {
    let (store, _dir) = open_store().await;
    let mut conflicting = native_node("codex", "/usr/bin/codex", &[]);
    conflicting.assignee = Some("alice".to_string());
    let mut nodes = BTreeMap::new();
    nodes.insert("build".to_string(), conflicting);

    let error = agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_native_assignee_conflict".to_string()),
            owner: "orchestrator".to_string(),
            label: "Conflicting graph".to_string(),
            nodes,
        },
    )
    .await
    .expect_err("assignee and execution are mutually exclusive");
    assert!(
        matches!(&error, agentd_store::StoreError::Invariant(message)
            if message.contains("assignee") && message.contains("execution")),
        "expected an invariant naming both fields, got: {error}"
    );
}

async fn native_pair_graph(store: &SqliteStore, graph_id: &str) -> String {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "build".to_string(),
        native_node("codex", "/usr/bin/codex", &[]),
    );
    nodes.insert("report".to_string(), node("codex-b", "Report", &["build"]));
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some(graph_id.to_string()),
            owner: "orchestrator".to_string(),
            label: "Native pair".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");
    let graph = agent_chat_task_graph_repo::advance_graph(store.pool(), graph_id)
        .await
        .expect("advance graph")
        .expect("graph present");
    graph.nodes["build"]
        .execution_task_id
        .clone()
        .expect("execution task id")
}

async fn force_queue_status(store: &SqliteStore, execution_task_id: &str, status: &str) {
    sqlx::query(
        "UPDATE execution_task_queue SET status = ?, current_lease_id = NULL, \
         last_reason = 'test forced', updated_at = 0 WHERE execution_task_id = ?",
    )
    .bind(status)
    .bind(execution_task_id)
    .execute(store.pool())
    .await
    .expect("force queue status");
}

#[tokio::test]
async fn a_completed_queue_row_completes_its_node_and_unlocks_downstream() {
    let (store, _dir) = open_store().await;
    let execution_task_id = native_pair_graph(&store, "graph_settle_ok").await;
    force_queue_status(&store, &execution_task_id, "completed").await;

    let settled = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 500)
        .await
        .expect("settle");
    assert_eq!(settled, 1);

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_settle_ok")
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(graph.nodes["build"].status, "complete");
    assert_eq!(
        graph.nodes["build"]
            .result
            .as_ref()
            .and_then(|result| result
                .get("executionTaskId")
                .and_then(serde_json::Value::as_str)),
        Some(execution_task_id.as_str())
    );
    assert_eq!(
        graph.nodes["report"].status, "dispatched",
        "the downstream node must be unlocked by the native completion"
    );

    let again = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 600)
        .await
        .expect("settle again");
    assert_eq!(again, 0, "settlement is idempotent");
}

#[tokio::test]
async fn a_dead_lettered_queue_row_fails_its_node_and_its_graph() {
    let (store, _dir) = open_store().await;
    let execution_task_id = native_pair_graph(&store, "graph_settle_fail").await;
    force_queue_status(&store, &execution_task_id, "dead_letter").await;

    let settled = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 500)
        .await
        .expect("settle");
    assert_eq!(settled, 1);

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_settle_fail")
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(graph.nodes["build"].status, "failed");
    assert!(
        graph.nodes["build"]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("test forced")),
        "the queue reason must reach the node: {:?}",
        graph.nodes["build"].error
    );
    assert_eq!(graph.nodes["report"].status, "failed");
    assert_eq!(graph.status, "failed");
}

#[tokio::test]
async fn settle_absorbs_a_node_already_terminated_by_a_late_message_without_losing_the_link() {
    let (store, _dir) = open_store().await;
    let execution_task_id = native_pair_graph(&store, "graph_settle_race").await;

    // Simulate a late message reaching the node and marking it terminal
    // *before* the queue row itself reaches a terminal status. This is the
    // legitimate-conflict case: update_node_and_advance's settled-node guard
    // will reject the settle pass's own patch as a `StoreError::Conflict`
    // that is not the "changed concurrently" CAS-exhaustion variant.
    agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_settle_race",
        "build",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("failed".to_string()),
            result: None,
            error: Some("message-path failure".to_string()),
        },
    )
    .await
    .expect("update via message path")
    .expect("node present");

    // Only now does the queue row (independently) reach a terminal status.
    force_queue_status(&store, &execution_task_id, "completed").await;

    let settled = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 500)
        .await
        .expect("settle absorbs the legitimate terminal-state conflict");
    assert_eq!(
        settled, 1,
        "the link must still be marked settled even though the patch itself was rejected"
    );

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_settle_race")
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(
        graph.nodes["build"].status, "failed",
        "node status must be left exactly as the message path set it"
    );
    assert_eq!(
        graph.nodes["build"].error.as_deref(),
        Some("message-path failure"),
        "the settle pass must not overwrite the message path's error with its own dead-letter reason"
    );

    let again = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 600)
        .await
        .expect("settle again");
    assert_eq!(again, 0, "the link row must not be revisited once settled");
}

/// A graph whose downstream node is itself native, so settling the upstream one
/// makes the advance enqueue the downstream execution. Returns the upstream
/// node's execution task id.
async fn native_chain_graph(store: &SqliteStore, graph_id: &str) -> String {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "build".to_string(),
        native_node("codex", "/usr/bin/codex", &[]),
    );
    nodes.insert(
        "report".to_string(),
        native_node("codex", "/usr/bin/codex", &["build"]),
    );
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some(graph_id.to_string()),
            owner: "orchestrator".to_string(),
            label: "Native chain".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");
    let graph = agent_chat_task_graph_repo::advance_graph(store.pool(), graph_id)
        .await
        .expect("advance graph")
        .expect("graph present");
    graph.nodes["build"]
        .execution_task_id
        .clone()
        .expect("execution task id")
}

#[tokio::test]
async fn a_downstream_enqueue_outage_keeps_the_settling_node_retryable() {
    let (store, _dir) = open_store().await;
    let graph_id = "graph_settle_enqueue_outage";
    let execution_task_id = native_chain_graph(&store, graph_id).await;
    force_queue_status(&store, &execution_task_id, "completed").await;

    // Make the downstream node's enqueue fail the way a busy database does:
    // the queue INSERT aborts, sqlx surfaces it, and the scheduler port maps it
    // to `Unavailable`, which the repo reports as a `Conflict`. The settle pass
    // must not read that conflict as "somebody else finished this node".
    let downstream_request_id = format!("task-graph-{}-{graph_id}-report", graph_id.len());
    sqlx::query(&format!(
        "CREATE TRIGGER simulate_enqueue_outage BEFORE INSERT ON execution_task_queue \
         WHEN NEW.request_id = '{downstream_request_id}' \
         BEGIN SELECT RAISE(ABORT, 'database is locked'); END"
    ))
    .execute(store.pool())
    .await
    .expect("install the outage trigger");

    let error = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 500)
        .await
        .expect_err("a transient downstream enqueue failure must not be absorbed");
    assert!(
        matches!(&error, StoreError::Conflict(message)
            if message.contains("enqueue node 'report' unavailable")),
        "the outage must surface as the enqueue conflict, got {error:?}"
    );

    assert_eq!(
        scalar_count(
            &store,
            "SELECT COUNT(*) FROM task_graph_node_executions \
             WHERE graph_id = 'graph_settle_enqueue_outage' AND node_id = 'build' AND settled = 0"
        )
        .await,
        1,
        "the link must stay unsettled so the next tick retries it"
    );
    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), graph_id)
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(
        graph.nodes["build"].status, "dispatched",
        "the patch never reached the graph, so the node must be untouched"
    );
    assert_eq!(graph.nodes["report"].status, "pending");
    assert_eq!(graph.status, "active");

    // The outage clears; the very next settle pass heals both nodes.
    sqlx::query("DROP TRIGGER simulate_enqueue_outage")
        .execute(store.pool())
        .await
        .expect("clear the outage");

    let settled = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 600)
        .await
        .expect("settle after the outage clears");
    assert_eq!(settled, 1);
    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), graph_id)
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(graph.nodes["build"].status, "complete");
    assert_eq!(graph.nodes["report"].status, "dispatched");
    assert_eq!(
        scalar_count(
            &store,
            "SELECT COUNT(*) FROM execution_task_queue q \
             JOIN task_graph_node_executions e ON e.execution_task_id = q.execution_task_id \
             WHERE e.graph_id = 'graph_settle_enqueue_outage' AND e.node_id = 'report'"
        )
        .await,
        1,
        "the downstream execution must be enqueued on the retry"
    );
    assert_eq!(
        scalar_count(
            &store,
            "SELECT COUNT(*) FROM task_graph_node_executions \
             WHERE graph_id = 'graph_settle_enqueue_outage' AND node_id = 'build' AND settled = 1"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn deleting_a_graph_settles_its_open_node_executions() {
    let (store, _dir) = open_store().await;
    let execution_task_id = native_pair_graph(&store, "graph_settle_deleted").await;

    agent_chat_task_graph_repo::delete_graph(store.pool(), "graph_settle_deleted")
        .await
        .expect("delete graph")
        .expect("graph present");
    force_queue_status(&store, &execution_task_id, "completed").await;

    let settled = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 500)
        .await
        .expect("settle");
    assert_eq!(settled, 0, "a deleted graph has no open node executions");

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_settle_deleted")
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(graph.nodes["build"].status, "cancelled");
}

#[tokio::test]
async fn one_unreadable_legacy_row_does_not_break_the_listing() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_good".to_string()),
            owner: "orchestrator".to_string(),
            label: "Good graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");
    sqlx::query(
        "INSERT INTO agent_chat_task_graphs \
         (id, owner, label, status, raw_json, record_version, imported_at) \
         VALUES ('graph_broken', 'alex', 'Broken', 'active', '{\"nodes\":', 1, 0)",
    )
    .execute(store.pool())
    .await
    .expect("insert unreadable row");

    let listed = agent_chat_task_graph_repo::list_graphs(store.pool(), None)
        .await
        .expect("listing tolerates one unreadable row");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "graph_good");
}

#[tokio::test]
async fn advance_active_graphs_redrives_a_graph_whose_initial_advance_never_ran() {
    let (store, _dir) = open_store().await;

    // `create_graph` only persists; `advance_graph` is what dispatches. A
    // create whose advance failed on a transient database error leaves exactly
    // this state, and before this sweep nothing re-drove it.
    let mut nodes = BTreeMap::new();
    nodes.insert("a".to_string(), node("codex-a", "Do A", &[]));
    let created = agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_stranded".to_string()),
            owner: "orchestrator".to_string(),
            label: "Stranded graph".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");
    assert_eq!(created.status, "active");
    assert_eq!(created.nodes["a"].status, "pending");
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM direct_messages").await,
        0
    );

    let advanced = agent_chat_task_graph_repo::advance_active_graphs(store.pool())
        .await
        .expect("advance active graphs");
    assert_eq!(advanced, 1);

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_stranded")
        .await
        .expect("get graph")
        .expect("graph exists");
    assert_eq!(graph.nodes["a"].status, "dispatched");
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM direct_messages").await,
        1
    );
}

#[tokio::test]
async fn advance_active_graphs_is_idempotent_and_skips_settled_graphs() {
    let (store, _dir) = open_store().await;

    let mut nodes = BTreeMap::new();
    nodes.insert("a".to_string(), node("codex-a", "Do A", &[]));
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_once".to_string()),
            owner: "orchestrator".to_string(),
            label: "Once".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");

    let first = agent_chat_task_graph_repo::advance_active_graphs(store.pool())
        .await
        .expect("first sweep");
    assert_eq!(first, 1);
    let second = agent_chat_task_graph_repo::advance_active_graphs(store.pool())
        .await
        .expect("second sweep");
    assert_eq!(
        second, 1,
        "an already-dispatched active graph re-advances cleanly"
    );
    // The dispatch itself must not be repeated: one message, not two.
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM direct_messages").await,
        1
    );

    agent_chat_task_graph_repo::delete_graph(store.pool(), "graph_once")
        .await
        .expect("delete graph");
    let third = agent_chat_task_graph_repo::advance_active_graphs(store.pool())
        .await
        .expect("third sweep");
    assert_eq!(third, 0, "a cancelled graph is not active and is skipped");
}

#[tokio::test]
async fn a_sweep_that_advances_nothing_does_not_write() {
    let (store, _dir) = open_store().await;

    // The sweep runs every maintenance tick, and a graph parked on a
    // `dispatched` node waiting for an external reply advances nothing on each
    // of those ticks. Re-serializing the whole graph and taking the single
    // writer for that is pure write amplification, so a no-op advance must not
    // touch the row at all.
    let mut nodes = BTreeMap::new();
    nodes.insert("a".to_string(), node("codex-a", "Do A", &[]));
    nodes.insert("b".to_string(), node("codex-b", "Do B", &["a"]));
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_parked".to_string()),
            owner: "orchestrator".to_string(),
            label: "Parked".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");

    let advanced = agent_chat_task_graph_repo::advance_active_graphs(store.pool())
        .await
        .expect("first sweep");
    assert_eq!(advanced, 1);
    let parked = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_parked")
        .await
        .expect("get graph")
        .expect("graph exists");
    assert_eq!(parked.nodes["a"].status, "dispatched");
    assert_eq!(parked.nodes["b"].status, "pending");

    let parked_version = scalar_count(
        &store,
        "SELECT record_version FROM agent_chat_task_graphs WHERE id = 'graph_parked'",
    )
    .await;

    for _ in 0..4 {
        agent_chat_task_graph_repo::advance_active_graphs(store.pool())
            .await
            .expect("no-op sweep");
    }

    let swept_version = scalar_count(
        &store,
        "SELECT record_version FROM agent_chat_task_graphs WHERE id = 'graph_parked'",
    )
    .await;
    assert_eq!(
        swept_version, parked_version,
        "a sweep that changes nothing must not write the graph row"
    );
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM direct_messages").await,
        1,
        "and must not re-dispatch"
    );
}

/// The status filter must run in SQL, keyed on the `status` column, not in
/// Rust on the status deserialized from `raw_json`.
///
/// `advance_active_graphs` calls this on every maintenance tick, so filtering
/// after `row_to_graph` would deserialize every graph ever created — nothing
/// deletes completed ones — twelve times a minute on the pool the HTTP
/// handlers share.
///
/// Desyncing the column from the JSON is a test-only artifact: no writer can
/// produce it, and it is the only way to observe which of the two the query
/// actually keys on. Before the SQL predicate this listing returned the row,
/// because the parsed status still said `active`.
#[tokio::test]
async fn list_graphs_filters_on_the_status_column_not_the_parsed_json() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_desynced".to_string()),
            owner: "orchestrator".to_string(),
            label: "Desynced".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");
    sqlx::query("UPDATE agent_chat_task_graphs SET status = 'completed' WHERE id = ?")
        .bind("graph_desynced")
        .execute(store.pool())
        .await
        .expect("desync the status column from raw_json");

    let active = agent_chat_task_graph_repo::list_graphs(store.pool(), Some("active"))
        .await
        .expect("list active graphs");
    assert!(
        active.is_empty(),
        "a row whose status column is 'completed' must never be selected for 'active', \
         even though its raw_json still says active: {active:?}"
    );

    // An unparseable row that the status column excludes must not change the
    // outcome either, and the unfiltered listing still tolerates it.
    sqlx::query(
        "INSERT INTO agent_chat_task_graphs \
         (id, owner, label, status, raw_json, record_version, imported_at) \
         VALUES ('graph_broken', 'alex', 'Broken', 'completed', '{\"nodes\":', 1, 0)",
    )
    .execute(store.pool())
    .await
    .expect("insert unreadable row");
    let active = agent_chat_task_graph_repo::list_graphs(store.pool(), Some("active"))
        .await
        .expect("list active graphs alongside an unreadable row");
    assert!(active.is_empty());
    let all = agent_chat_task_graph_repo::list_graphs(store.pool(), None)
        .await
        .expect("unfiltered listing still tolerates the unreadable row");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, "graph_desynced");
}

/// `upsert_imported_task_graph` binds the source's `status` field, which is
/// `NULL` when the imported graph has none — while `normalize_imported_task_graph`
/// gives that same graph the `"active"` fallback inside `raw_json`. A bare
/// `WHERE status = ?` would drop those rows, stranding every imported active
/// graph: invisible to `/api/task-graphs?status=active` and never advanced by
/// the maintenance sweep.
#[tokio::test]
async fn list_graphs_still_matches_an_imported_row_with_a_null_status_column() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_imported".to_string()),
            owner: "orchestrator".to_string(),
            label: "Imported".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");
    sqlx::query("UPDATE agent_chat_task_graphs SET status = NULL WHERE id = ?")
        .bind("graph_imported")
        .execute(store.pool())
        .await
        .expect("null the status column the way an import without a status does");

    let active = agent_chat_task_graph_repo::list_graphs(store.pool(), Some("active"))
        .await
        .expect("list active graphs");
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "graph_imported");
    assert_eq!(active[0].status, "active");

    let completed = agent_chat_task_graph_repo::list_graphs(store.pool(), Some("completed"))
        .await
        .expect("list completed graphs");
    assert!(
        completed.is_empty(),
        "a NULL status column still has to satisfy the Rust filter: {completed:?}"
    );
}
