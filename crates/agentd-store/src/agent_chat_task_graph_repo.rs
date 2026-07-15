//! Live agent-chat-compatible task-graph operations.
//!
//! p225 preserved imported `task_graphs.json` snapshots in
//! `agent_chat_task_graphs.raw_json`; p227 makes that compatibility table live.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

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
        return Err(StoreError::Conflict(format!(
            "task graph already exists: {id}"
        )));
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
                message_id: None,
                started_at: None,
                dispatched_at: None,
                completed_at: None,
            },
        );
    }
    validate_graph_nodes(&nodes)?;
    let now = now_text();
    let graph = AgentChatTaskGraphRecord {
        id,
        owner,
        label,
        status: "active".to_string(),
        nodes,
        created_at: now.clone(),
        updated_at: now,
        completed_at: None,
    };
    upsert_graph(pool, &graph).await?;
    Ok(graph)
}

pub async fn list_graphs(
    pool: &SqlitePool,
    status: Option<&str>,
) -> Result<Vec<AgentChatTaskGraphRecord>, StoreError> {
    let rows = sqlx::query(graph_select_sql("ORDER BY rowid"))
        .fetch_all(pool)
        .await?;
    let status = status.and_then(|value| clean_text(Some(value.to_string())));
    let graphs = rows
        .iter()
        .map(row_to_graph)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
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
    let row = sqlx::query(graph_select_sql("WHERE id = ?"))
        .bind(&id)
        .fetch_optional(pool)
        .await?;
    row.map(|row| row_to_graph(&row)).transpose()
}

pub async fn advance_graph(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<AgentChatTaskGraphRecord>, StoreError> {
    let Some(graph) = get_graph(pool, id).await? else {
        return Ok(None);
    };
    advance_graph_record(pool, graph).await.map(Some)
}

pub async fn update_node_and_advance(
    pool: &SqlitePool,
    graph_id: &str,
    node_id: &str,
    patch: UpdateAgentChatTaskGraphNode,
) -> Result<Option<(AgentChatTaskGraphRecord, AgentChatTaskGraphNode)>, StoreError> {
    let Some(mut graph) = get_graph(pool, graph_id).await? else {
        return Ok(None);
    };
    let Some(node) = graph.nodes.get_mut(node_id) else {
        return Ok(None);
    };
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
    let mut graph = advance_graph_record(pool, graph).await?;
    if let Some(agent) = release_agent {
        release_and_drain_scheduler(pool, &agent).await?;
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
    upsert_graph(pool, &graph).await?;
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

#[allow(clippy::too_many_lines)]
async fn advance_graph_record(
    pool: &SqlitePool,
    mut graph: AgentChatTaskGraphRecord,
) -> Result<AgentChatTaskGraphRecord, StoreError> {
    if graph.status != "active" {
        upsert_graph(pool, &graph).await?;
        return Ok(graph);
    }
    let mut changed = false;
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
            if snapshot.role.as_deref().and_then(clean_str).is_some() {
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
    }
    upsert_graph(pool, &graph).await?;
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
        dispatch_drained_task_graph_ticket(pool, release).await?;
    }
    Ok(())
}

async fn dispatch_drained_task_graph_ticket(
    pool: &SqlitePool,
    release: agent_scheduler_repo::ReleaseResult,
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
    upsert_graph(pool, &graph).await?;
    message_repo::insert_direct_message(pool, message.input).await?;
    Ok(())
}

fn apply_node_patch(
    node: &mut AgentChatTaskGraphNode,
    patch: UpdateAgentChatTaskGraphNode,
) -> Result<(), StoreError> {
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
    graph: &AgentChatTaskGraphRecord,
) -> Result<(), StoreError> {
    validate_member("graph status", &graph.status, GRAPH_STATUSES)?;
    validate_graph_nodes(&graph.nodes)?;
    let raw_json = serde_json::to_string(graph)?;
    sqlx::query(
        "INSERT INTO agent_chat_task_graphs \
         (id, owner, label, status, raw_json, imported_at) VALUES (?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
          owner = excluded.owner, \
          label = excluded.label, \
          status = excluded.status, \
          raw_json = excluded.raw_json, \
          imported_at = excluded.imported_at",
    )
    .bind(&graph.id)
    .bind(&graph.owner)
    .bind(&graph.label)
    .bind(&graph.status)
    .bind(raw_json)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

fn graph_select_sql(tail: &'static str) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(format!(
        "SELECT id, owner, label, status, raw_json FROM agent_chat_task_graphs {tail}"
    ))
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
        if clean_text(Some(node.assignee.clone())).is_none()
            && node.role.as_deref().and_then(clean_str).is_none()
        {
            return Err(StoreError::Invariant(format!(
                "node '{id}' assignee or role required"
            )));
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
