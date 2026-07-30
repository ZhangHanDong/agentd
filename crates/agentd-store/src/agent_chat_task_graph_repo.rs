//! Live agent-chat-compatible task-graph operations.
//!
//! p225 preserved imported `task_graphs.json` snapshots in
//! `agent_chat_task_graphs.raw_json`; p227 makes that compatibility table live.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

use agentd_core::types::{NativeExecutionSpec, NodeId, RunId};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, SqlitePool};

use crate::agent_scheduler_repo::{self, SchedulerConfig};
use crate::error::StoreError;
use crate::message_repo::{self, DirectMessageInput};
use crate::util::{now_unix, now_unix_ms};

static NEXT_GRAPH_ID: AtomicU64 = AtomicU64::new(1);

const GRAPH_STATUSES: &[&str] = &["active", "complete", "failed", "cancelled"];
const NODE_STATUSES: &[&str] = &[
    "pending",
    "dispatched",
    "active",
    "complete",
    "failed",
    "skipped",
    "cancelled",
];
const RESULT_MAX_BYTES: usize = 65_536;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatTaskGraphRecord {
    pub id: String,
    pub owner: String,
    pub label: String,
    pub status: String,
    #[serde(default)]
    pub nodes: BTreeMap<String, AgentChatTaskGraphNode>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    #[serde(
        rename = "completedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<String>,
    /// Column-backed optimistic-concurrency version. Never serialized into
    /// `raw_json` (the column is the authority) and never deserialized from it;
    /// `0` means "not yet inserted".
    #[serde(default, skip_serializing)]
    pub record_version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentChatTaskGraphNode {
    pub id: String,
    pub assignee: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    #[serde(
        default,
        rename = "schedulerReservationId",
        alias = "scheduler_reservation_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub scheduler_reservation_id: Option<String>,
    #[serde(
        default,
        rename = "schedulerTicket",
        alias = "scheduler_ticket",
        skip_serializing_if = "Option::is_none"
    )]
    pub scheduler_ticket: Option<String>,
    #[serde(
        default,
        rename = "schedulerStatus",
        alias = "scheduler_status",
        skip_serializing_if = "Option::is_none"
    )]
    pub scheduler_status: Option<String>,
    #[serde(
        default,
        rename = "provisionedName",
        alias = "provisioned_name",
        skip_serializing_if = "Option::is_none"
    )]
    pub provisioned_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<Value>,
    pub description: String,
    #[serde(default, alias = "dependsOn")]
    pub depends_on: Vec<String>,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<Value>,
    /// A `NativeExecutionSpec`-shaped object. When present the node is executed
    /// by a native worker through the M2 durable queue instead of being
    /// messaged to an assignee.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<Value>,
    #[serde(
        default,
        rename = "executionTaskId",
        alias = "execution_task_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub execution_task_id: Option<String>,
    #[serde(
        default,
        rename = "message_id",
        alias = "messageId",
        skip_serializing_if = "Option::is_none"
    )]
    pub message_id: Option<String>,
    #[serde(rename = "startedAt", default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    #[serde(
        rename = "dispatchedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub dispatched_at: Option<String>,
    #[serde(
        rename = "completedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentChatTaskGraphNodeInput {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub capability: Option<String>,
    pub description: String,
    #[serde(default, alias = "dependsOn")]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub condition: Option<Value>,
    #[serde(default)]
    pub execution: Option<Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateAgentChatTaskGraph {
    #[serde(default)]
    pub id: Option<String>,
    pub owner: String,
    pub label: String,
    #[serde(default)]
    pub nodes: BTreeMap<String, AgentChatTaskGraphNodeInput>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct UpdateAgentChatTaskGraphNode {
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentChatTaskGraphMessageResult {
    pub graph_id: String,
    pub node_id: String,
    pub status: String,
    pub graph: AgentChatTaskGraphRecord,
}

pub async fn create_graph(
    pool: &SqlitePool,
    input: CreateAgentChatTaskGraph,
) -> Result<AgentChatTaskGraphRecord, StoreError> {
    let id = input
        .id
        .and_then(|value| clean_text(Some(value)))
        .unwrap_or_else(generate_graph_id);
    if get_graph(pool, &id).await?.is_some() {
        return Err(graph_already_exists(&id));
    }
    let owner = required(input.owner, "owner required")?;
    let label = required(input.label, "label required")?;
    if input.nodes.is_empty() {
        return Err(StoreError::Invariant("nodes required".to_string()));
    }
    let mut nodes = BTreeMap::new();
    for (key, input_node) in input.nodes {
        let node_id = clean_text(input_node.id).unwrap_or_else(|| key.clone());
        let key = required(key, "node id required")?;
        if node_id != key {
            return Err(StoreError::Invariant(format!(
                "node id mismatch: key '{key}' vs id '{node_id}'"
            )));
        }
        nodes.insert(
            key.clone(),
            AgentChatTaskGraphNode {
                id: key,
                assignee: clean_text(input_node.assignee).unwrap_or_default(),
                role: clean_text(input_node.role),
                capability: clean_text(input_node.capability),
                tier: None,
                scheduler_reservation_id: None,
                scheduler_ticket: None,
                scheduler_status: None,
                provisioned_name: None,
                runtime: None,
                description: required(input_node.description, "node description required")?,
                depends_on: normalize_list(input_node.depends_on),
                status: "pending".to_string(),
                result: None,
                error: None,
                condition: input_node.condition,
                execution: input_node.execution,
                execution_task_id: None,
                message_id: None,
                started_at: None,
                dispatched_at: None,
                completed_at: None,
            },
        );
    }
    validate_graph_nodes(&nodes)?;
    let now = now_text();
    let mut graph = AgentChatTaskGraphRecord {
        id,
        owner,
        label,
        status: "active".to_string(),
        nodes,
        created_at: now.clone(),
        updated_at: now,
        completed_at: None,
        record_version: 0,
    };
    upsert_graph(pool, &mut graph).await?;
    Ok(graph)
}

pub async fn list_graphs(
    pool: &SqlitePool,
    status: Option<&str>,
) -> Result<Vec<AgentChatTaskGraphRecord>, StoreError> {
    let status = status.and_then(|value| clean_text(Some(value.to_string())));
    // Pre-filter in SQL rather than only after `row_to_graph`: this runs on
    // every maintenance tick via `advance_active_graphs`, and parsing first
    // would deserialize the `raw_json` of every graph ever created — including
    // long-completed ones, which nothing deletes.
    //
    // The `OR` below defeats `idx_agent_chat_task_graphs_status`: EXPLAIN QUERY
    // PLAN reports SCAN, not a SEARCH via that index (plain `status = ?` would
    // seek it, `ORDER BY rowid` included). So this still walks every leaf page
    // per tick, growing with total graph count. What it does buy is the part
    // that mattered: `raw_json` never crosses into Rust and `serde_json` never
    // runs for a non-matching row, since SQLite reads `status` from the record
    // header without faulting in the overflow pages. To get the index seek
    // back, make the data agree instead of widening the predicate — have the
    // import bind the same normalized status it writes into `raw_json`, backfill
    // `UPDATE ... SET status = 'active' WHERE status IS NULL` (exactly what
    // every reader already infers for those rows), then drop the `OR`.
    //
    // `OR status IS NULL` keeps the SQL predicate exactly as wide as the Rust
    // one below. `upsert_graph` always writes the column from the same value it
    // serializes into `raw_json`, and `upsert_imported_task_graph` binds the
    // source's `status` field trimmed the same way `normalize_imported_task_graph`
    // trims it — so a non-NULL column always equals the JSON status. The one
    // divergence is an imported graph with no usable `status` field at all: the
    // column is NULL while the JSON gets the `"active"` fallback. Those rows
    // must still reach the Rust filter or importing would strand active graphs.
    let sql = match status {
        Some(_) => graph_select_sql("WHERE status = ? OR status IS NULL ORDER BY rowid"),
        None => graph_select_sql("ORDER BY rowid"),
    };
    let mut query = sqlx::query(sql.as_str());
    if let Some(status) = status.clone() {
        query = query.bind(status);
    }
    let rows = query.fetch_all(pool).await?;
    // A legacy row that predates normalization must not take the whole listing
    // down with it; `get_graph` still surfaces the parse error for that id.
    let graphs = rows
        .iter()
        .filter_map(|row| row_to_graph(row).ok())
        .filter(|graph| {
            status
                .as_deref()
                .is_none_or(|status| graph.status == status)
        })
        .collect();
    Ok(graphs)
}

pub async fn get_graph(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<AgentChatTaskGraphRecord>, StoreError> {
    let Some(id) = clean_str(id) else {
        return Ok(None);
    };
    let row = sqlx::query(graph_select_sql("WHERE id = ?").as_str())
        .bind(&id)
        .fetch_optional(pool)
        .await?;
    row.map(|row| row_to_graph(&row)).transpose()
}

pub async fn advance_graph(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<AgentChatTaskGraphRecord>, StoreError> {
    for _ in 0..MAX_GRAPH_WRITE_ATTEMPTS {
        let Some(graph) = get_graph(pool, id).await? else {
            return Ok(None);
        };
        match advance_graph_record(pool, graph, false).await {
            Ok(graph) => return Ok(Some(graph)),
            Err(error) if is_concurrent_write_conflict(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Err(concurrent_write_conflict(id))
}

pub async fn update_node_and_advance(
    pool: &SqlitePool,
    graph_id: &str,
    node_id: &str,
    patch: UpdateAgentChatTaskGraphNode,
) -> Result<Option<(AgentChatTaskGraphRecord, AgentChatTaskGraphNode)>, StoreError> {
    for _ in 0..MAX_GRAPH_WRITE_ATTEMPTS {
        match update_node_and_advance_once(pool, graph_id, node_id, patch.clone()).await {
            Err(error) if is_concurrent_write_conflict(&error) => {}
            other => return other,
        }
    }
    Err(concurrent_write_conflict(graph_id))
}

async fn update_node_and_advance_once(
    pool: &SqlitePool,
    graph_id: &str,
    node_id: &str,
    patch: UpdateAgentChatTaskGraphNode,
) -> Result<Option<(AgentChatTaskGraphRecord, AgentChatTaskGraphNode)>, StoreError> {
    let Some(mut graph) = get_graph(pool, graph_id).await? else {
        return Ok(None);
    };
    // A graph that is no longer active is settled: `advance_graph_record`
    // early-returns for it, so without this guard the patch would still be
    // persisted onto a cancelled/complete graph by the trailing upsert.
    if graph.status != "active" {
        return Err(StoreError::Conflict(format!(
            "task graph '{graph_id}' is {} and no longer accepts node updates",
            graph.status
        )));
    }
    let Some(node) = graph.nodes.get_mut(node_id) else {
        return Ok(None);
    };
    // Settled nodes are immutable: their result has already unlocked (or
    // failed) downstream work, so rewriting one would desynchronize the graph
    // from the decisions already taken on it.
    if node_terminal(&node.status) {
        return Err(StoreError::Conflict(format!(
            "task graph node '{node_id}' is already {}",
            node.status
        )));
    }
    let release_agent = patch
        .status
        .as_deref()
        .and_then(clean_str)
        .filter(|status| node_terminal(status))
        .and_then(|_| {
            node.scheduler_reservation_id
                .as_ref()
                .and_then(|_| clean_str(&node.assignee))
        });
    apply_node_patch(node, patch)?;
    graph.updated_at = now_text();
    // The patch above is this call's whole point, so the advance must write
    // even when it finds nothing further to do.
    let mut graph = advance_graph_record(pool, graph, true).await?;
    if let Some(agent) = release_agent {
        // The node patch is committed by this point, so the scheduler drain is
        // a best-effort follow-on rather than part of this call's outcome. Its
        // own version conflict must not escape into the caller's retry wrapper:
        // a second attempt would re-read the node as already terminal, trip the
        // settled-node guard, and turn a durable success into a spurious 409.
        if let Err(error) = release_and_drain_scheduler(pool, &agent).await {
            if !is_concurrent_write_conflict(&error) {
                return Err(error);
            }
            tracing::error!(
                graph_id,
                agent = agent.as_str(),
                error = %error,
                "drained task-graph ticket lost every dispatch attempt; its node stays pending with a stale scheduler ticket"
            );
        }
        if let Some(updated) = get_graph(pool, graph_id).await? {
            graph = updated;
        }
    }
    let Some(node) = graph.nodes.get(node_id).cloned() else {
        return Ok(None);
    };
    Ok(Some((graph, node)))
}

pub async fn delete_graph(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<AgentChatTaskGraphRecord>, StoreError> {
    for _ in 0..MAX_GRAPH_WRITE_ATTEMPTS {
        match delete_graph_once(pool, id).await {
            Err(error) if is_concurrent_write_conflict(&error) => {}
            other => return other,
        }
    }
    Err(concurrent_write_conflict(id))
}

async fn delete_graph_once(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<AgentChatTaskGraphRecord>, StoreError> {
    let Some(mut graph) = get_graph(pool, id).await? else {
        return Ok(None);
    };
    let now = now_text();
    graph.status = "cancelled".to_string();
    graph.updated_at.clone_from(&now);
    graph.completed_at = Some(now.clone());
    for node in graph.nodes.values_mut() {
        if !node_terminal(&node.status) {
            node.status = "cancelled".to_string();
            node.completed_at = Some(now.clone());
        }
    }
    // The graph is cancelled, so its nodes must never be rewritten by a later
    // settlement pass. The in-flight queue rows are left to finish and are
    // discarded; M2 exposes no scheduler cancel primitive (see plan non-goals).
    sqlx::query(
        "UPDATE task_graph_node_executions SET settled = 1, settled_at = ? \
         WHERE graph_id = ? AND settled = 0",
    )
    .bind(now_unix())
    .bind(&graph.id)
    .execute(pool)
    .await?;
    upsert_graph(pool, &mut graph).await?;
    Ok(Some(graph))
}

pub async fn handle_result_message(
    pool: &SqlitePool,
    from: &str,
    reply_to: Option<&str>,
    schema: Option<&Value>,
) -> Result<Option<AgentChatTaskGraphMessageResult>, StoreError> {
    let Some(reply_to) = reply_to.and_then(clean_str) else {
        return Ok(None);
    };
    let Some(schema) = schema else {
        return Ok(None);
    };
    let kind = schema
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(kind, "task_graph_result" | "task_graph_failed") {
        return Ok(None);
    }
    let Some(payload) = schema.get("payload").and_then(Value::as_object) else {
        return Ok(None);
    };
    let Some(graph_id) = payload
        .get("graphId")
        .and_then(Value::as_str)
        .and_then(clean_str)
    else {
        return Ok(None);
    };
    let Some(node_id) = payload
        .get("nodeId")
        .and_then(Value::as_str)
        .and_then(clean_str)
    else {
        return Ok(None);
    };
    let Some(graph) = get_graph(pool, &graph_id).await? else {
        return Ok(None);
    };
    let Some(node) = graph.nodes.get(&node_id) else {
        return Ok(None);
    };
    if node.assignee != from || node.message_id.as_deref() != Some(reply_to.as_str()) {
        return Ok(None);
    }
    // A late or duplicated result for an already-settled node (or a graph that
    // has since been cancelled) is dropped, not rejected: the message itself is
    // legitimate and is still stored, it simply no longer moves the graph.
    if graph.status != "active" || node_terminal(&node.status) {
        return Ok(None);
    }

    let patch = if kind == "task_graph_result" {
        UpdateAgentChatTaskGraphNode {
            status: Some("complete".to_string()),
            result: payload.get("result").cloned(),
            error: None,
        }
    } else {
        UpdateAgentChatTaskGraphNode {
            status: Some("failed".to_string()),
            result: payload.get("result").cloned(),
            error: payload
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| Some("task graph node failed".to_string())),
        }
    };
    let Some((graph, node)) = update_node_and_advance(pool, &graph_id, &node_id, patch).await?
    else {
        return Ok(None);
    };
    Ok(Some(AgentChatTaskGraphMessageResult {
        graph_id,
        node_id,
        status: node.status,
        graph,
    }))
}

/// Drive a graph as far as it will go and persist it.
///
/// `caller_changed` reports whether the caller already mutated `graph` and is
/// relying on this call's write to persist it. It matters because the write is
/// skipped when nothing changed: the maintenance sweep re-advances every active
/// graph every tick, and a graph parked on a `dispatched` node awaiting an
/// external reply would otherwise re-serialize its whole JSON and take the
/// single writer on each of those ticks for no state change at all. A caller
/// that patched a node must pass `true`, or that patch is silently dropped.
#[allow(clippy::too_many_lines)]
async fn advance_graph_record(
    pool: &SqlitePool,
    mut graph: AgentChatTaskGraphRecord,
    caller_changed: bool,
) -> Result<AgentChatTaskGraphRecord, StoreError> {
    if graph.status != "active" {
        if caller_changed {
            upsert_graph(pool, &mut graph).await?;
        }
        return Ok(graph);
    }
    let mut changed = caller_changed;
    loop {
        let mut changed_this_pass = false;
        let node_ids = graph.nodes.keys().cloned().collect::<Vec<_>>();
        for node_id in node_ids {
            let Some(snapshot) = graph.nodes.get(&node_id).cloned() else {
                continue;
            };
            if snapshot.status != "pending" {
                continue;
            }
            if snapshot.depends_on.iter().any(|dep| {
                graph
                    .nodes
                    .get(dep)
                    .is_some_and(|node| matches!(node.status.as_str(), "failed" | "cancelled"))
            }) {
                let now = now_text();
                if let Some(node) = graph.nodes.get_mut(&node_id) {
                    node.status = "failed".to_string();
                    node.error = Some("dependency failed".to_string());
                    node.completed_at = Some(now);
                }
                changed = true;
                changed_this_pass = true;
                continue;
            }
            let deps_ready = snapshot.depends_on.iter().all(|dep| {
                graph
                    .nodes
                    .get(dep)
                    .is_some_and(|node| matches!(node.status.as_str(), "complete" | "skipped"))
            });
            if !deps_ready {
                continue;
            }
            if !condition_matches(&graph, &snapshot)? {
                let now = now_text();
                if let Some(node) = graph.nodes.get_mut(&node_id) {
                    node.status = "skipped".to_string();
                    node.completed_at = Some(now);
                }
                changed = true;
                changed_this_pass = true;
                continue;
            }
            if matches!(
                snapshot.scheduler_status.as_deref(),
                Some("queued" | "provision")
            ) {
                continue;
            }
            if snapshot.execution.is_some() {
                let execution_task_id = dispatch_native_node(pool, &graph.id, &snapshot).await?;
                let dispatched_at = now_text();
                if let Some(node) = graph.nodes.get_mut(&node_id) {
                    node.status = "dispatched".to_string();
                    node.execution_task_id = Some(execution_task_id);
                    node.dispatched_at.get_or_insert(dispatched_at);
                }
            } else if snapshot.role.as_deref().and_then(clean_str).is_some() {
                match dispatch_scheduled_node(pool, &graph, &snapshot).await? {
                    ScheduledNodeDispatch::Routed {
                        message,
                        agent,
                        reservation_id,
                        role,
                        tier,
                    } => {
                        let message = *message;
                        let dispatched_at = now_text();
                        if let Some(node) = graph.nodes.get_mut(&node_id) {
                            node.assignee = agent;
                            node.role = Some(role);
                            node.tier = Some(tier);
                            node.scheduler_reservation_id = Some(reservation_id);
                            node.scheduler_status = Some("routed".to_string());
                            node.status = "dispatched".to_string();
                            node.message_id = Some(message.id.clone());
                            node.dispatched_at.get_or_insert(dispatched_at);
                        }
                        message_repo::insert_direct_message(pool, message.input).await?;
                    }
                    ScheduledNodeDispatch::Queued { ticket, role, tier } => {
                        if let Some(node) = graph.nodes.get_mut(&node_id) {
                            node.role = Some(role);
                            node.tier = Some(tier);
                            node.scheduler_ticket = Some(ticket);
                            node.scheduler_status = Some("queued".to_string());
                        }
                    }
                    ScheduledNodeDispatch::Provision {
                        reservation_id,
                        provisioned_name,
                        runtime,
                        role,
                        tier,
                    } => {
                        if let Some(node) = graph.nodes.get_mut(&node_id) {
                            node.role = Some(role);
                            node.tier = Some(tier);
                            node.scheduler_reservation_id = Some(reservation_id);
                            node.provisioned_name = provisioned_name;
                            node.runtime = Some(runtime);
                            node.scheduler_status = Some("provision".to_string());
                        }
                    }
                }
            } else {
                let dispatch = dispatch_message(&graph, &snapshot);
                let dispatched_at = now_text();
                if let Some(node) = graph.nodes.get_mut(&node_id) {
                    node.status = "dispatched".to_string();
                    node.message_id = Some(dispatch.id.clone());
                    node.dispatched_at.get_or_insert(dispatched_at);
                }
                message_repo::insert_direct_message(pool, dispatch.input).await?;
            }
            changed = true;
            changed_this_pass = true;
        }
        if !changed_this_pass {
            break;
        }
    }

    if graph.nodes.values().all(|node| node_terminal(&node.status)) {
        graph.status = if graph.nodes.values().any(|node| node.status == "failed") {
            "failed".to_string()
        } else {
            "complete".to_string()
        };
        graph.completed_at = Some(now_text());
        changed = true;
    }
    if changed {
        graph.updated_at = now_text();
        upsert_graph(pool, &mut graph).await?;
    }
    Ok(graph)
}

struct DispatchMessage {
    id: String,
    input: DirectMessageInput,
}

enum ScheduledNodeDispatch {
    Routed {
        message: Box<DispatchMessage>,
        agent: String,
        reservation_id: String,
        role: String,
        tier: String,
    },
    Queued {
        ticket: String,
        role: String,
        tier: String,
    },
    Provision {
        reservation_id: String,
        provisioned_name: Option<String>,
        runtime: Value,
        role: String,
        tier: String,
    },
}

struct SchedulerMessageMeta {
    reservation_id: String,
    scheduler_status: String,
    role: String,
    tier: String,
}

async fn dispatch_scheduled_node(
    pool: &SqlitePool,
    graph: &AgentChatTaskGraphRecord,
    node: &AgentChatTaskGraphNode,
) -> Result<ScheduledNodeDispatch, StoreError> {
    let role = node
        .role
        .as_deref()
        .and_then(clean_str)
        .ok_or_else(|| StoreError::Invariant("scheduler role required".to_string()))?;
    let result = agent_scheduler_repo::dispatch(
        pool,
        agent_scheduler_repo::DispatchRequest {
            role,
            capability: node.capability.clone(),
            task: Some(scheduler_task_payload(graph, node)),
            room: Some(graph.id.clone()),
        },
        SchedulerConfig::default(),
    )
    .await?;
    match result.status.as_str() {
        "routed" => {
            let reservation = result.reservation.ok_or_else(|| {
                StoreError::Invariant("routed scheduler result missing reservation".to_string())
            })?;
            let agent = result.agent.ok_or_else(|| {
                StoreError::Invariant("routed scheduler result missing agent".to_string())
            })?;
            let meta = SchedulerMessageMeta {
                reservation_id: reservation.id.clone(),
                scheduler_status: "routed".to_string(),
                role: result.role.clone(),
                tier: result.tier.clone(),
            };
            Ok(ScheduledNodeDispatch::Routed {
                message: Box::new(dispatch_message_to(graph, node, &agent, Some(&meta))),
                agent,
                reservation_id: reservation.id,
                role: result.role,
                tier: result.tier,
            })
        }
        "queued" => Ok(ScheduledNodeDispatch::Queued {
            ticket: result.ticket.ok_or_else(|| {
                StoreError::Invariant("queued scheduler result missing ticket".to_string())
            })?,
            role: result.role,
            tier: result.tier,
        }),
        "provision" => {
            let reservation = result.reservation.ok_or_else(|| {
                StoreError::Invariant("provision scheduler result missing reservation".to_string())
            })?;
            Ok(ScheduledNodeDispatch::Provision {
                reservation_id: reservation.id,
                provisioned_name: result.name,
                runtime: result.runtime,
                role: result.role,
                tier: result.tier,
            })
        }
        other => Err(StoreError::Invariant(format!(
            "unsupported scheduler result status: {other}"
        ))),
    }
}

fn parse_execution_spec(
    node_id: &str,
    execution: &Value,
) -> Result<NativeExecutionSpec, StoreError> {
    let spec: NativeExecutionSpec = serde_json::from_value(execution.clone())
        .map_err(|error| StoreError::Invariant(format!("node '{node_id}' execution: {error}")))?;
    spec.validate().map_err(|message| {
        StoreError::Invariant(format!("node '{node_id}' execution: {message}"))
    })?;
    Ok(spec)
}

/// Permanent idempotency key for a node's durable-queue enqueue. Deriving it
/// from the graph and node ids (never from a fresh ULID) is what makes a
/// replayed advance return the existing queue row instead of a second one.
/// The graph id is length-prefixed to keep the key injective: a plain
/// `{graph_id}-{node_id}` join collides for graph "a-b" + node "c" against
/// graph "a" + node "b-c", and a request-id collision is a permanent denial
/// (the losing node's enqueue fails the payload-equality check forever).
fn node_execution_request_id(graph_id: &str, node_id: &str) -> String {
    format!("task-graph-{}-{graph_id}-{node_id}", graph_id.len())
}

/// The committed link between a graph node and its execution task. `created_at`
/// is load-bearing: it is the enqueue's timestamp, so a replayed enqueue is an
/// exact replay rather than a payload-mismatch `Conflict`.
struct NodeExecutionLink {
    execution_task_id: String,
    created_at: i64,
}

async fn existing_node_execution(
    pool: &SqlitePool,
    graph_id: &str,
    node_id: &str,
) -> Result<Option<NodeExecutionLink>, StoreError> {
    let existing: Option<(String, i64)> = sqlx::query_as(
        "SELECT execution_task_id, created_at FROM task_graph_node_executions \
         WHERE graph_id = ? AND node_id = ?",
    )
    .bind(graph_id)
    .bind(node_id)
    .fetch_optional(pool)
    .await?;
    Ok(
        existing.map(|(execution_task_id, created_at)| NodeExecutionLink {
            execution_task_id,
            created_at,
        }),
    )
}

/// Whether the task already has a queue row in any state. Terminal queue rows
/// are retained, so absence is unambiguous: the enqueue never landed.
async fn node_execution_queued(
    pool: &SqlitePool,
    execution_task_id: &str,
) -> Result<bool, StoreError> {
    let found: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM execution_task_queue WHERE execution_task_id = ?")
            .bind(execution_task_id)
            .fetch_optional(pool)
            .await?;
    Ok(found.is_some())
}

/// Enqueue the node's execution task, keyed on the permanent request id and
/// timestamped from the link row so every replay is byte-identical and the
/// scheduler hands back the existing row instead of a `Conflict`.
async fn enqueue_node_execution(
    pool: &SqlitePool,
    graph_id: &str,
    node_id: &str,
    link: &NodeExecutionLink,
) -> Result<(), StoreError> {
    agentd_core::ports::DurableSchedulerPort::enqueue(
        &crate::durable_scheduler::SqliteDurableScheduler::new(pool.clone()),
        &agentd_core::ports::SchedulerEnqueueRequest {
            request_id: node_execution_request_id(graph_id, node_id),
            execution_task_id: agentd_core::types::TaskRunId::from_string(
                link.execution_task_id.clone(),
            ),
            max_attempts: 3,
            available_at: link.created_at,
            enqueued_at: link.created_at,
        },
    )
    .await
    .map_err(|error| enqueue_failed(node_id, &error))?;
    Ok(())
}

/// A transient scheduler failure is reported as a `Conflict` so callers can
/// retry the advance; anything else is a genuine invariant break. Neither
/// message ends in `CONCURRENT_WRITE_SUFFIX`, so the graph write-retry wrapper
/// does not swallow either one.
fn enqueue_failed(node_id: &str, error: &agentd_core::ports::DurableSchedulerError) -> StoreError {
    match error {
        agentd_core::ports::DurableSchedulerError::Unavailable(_) => StoreError::Conflict(format!(
            "enqueue node '{node_id}' unavailable, retry: {error}"
        )),
        _ => StoreError::Invariant(format!("enqueue node '{node_id}': {error}")),
    }
}

/// Create (or reuse) the node's execution task and enqueue it for a native
/// worker. Ordering is deliberate: the link row is written before the enqueue
/// so a crash can at worst leak an un-enqueued `task_runs` row, never a second
/// queue entry for the same node. That leak is then healed here — a link row
/// with no queue row means the enqueue never landed, and the replay below is
/// exact, so it either creates the missing row or returns the one that raced
/// in.
async fn dispatch_native_node(
    pool: &SqlitePool,
    graph_id: &str,
    node: &AgentChatTaskGraphNode,
) -> Result<String, StoreError> {
    if let Some(link) = existing_node_execution(pool, graph_id, &node.id).await? {
        if !node_execution_queued(pool, &link.execution_task_id).await? {
            enqueue_node_execution(pool, graph_id, &node.id, &link).await?;
        }
        return Ok(link.execution_task_id);
    }
    let execution = node
        .execution
        .as_ref()
        .ok_or_else(|| StoreError::Invariant(format!("node '{}' has no execution", node.id)))?;
    let spec = parse_execution_spec(&node.id, execution)?;
    let run_id = RunId::new();
    crate::run_repo::insert_run(pool, &run_id, &format!("task-graph:{graph_id}")).await?;
    let task_id = crate::task_repo::insert_task_run_with_spec(
        pool,
        &run_id,
        &NodeId::parsed(node.id.clone()),
        &spec,
    )
    .await?;
    let created_at = now_unix();
    sqlx::query(
        "INSERT INTO task_graph_node_executions \
         (graph_id, node_id, execution_task_id, run_id, settled, created_at) \
         VALUES (?, ?, ?, ?, 0, ?) ON CONFLICT(graph_id, node_id) DO NOTHING",
    )
    .bind(graph_id)
    .bind(&node.id)
    .bind(task_id.as_str())
    .bind(run_id.as_str())
    .bind(created_at)
    .execute(pool)
    .await?;
    // A concurrent advance may have won the insert; the winner's row is the one
    // that gets enqueued, timestamps included.
    let link = existing_node_execution(pool, graph_id, &node.id)
        .await?
        .unwrap_or(NodeExecutionLink {
            execution_task_id: task_id.as_str().to_string(),
            created_at,
        });
    enqueue_node_execution(pool, graph_id, &node.id, &link).await?;
    Ok(link.execution_task_id)
}

/// Re-drive every `active` task graph.
///
/// `advance_graph` has exactly one caller — graph creation — so a create whose
/// advance failed on a transient database error strands an `active` graph with
/// a `pending` root that nothing else re-drives. Advancing is idempotent (a
/// node already `dispatched` is not re-dispatched), so an unconditional sweep
/// is the whole repair.
///
/// Returns the number of graphs advanced. One graph's failure is isolated and
/// logged, never propagated: this runs on the maintenance tick, where a single
/// poisoned graph must not stop the sweep or the loop.
pub async fn advance_active_graphs(pool: &SqlitePool) -> Result<u64, StoreError> {
    let graphs = list_graphs(pool, Some("active")).await?;
    let mut advanced = 0_u64;
    for graph in graphs {
        match advance_graph(pool, &graph.id).await {
            Ok(Some(_)) => advanced += 1,
            // Deleted between the listing and the advance; nothing to repair.
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    graph_id = graph.id.as_str(),
                    %error,
                    "re-advancing active task graph failed this tick"
                );
            }
        }
    }
    Ok(advanced)
}

/// `(graph_id, node_id, execution_task_id, queue_status, last_reason)`
type NodeSettlementRow = (String, String, String, String, Option<String>);

/// Drive task-graph nodes from the durable scheduler's terminal queue statuses.
/// Called every daemon maintenance tick, immediately after
/// `SqliteDurableScheduler::reconcile` has mapped terminal leases onto queue
/// rows. Returns the number of link rows settled.
///
/// # Errors
/// [`StoreError`] if the link/queue query or a node update fails.
pub async fn settle_node_executions(
    pool: &SqlitePool,
    observed_at: i64,
) -> Result<u64, StoreError> {
    let rows: Vec<NodeSettlementRow> = sqlx::query_as(
        "SELECT e.graph_id, e.node_id, e.execution_task_id, q.status, q.last_reason \
         FROM task_graph_node_executions e \
         JOIN execution_task_queue q ON q.execution_task_id = e.execution_task_id \
         WHERE e.settled = 0 AND q.status IN ('completed', 'dead_letter', 'cancelled')",
    )
    .fetch_all(pool)
    .await?;
    let mut settled = 0_u64;
    for (graph_id, node_id, execution_task_id, queue_status, last_reason) in rows {
        let patch = match queue_status.as_str() {
            "completed" => UpdateAgentChatTaskGraphNode {
                status: Some("complete".to_string()),
                result: Some(json!({
                    "executionTaskId": execution_task_id,
                    "queueStatus": queue_status,
                })),
                error: None,
            },
            "cancelled" => UpdateAgentChatTaskGraphNode {
                status: Some("cancelled".to_string()),
                result: None,
                error: None,
            },
            // Only reachable via dead_letter today: the SQL IN list above and
            // the execution_task_queue CHECK constraint together enumerate
            // every queue_status this query can observe. Adding a new queue
            // status to the SELECT's IN list requires a matching arm here.
            _ => UpdateAgentChatTaskGraphNode {
                status: Some("failed".to_string()),
                result: None,
                error: Some(last_reason.unwrap_or_else(|| "execution dead-lettered".to_string())),
            },
        };
        if let Err(error) = update_node_and_advance(pool, &graph_id, &node_id, patch).await {
            // Marking the link settled discards this row forever: the
            // `settled = 0` filter above never selects it again, and a native
            // node carries no assignee token, so nothing else can drive it.
            // So absorb only on proof, never on the error variant alone.
            //
            // The "changed concurrently" CAS-exhaustion conflict is a plain
            // propagate: the patch was never applied.
            if is_concurrent_write_conflict(&error) {
                return Err(error);
            }
            if !matches!(error, StoreError::Conflict(_)) {
                return Err(error);
            }
            // Every other `Conflict` needs the graph re-read, because the
            // variant does not say whether the patch landed. Two shapes reach
            // here and they want opposite handling:
            //  - the node or its graph settled by another route (an operator
            //    cancelled the graph, a late result message landed) before
            //    this patch could apply. The execution is finished either way,
            //    so absorb and close the link row.
            //  - `update_node_and_advance` applied the patch in memory, then
            //    the advance it triggered failed downstream *before* the
            //    graph was persisted, so nothing landed. `enqueue_failed`
            //    reports a transient scheduler outage this way
            //    ("enqueue node '…' unavailable, retry: …") when advancing
            //    into a downstream native node hits a busy database. Absorbing
            //    that would strand this node dispatched and the downstream one
            //    pending with no alarm and no way back.
            // The re-read tells them apart from state alone, so no caller has
            // to keep an inventory of conflict messages in sync with this one.
            if !node_settled_elsewhere(pool, &graph_id, &node_id).await? {
                return Err(error);
            }
        }
        mark_node_execution_settled(pool, &graph_id, &node_id, observed_at).await?;
        settled += 1;
    }
    Ok(settled)
}

/// Whether the persisted graph proves this node no longer needs the settle
/// pass's patch: the graph is gone or no longer active, or the node is gone or
/// already terminal. Read after a `Conflict` from `update_node_and_advance`,
/// this is the evidence that separates "somebody else already settled it" from
/// "the write never landed".
async fn node_settled_elsewhere(
    pool: &SqlitePool,
    graph_id: &str,
    node_id: &str,
) -> Result<bool, StoreError> {
    let Some(graph) = get_graph(pool, graph_id).await? else {
        return Ok(true);
    };
    if graph.status != "active" {
        return Ok(true);
    }
    let Some(node) = graph.nodes.get(node_id) else {
        return Ok(true);
    };
    Ok(node_terminal(&node.status))
}

async fn mark_node_execution_settled(
    pool: &SqlitePool,
    graph_id: &str,
    node_id: &str,
    observed_at: i64,
) -> Result<(), StoreError> {
    let updated = sqlx::query(
        "UPDATE task_graph_node_executions SET settled = 1, settled_at = ? \
         WHERE graph_id = ? AND node_id = ? AND settled = 0",
    )
    .bind(observed_at)
    .bind(graph_id)
    .bind(node_id)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "node execution '{graph_id}/{node_id}' was settled concurrently"
        )));
    }
    Ok(())
}

fn dispatch_message(
    graph: &AgentChatTaskGraphRecord,
    node: &AgentChatTaskGraphNode,
) -> DispatchMessage {
    dispatch_message_to(graph, node, &node.assignee, None)
}

fn dispatch_message_to(
    graph: &AgentChatTaskGraphRecord,
    node: &AgentChatTaskGraphNode,
    to: &str,
    scheduler: Option<&SchedulerMessageMeta>,
) -> DispatchMessage {
    let id = node
        .message_id
        .clone()
        .unwrap_or_else(|| dispatch_message_id(&graph.id, &node.id));
    let dependency_results = node
        .depends_on
        .iter()
        .filter_map(|dep| {
            graph.nodes.get(dep).map(|dep_node| {
                json!({
                    "nodeId": dep,
                    "status": dep_node.status.clone(),
                    "result": dep_node.result.clone(),
                    "error": dep_node.error.clone(),
                })
            })
        })
        .collect::<Vec<_>>();
    let mut payload = json!({
        "dispatchKey": format!("{}:{}", graph.id, node.id),
        "graphId": graph.id.clone(),
        "nodeId": node.id.clone(),
        "description": node.description.clone(),
        "dependencyResults": dependency_results,
    });
    if let Some(scheduler) = scheduler
        && let Some(payload) = payload.as_object_mut()
    {
        payload.insert(
            "schedulerReservationId".to_string(),
            Value::String(scheduler.reservation_id.clone()),
        );
        payload.insert(
            "schedulerStatus".to_string(),
            Value::String(scheduler.scheduler_status.clone()),
        );
        payload.insert("role".to_string(), Value::String(scheduler.role.clone()));
        payload.insert("tier".to_string(), Value::String(scheduler.tier.clone()));
    }
    let schema = json!({
        "kind": "task_graph_dispatch",
        "version": 1,
        "payload": payload
    });
    DispatchMessage {
        id: id.clone(),
        input: DirectMessageInput {
            message_id: Some(id),
            ts: None,
            from: graph.owner.clone(),
            to: to.to_string(),
            message_type: Some("request".to_string()),
            priority: Some("high".to_string()),
            summary: format!(
                "Task graph {} / {}: {}",
                graph.id, node.id, node.description
            ),
            full: node.description.clone(),
            reply_to: None,
            source: Some("task_graph".to_string()),
            source_room: None,
            sender_mxid: None,
            trust_level: Some("system".to_string()),
            from_id: Some(graph.owner.clone()),
            schema: Some(schema),
            attachments: Vec::new(),
        },
    }
}

fn scheduler_task_payload(
    graph: &AgentChatTaskGraphRecord,
    node: &AgentChatTaskGraphNode,
) -> Value {
    let dependency_results = node
        .depends_on
        .iter()
        .filter_map(|dep| {
            graph.nodes.get(dep).map(|dep_node| {
                json!({
                    "nodeId": dep,
                    "status": dep_node.status.clone(),
                    "result": dep_node.result.clone(),
                    "error": dep_node.error.clone(),
                })
            })
        })
        .collect::<Vec<_>>();
    json!({
        "kind": "task_graph_node",
        "graphId": graph.id.clone(),
        "nodeId": node.id.clone(),
        "description": node.description.clone(),
        "dependencyResults": dependency_results,
    })
}

async fn release_and_drain_scheduler(pool: &SqlitePool, agent: &str) -> Result<(), StoreError> {
    let release = agent_scheduler_repo::release(
        pool,
        agent_scheduler_repo::ReleaseRequest {
            agent: agent.to_string(),
        },
    )
    .await?;
    if release.status == "drained" {
        dispatch_drained_task_graph_ticket(pool, &release).await?;
    }
    Ok(())
}

/// Marking the drained node dispatched is its own versioned write, made after
/// whatever write triggered the release already committed. Absorb a version
/// race here — against a fresh read of the graph — so the conflict never
/// reaches a caller whose own work is already durable.
async fn dispatch_drained_task_graph_ticket(
    pool: &SqlitePool,
    release: &agent_scheduler_repo::ReleaseResult,
) -> Result<(), StoreError> {
    for _ in 0..MAX_GRAPH_WRITE_ATTEMPTS {
        match dispatch_drained_task_graph_ticket_once(pool, release).await {
            Err(error) if is_concurrent_write_conflict(&error) => {}
            other => return other,
        }
    }
    let graph_id = release
        .task
        .as_ref()
        .and_then(|task| task.get("graphId"))
        .and_then(Value::as_str)
        .unwrap_or(release.agent.as_str());
    Err(concurrent_write_conflict(graph_id))
}

async fn dispatch_drained_task_graph_ticket_once(
    pool: &SqlitePool,
    release: &agent_scheduler_repo::ReleaseResult,
) -> Result<(), StoreError> {
    let Some(task) = release.task.as_ref() else {
        return Ok(());
    };
    if task.get("kind").and_then(Value::as_str) != Some("task_graph_node") {
        return Ok(());
    }
    let Some(graph_id) = task
        .get("graphId")
        .and_then(Value::as_str)
        .and_then(clean_str)
    else {
        return Ok(());
    };
    let Some(node_id) = task
        .get("nodeId")
        .and_then(Value::as_str)
        .and_then(clean_str)
    else {
        return Ok(());
    };
    let Some(mut graph) = get_graph(pool, &graph_id).await? else {
        return Ok(());
    };
    if graph.status != "active" {
        return Ok(());
    }
    let Some(snapshot) = graph.nodes.get(&node_id).cloned() else {
        return Ok(());
    };
    if snapshot.status != "pending" {
        return Ok(());
    }
    if release.ticket.as_deref() != snapshot.scheduler_ticket.as_deref() {
        return Ok(());
    }
    let Some(reservation) = release.reservation.as_ref() else {
        return Ok(());
    };
    let role = release
        .role
        .clone()
        .unwrap_or_else(|| reservation.role.clone());
    let tier = release
        .tier
        .clone()
        .unwrap_or_else(|| reservation.tier.clone());
    let meta = SchedulerMessageMeta {
        reservation_id: reservation.id.clone(),
        scheduler_status: "drained".to_string(),
        role: role.clone(),
        tier: tier.clone(),
    };
    let message = dispatch_message_to(&graph, &snapshot, &release.agent, Some(&meta));
    let dispatched_at = now_text();
    if let Some(node) = graph.nodes.get_mut(&node_id) {
        node.assignee.clone_from(&release.agent);
        node.role = Some(role);
        node.tier = Some(tier);
        node.scheduler_reservation_id = Some(reservation.id.clone());
        node.scheduler_status = Some("drained".to_string());
        node.status = "dispatched".to_string();
        node.message_id = Some(message.id.clone());
        node.dispatched_at.get_or_insert(dispatched_at);
    }
    graph.updated_at = now_text();
    upsert_graph(pool, &mut graph).await?;
    message_repo::insert_direct_message(pool, message.input).await?;
    Ok(())
}

fn apply_node_patch(
    node: &mut AgentChatTaskGraphNode,
    patch: UpdateAgentChatTaskGraphNode,
) -> Result<(), StoreError> {
    if patch.status.is_none() && patch.result.is_none() && patch.error.is_none() {
        return Err(StoreError::Invariant(
            "node patch requires status, result, or error".to_string(),
        ));
    }
    let now = now_text();
    if let Some(status) = patch.status {
        let status = required(status, "status required")?;
        validate_member("node status", &status, NODE_STATUSES)?;
        node.status.clone_from(&status);
        if status == "active" && node.started_at.is_none() {
            node.started_at = Some(now.clone());
        }
        if node_terminal(&status) && node.completed_at.is_none() {
            node.completed_at = Some(now.clone());
        }
    }
    if let Some(result) = patch.result {
        let bytes = serde_json::to_vec(&result)?;
        if bytes.len() > RESULT_MAX_BYTES {
            return Err(StoreError::Invariant(format!(
                "result exceeds max bytes ({RESULT_MAX_BYTES})"
            )));
        }
        node.result = Some(result);
    }
    if let Some(error) = patch.error {
        node.error = clean_text(Some(error));
    }
    Ok(())
}

async fn upsert_graph(
    pool: &SqlitePool,
    graph: &mut AgentChatTaskGraphRecord,
) -> Result<(), StoreError> {
    validate_member("graph status", &graph.status, GRAPH_STATUSES)?;
    validate_graph_nodes(&graph.nodes)?;
    let raw_json = serde_json::to_string(graph)?;
    let now = now_unix();
    if graph.record_version == 0 {
        let inserted = sqlx::query(
            "INSERT INTO agent_chat_task_graphs \
             (id, owner, label, status, raw_json, record_version, imported_at) \
             VALUES (?, ?, ?, ?, ?, 1, ?)",
        )
        .bind(&graph.id)
        .bind(&graph.owner)
        .bind(&graph.label)
        .bind(&graph.status)
        .bind(raw_json)
        .bind(now)
        .execute(pool)
        .await;
        if let Err(error) = inserted {
            // `create_graph` pre-checks the id, but two creators can clear that
            // check together and only collide here. A duplicate id is a caller
            // conflict (409), not an internal failure.
            if is_duplicate_graph_id(&error) {
                return Err(graph_already_exists(&graph.id));
            }
            return Err(error.into());
        }
        graph.record_version = 1;
        return Ok(());
    }
    let next_version = graph.record_version.saturating_add(1);
    let updated = sqlx::query(
        "UPDATE agent_chat_task_graphs SET owner = ?, label = ?, status = ?, raw_json = ?, \
         record_version = ?, imported_at = ? WHERE id = ? AND record_version = ?",
    )
    .bind(&graph.owner)
    .bind(&graph.label)
    .bind(&graph.status)
    .bind(raw_json)
    .bind(i64::try_from(next_version).unwrap_or(i64::MAX))
    .bind(now)
    .bind(&graph.id)
    .bind(i64::try_from(graph.record_version).unwrap_or(i64::MAX))
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(concurrent_write_conflict(&graph.id));
    }
    graph.record_version = next_version;
    Ok(())
}

fn graph_select_sql(tail: &str) -> String {
    format!(
        "SELECT id, owner, label, status, raw_json, record_version \
         FROM agent_chat_task_graphs {tail}"
    )
}

fn row_to_graph(row: &sqlx::sqlite::SqliteRow) -> Result<AgentChatTaskGraphRecord, StoreError> {
    let raw_json: String = row.get("raw_json");
    let mut graph: AgentChatTaskGraphRecord = serde_json::from_str(&raw_json)?;
    if graph.owner.is_empty()
        && let Ok(owner) = row.try_get::<Option<String>, _>("owner")
        && let Some(owner) = owner
    {
        graph.owner = owner;
    }
    if graph.label.is_empty()
        && let Ok(label) = row.try_get::<Option<String>, _>("label")
        && let Some(label) = label
    {
        graph.label = label;
    }
    if graph.status.is_empty()
        && let Ok(status) = row.try_get::<Option<String>, _>("status")
        && let Some(status) = status
    {
        graph.status = status;
    }
    graph.record_version = u64::try_from(row.get::<i64, _>("record_version")).unwrap_or(1);
    Ok(graph)
}

fn validate_graph_nodes(
    nodes: &BTreeMap<String, AgentChatTaskGraphNode>,
) -> Result<(), StoreError> {
    let ids = nodes.keys().cloned().collect::<BTreeSet<_>>();
    for (id, node) in nodes {
        if node.id != *id {
            return Err(StoreError::Invariant(format!(
                "node id mismatch: key '{id}' vs id '{}'",
                node.id
            )));
        }
        // Native execution wins over both routing inputs, so accepting either
        // alongside it would silently drop what the caller asked for.
        if node.role.is_some() && node.execution.is_some() {
            return Err(StoreError::Invariant(format!(
                "node '{id}' cannot set both role and execution"
            )));
        }
        if clean_text(Some(node.assignee.clone())).is_some() && node.execution.is_some() {
            return Err(StoreError::Invariant(format!(
                "node '{id}' cannot set both assignee and execution"
            )));
        }
        if clean_text(Some(node.assignee.clone())).is_none()
            && node.role.as_deref().and_then(clean_str).is_none()
            && node.execution.is_none()
        {
            return Err(StoreError::Invariant(format!(
                "node '{id}' assignee, role, or execution required"
            )));
        }
        if let Some(execution) = node.execution.as_ref() {
            // Re-validated on every write, so specs already stored are subject
            // to any future tightening of `NativeExecutionSpec::validate`.
            parse_execution_spec(id, execution)?;
        }
        required(node.description.clone(), "node description required")?;
        validate_member("node status", &node.status, NODE_STATUSES)?;
        for dep in &node.depends_on {
            if dep == id {
                return Err(StoreError::Invariant(format!(
                    "node '{id}' cannot depend on itself"
                )));
            }
            if !ids.contains(dep) {
                return Err(StoreError::Invariant(format!(
                    "node '{id}' depends on missing node '{dep}'"
                )));
            }
        }
    }
    ensure_acyclic(nodes)
}

fn ensure_acyclic(nodes: &BTreeMap<String, AgentChatTaskGraphNode>) -> Result<(), StoreError> {
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    for id in nodes.keys() {
        visit(id, nodes, &mut visiting, &mut visited)?;
    }
    Ok(())
}

fn visit(
    id: &str,
    nodes: &BTreeMap<String, AgentChatTaskGraphNode>,
    visiting: &mut BTreeSet<String>,
    visited: &mut BTreeSet<String>,
) -> Result<(), StoreError> {
    if visited.contains(id) {
        return Ok(());
    }
    if !visiting.insert(id.to_string()) {
        return Err(StoreError::Invariant(
            "dependency cycle detected".to_string(),
        ));
    }
    let Some(node) = nodes.get(id) else {
        return Ok(());
    };
    for dep in &node.depends_on {
        visit(dep, nodes, visiting, visited)?;
    }
    visiting.remove(id);
    visited.insert(id.to_string());
    Ok(())
}

fn condition_matches(
    graph: &AgentChatTaskGraphRecord,
    node: &AgentChatTaskGraphNode,
) -> Result<bool, StoreError> {
    let Some(condition) = node.condition.as_ref() else {
        return Ok(true);
    };
    let Some(object) = condition.as_object() else {
        return Ok(true);
    };
    let dep_id = object
        .get("dep")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let Some(dep) = graph.nodes.get(dep_id) else {
        return Ok(false);
    };
    let mut actual = dep.result.as_ref().unwrap_or(&Value::Null);
    if let Some(path) = object.get("path").and_then(Value::as_str) {
        for segment in path.split('.').filter(|segment| !segment.is_empty()) {
            if matches!(segment, "__proto__" | "constructor" | "prototype") {
                return Ok(false);
            }
            actual = actual.get(segment).unwrap_or(&Value::Null);
        }
    }
    if let Some(expected) = object.get("eq") {
        return Ok(actual == expected);
    }
    if let Some(expected) = object.get("neq") {
        return Ok(actual != expected);
    }
    if let Some(expected) = object.get("value") {
        match object.get("op").and_then(Value::as_str).unwrap_or("eq") {
            "eq" => return Ok(actual == expected),
            "neq" => return Ok(actual != expected),
            "in" => {
                return Ok(expected
                    .as_array()
                    .is_some_and(|values| values.iter().any(|value| value == actual)));
            }
            other => {
                return Err(StoreError::Invariant(format!(
                    "unsupported condition op: {other}"
                )));
            }
        }
    }
    if let Some(values) = object.get("in").and_then(Value::as_array) {
        return Ok(values.iter().any(|value| value == actual));
    }
    Ok(true)
}

fn node_terminal(status: &str) -> bool {
    matches!(status, "complete" | "failed" | "skipped" | "cancelled")
}

const MAX_GRAPH_WRITE_ATTEMPTS: usize = 8;
const CONCURRENT_WRITE_SUFFIX: &str = "changed concurrently";

fn concurrent_write_conflict(graph_id: &str) -> StoreError {
    StoreError::Conflict(format!("task graph '{graph_id}' {CONCURRENT_WRITE_SUFFIX}"))
}

fn is_concurrent_write_conflict(error: &StoreError) -> bool {
    matches!(error, StoreError::Conflict(message) if message.ends_with(CONCURRENT_WRITE_SUFFIX))
}

fn graph_already_exists(graph_id: &str) -> StoreError {
    StoreError::Conflict(format!("task graph already exists: {graph_id}"))
}

fn is_duplicate_graph_id(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
}

fn validate_member(field: &str, value: &str, allowed: &[&str]) -> Result<(), StoreError> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(StoreError::Invariant(format!("invalid {field}: {value}")))
}

fn required(value: String, message: &str) -> Result<String, StoreError> {
    clean_text(Some(value)).ok_or_else(|| StoreError::Invariant(message.to_string()))
}

fn clean_text(value: Option<String>) -> Option<String> {
    let trimmed = value?.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn clean_str(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn normalize_list(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        let Some(value) = clean_text(Some(value)) else {
            continue;
        };
        if !out.contains(&value) {
            out.push(value);
        }
    }
    out
}

fn generate_graph_id() -> String {
    let ts = now_unix();
    let seq = NEXT_GRAPH_ID.fetch_add(1, Ordering::Relaxed);
    format!("graph_{ts}_{seq:06x}")
}

fn dispatch_message_id(graph_id: &str, node_id: &str) -> String {
    format!(
        "msg_task_graph_dispatch_{}_{}",
        sanitize_id(graph_id),
        sanitize_id(node_id)
    )
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn now_text() -> String {
    format!("{}Z", now_unix_ms())
}
