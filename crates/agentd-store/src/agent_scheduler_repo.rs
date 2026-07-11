//! Durable pool scheduler operations for the agent-chat replacement surface.
//!
//! Agent-chat kept dispatch busy/queue state in memory. Agentd persists the
//! same scheduler decisions so daemon restarts do not forget active
//! reservations or queued tickets.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, SqlitePool};

use crate::agent_repo::{self, AgentRecord};
use crate::error::StoreError;
use crate::util::now_unix;

const PROVISION_STATUS: &str = "provision";

#[derive(Debug, Clone, Copy, Default)]
pub struct SchedulerConfig {
    pub max_per_cell: i64,
}

#[derive(Debug, Clone, Default)]
pub struct PoolFilters {
    pub role: Option<String>,
    pub capability: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PoolAgent {
    pub name: String,
    pub role: Option<String>,
    pub capability: String,
    pub online: bool,
    pub busy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PoolSnapshot {
    pub grid: BTreeMap<String, BTreeMap<String, Vec<PoolAgent>>>,
    pub counts: BTreeMap<String, BTreeMap<String, usize>>,
    pub total: usize,
    pub agents: Vec<PoolAgent>,
}

#[derive(Debug, Clone)]
pub struct DispatchRequest {
    pub role: String,
    pub capability: Option<String>,
    pub task: Option<Value>,
    pub room: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReleaseRequest {
    pub agent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SchedulerReservation {
    pub id: String,
    pub role: String,
    pub tier: String,
    pub agent: Option<String>,
    pub provisioned_name: Option<String>,
    pub status: String,
    pub task: Option<Value>,
    pub room: Option<String>,
    pub runtime: Value,
    pub ticket: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub released_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DispatchResult {
    pub status: String,
    pub role: String,
    pub tier: String,
    pub agent: Option<String>,
    pub reservation: Option<SchedulerReservation>,
    pub ticket: Option<String>,
    pub queue_depth: Option<usize>,
    pub name: Option<String>,
    pub runtime: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReleaseResult {
    pub status: String,
    pub agent: String,
    pub reservation: Option<SchedulerReservation>,
    pub ticket: Option<String>,
    pub role: Option<String>,
    pub tier: Option<String>,
    pub task: Option<Value>,
    pub room: Option<String>,
}

pub async fn pool_snapshot(
    pool: &SqlitePool,
    filters: PoolFilters,
) -> Result<PoolSnapshot, StoreError> {
    let role_filter = filters.role.as_deref().and_then(|v| clean_opt(v, 64));
    let capability_filter = filters.capability.and_then(|v| clean_tier(&v));
    let state_filter = filters
        .state
        .as_deref()
        .and_then(|v| clean_opt(v, 16))
        .unwrap_or_else(|| "any".to_string())
        .to_ascii_lowercase();
    let busy = active_busy_agents(pool).await?;
    let mut agents = agent_repo::list_agents(pool)
        .await?
        .into_iter()
        .map(|agent| pool_agent(agent, &busy))
        .filter(|agent| {
            role_filter
                .as_deref()
                .is_none_or(|role| agent.role.as_deref() == Some(role))
        })
        .filter(|agent| {
            capability_filter
                .as_deref()
                .is_none_or(|tier| agent.capability == tier)
        })
        .filter(|agent| match state_filter.as_str() {
            "idle" => agent.online && !agent.busy,
            "busy" => agent.busy,
            _ => true,
        })
        .collect::<Vec<_>>();
    agents.sort_by(|a, b| a.name.cmp(&b.name));

    let mut grid: BTreeMap<String, BTreeMap<String, Vec<PoolAgent>>> = BTreeMap::new();
    for agent in &agents {
        if let Some(role) = agent.role.as_deref() {
            grid.entry(role.to_string())
                .or_default()
                .entry(agent.capability.clone())
                .or_default()
                .push(agent.clone());
        }
    }
    let counts = grid
        .iter()
        .map(|(role, by_tier)| {
            (
                role.clone(),
                by_tier
                    .iter()
                    .map(|(tier, agents)| (tier.clone(), agents.len()))
                    .collect::<BTreeMap<_, _>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    Ok(PoolSnapshot {
        total: agents.len(),
        grid,
        counts,
        agents,
    })
}

pub async fn queue_status_count(pool: &SqlitePool, status: &str) -> Result<i64, StoreError> {
    let status = clean_required(status, 16, "status required")?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_scheduler_queue WHERE status = ?")
            .bind(status)
            .fetch_one(pool)
            .await?;
    Ok(count)
}

pub async fn dispatch(
    pool: &SqlitePool,
    input: DispatchRequest,
    config: SchedulerConfig,
) -> Result<DispatchResult, StoreError> {
    let role = clean_required(&input.role, 64, "role required")?;
    let tier = resolve_tier(&role, input.capability.as_deref());
    let busy = active_busy_agents(pool).await?;
    let agents = agent_repo::list_agents(pool).await?;
    if let Some(agent) = select_agent(&agents, &busy, &role, &tier) {
        let reservation = insert_reservation(
            pool,
            ReservationInsert {
                role: role.clone(),
                tier: tier.clone(),
                agent: Some(agent.name.clone()),
                provisioned_name: None,
                status: "routed".to_string(),
                task: input.task,
                room: input.room,
                runtime: json!({}),
                ticket: None,
            },
        )
        .await?;
        return Ok(DispatchResult {
            status: "routed".to_string(),
            role,
            tier,
            agent: Some(agent.name),
            reservation: Some(reservation),
            ticket: None,
            queue_depth: None,
            name: None,
            runtime: json!({}),
        });
    }

    if config.max_per_cell > 0 {
        let in_cell = i64::try_from(
            agents
                .iter()
                .filter(|agent| {
                    effective_role(agent).as_deref() == Some(role.as_str())
                        && effective_capability(agent) == tier
                })
                .count(),
        )
        .unwrap_or(i64::MAX);
        let provisioned = active_provision_count(pool, &role, &tier).await?;
        if in_cell + provisioned < config.max_per_cell {
            let name = next_provision_name(pool, &role, &tier).await?;
            let runtime = tier_runtime(&tier);
            let reservation = insert_reservation(
                pool,
                ReservationInsert {
                    role: role.clone(),
                    tier: tier.clone(),
                    agent: None,
                    provisioned_name: Some(name.clone()),
                    status: PROVISION_STATUS.to_string(),
                    task: input.task,
                    room: input.room,
                    runtime: runtime.clone(),
                    ticket: None,
                },
            )
            .await?;
            return Ok(DispatchResult {
                status: "provision".to_string(),
                role,
                tier,
                agent: None,
                reservation: Some(reservation),
                ticket: None,
                queue_depth: None,
                name: Some(name),
                runtime,
            });
        }
    }

    let ticket = next_ticket(pool).await?;
    let queue_depth = insert_queue(pool, &ticket, &role, &tier, input.task, input.room).await?;
    Ok(DispatchResult {
        status: "queued".to_string(),
        role,
        tier,
        agent: None,
        reservation: None,
        ticket: Some(ticket),
        queue_depth: Some(queue_depth),
        name: None,
        runtime: json!({}),
    })
}

pub async fn release(
    pool: &SqlitePool,
    input: ReleaseRequest,
) -> Result<ReleaseResult, StoreError> {
    let agent_name = clean_required(&input.agent, 128, "agent required")?;
    let now = now_unix();
    let active = active_reservation_for_agent(pool, &agent_name).await?;
    if let Some(reservation) = active.as_ref() {
        sqlx::query(
            "UPDATE agent_scheduler_reservations \
             SET status = 'released', released_at = ?, updated_at = ? WHERE id = ?",
        )
        .bind(now)
        .bind(now)
        .bind(&reservation.id)
        .execute(pool)
        .await?;
    }

    let Some(agent) = agent_repo::get_agent(pool, &agent_name).await? else {
        return Ok(ReleaseResult {
            status: "released".to_string(),
            agent: agent_name,
            reservation: None,
            ticket: None,
            role: None,
            tier: None,
            task: None,
            room: None,
        });
    };
    let Some(role) = effective_role(&agent) else {
        return Ok(released_result(agent_name));
    };
    let tier = effective_capability(&agent);
    let Some(ticket) = next_queued_ticket(pool, &role, &tier).await? else {
        return Ok(released_result(agent_name));
    };
    let task = ticket.task.clone();
    let room = ticket.room.clone();
    let reservation = insert_reservation(
        pool,
        ReservationInsert {
            role: role.clone(),
            tier: tier.clone(),
            agent: Some(agent_name.clone()),
            provisioned_name: None,
            status: "drained".to_string(),
            task: task.clone(),
            room: room.clone(),
            runtime: json!({}),
            ticket: Some(ticket.ticket.clone()),
        },
    )
    .await?;
    sqlx::query(
        "UPDATE agent_scheduler_queue \
         SET status = 'drained', drained_at = ?, updated_at = ?, reservation_id = ? \
         WHERE ticket = ?",
    )
    .bind(now)
    .bind(now)
    .bind(&reservation.id)
    .bind(&ticket.ticket)
    .execute(pool)
    .await?;

    Ok(ReleaseResult {
        status: "drained".to_string(),
        agent: agent_name,
        reservation: Some(reservation),
        ticket: Some(ticket.ticket),
        role: Some(role),
        tier: Some(tier),
        task,
        room,
    })
}

fn released_result(agent: String) -> ReleaseResult {
    ReleaseResult {
        status: "released".to_string(),
        agent,
        reservation: None,
        ticket: None,
        role: None,
        tier: None,
        task: None,
        room: None,
    }
}

#[derive(Debug, Clone)]
struct ReservationInsert {
    role: String,
    tier: String,
    agent: Option<String>,
    provisioned_name: Option<String>,
    status: String,
    task: Option<Value>,
    room: Option<String>,
    runtime: Value,
    ticket: Option<String>,
}

#[derive(Debug, Clone)]
struct QueueTicket {
    ticket: String,
    task: Option<Value>,
    room: Option<String>,
}

async fn insert_reservation(
    pool: &SqlitePool,
    input: ReservationInsert,
) -> Result<SchedulerReservation, StoreError> {
    let id = next_reservation_id(pool).await?;
    let now = now_unix();
    let task_json = optional_json_text(input.task.as_ref())?;
    let runtime_json = serde_json::to_string(&input.runtime)?;
    sqlx::query(
        "INSERT INTO agent_scheduler_reservations \
         (id, role, tier, agent, provisioned_name, status, task_json, room, runtime_json, \
          ticket, created_at, updated_at, released_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)",
    )
    .bind(&id)
    .bind(&input.role)
    .bind(&input.tier)
    .bind(input.agent.as_deref())
    .bind(input.provisioned_name.as_deref())
    .bind(&input.status)
    .bind(task_json.as_deref())
    .bind(input.room.as_deref())
    .bind(&runtime_json)
    .bind(input.ticket.as_deref())
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    get_reservation(pool, &id)
        .await?
        .ok_or_else(|| StoreError::Invariant(format!("reservation '{id}' missing after insert")))
}

async fn insert_queue(
    pool: &SqlitePool,
    ticket: &str,
    role: &str,
    tier: &str,
    task: Option<Value>,
    room: Option<String>,
) -> Result<usize, StoreError> {
    let now = now_unix();
    let task_json = optional_json_text(task.as_ref())?;
    sqlx::query(
        "INSERT INTO agent_scheduler_queue \
         (ticket, role, tier, task_json, room, status, created_at, updated_at, drained_at, reservation_id) \
         VALUES (?, ?, ?, ?, ?, 'queued', ?, ?, NULL, NULL)",
    )
    .bind(ticket)
    .bind(role)
    .bind(tier)
    .bind(task_json.as_deref())
    .bind(room.as_deref())
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    let depth: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_scheduler_queue \
         WHERE role = ? AND tier = ? AND status = 'queued'",
    )
    .bind(role)
    .bind(tier)
    .fetch_one(pool)
    .await?;
    Ok(usize::try_from(depth).unwrap_or(usize::MAX))
}

async fn get_reservation(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<SchedulerReservation>, StoreError> {
    let row = sqlx::query(
        "SELECT id, role, tier, agent, provisioned_name, status, task_json, room, \
         runtime_json, ticket, created_at, updated_at, released_at \
         FROM agent_scheduler_reservations WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| reservation_from_row(&row)).transpose()
}

async fn active_reservation_for_agent(
    pool: &SqlitePool,
    agent: &str,
) -> Result<Option<SchedulerReservation>, StoreError> {
    let row = sqlx::query(
        "SELECT id, role, tier, agent, provisioned_name, status, task_json, room, \
         runtime_json, ticket, created_at, updated_at, released_at \
         FROM agent_scheduler_reservations \
         WHERE agent = ? AND status IN ('routed', 'drained') \
         ORDER BY created_at ASC, id ASC LIMIT 1",
    )
    .bind(agent)
    .fetch_optional(pool)
    .await?;
    row.map(|row| reservation_from_row(&row)).transpose()
}

async fn next_queued_ticket(
    pool: &SqlitePool,
    role: &str,
    tier: &str,
) -> Result<Option<QueueTicket>, StoreError> {
    let row = sqlx::query(
        "SELECT ticket, task_json, room FROM agent_scheduler_queue \
         WHERE role = ? AND tier = ? AND status = 'queued' \
         ORDER BY created_at ASC, ticket ASC LIMIT 1",
    )
    .bind(role)
    .bind(tier)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        let task_json: Option<String> = row.get("task_json");
        Ok(QueueTicket {
            ticket: row.get("ticket"),
            task: parse_optional_json(task_json.as_deref())?,
            room: row.get("room"),
        })
    })
    .transpose()
}

fn reservation_from_row(row: &sqlx::sqlite::SqliteRow) -> Result<SchedulerReservation, StoreError> {
    let task_json: Option<String> = row.get("task_json");
    let runtime_json: Option<String> = row.get("runtime_json");
    Ok(SchedulerReservation {
        id: row.get("id"),
        role: row.get("role"),
        tier: row.get("tier"),
        agent: row.get("agent"),
        provisioned_name: row.get("provisioned_name"),
        status: row.get("status"),
        task: parse_optional_json(task_json.as_deref())?,
        room: row.get("room"),
        runtime: runtime_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_else(|| json!({})),
        ticket: row.get("ticket"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        released_at: row.get("released_at"),
    })
}

async fn active_busy_agents(pool: &SqlitePool) -> Result<BTreeSet<String>, StoreError> {
    let rows = sqlx::query(
        "SELECT DISTINCT agent FROM agent_scheduler_reservations \
         WHERE agent IS NOT NULL AND status IN ('routed', 'drained')",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .filter_map(|row| row.get::<Option<String>, _>("agent"))
        .collect())
}

async fn active_provision_count(
    pool: &SqlitePool,
    role: &str,
    tier: &str,
) -> Result<i64, StoreError> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_scheduler_reservations \
         WHERE role = ? AND tier = ? AND status = ?",
    )
    .bind(role)
    .bind(tier)
    .bind(PROVISION_STATUS)
    .fetch_one(pool)
    .await?)
}

async fn next_reservation_id(pool: &SqlitePool) -> Result<String, StoreError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_scheduler_reservations")
        .fetch_one(pool)
        .await?;
    Ok(format!("sched_res_{}_{}", now_unix(), count + 1))
}

async fn next_ticket(pool: &SqlitePool) -> Result<String, StoreError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_scheduler_queue")
        .fetch_one(pool)
        .await?;
    Ok(format!("disp-{}-{}", now_unix(), count + 1))
}

async fn next_provision_name(
    pool: &SqlitePool,
    role: &str,
    tier: &str,
) -> Result<String, StoreError> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_scheduler_reservations")
        .fetch_one(pool)
        .await?;
    Ok(format!("mx_{role}_{tier}_{count}"))
}

fn select_agent(
    agents: &[AgentRecord],
    busy: &BTreeSet<String>,
    role: &str,
    tier: &str,
) -> Option<AgentRecord> {
    let mut eligible = agents
        .iter()
        .filter(|agent| agent.status == "online")
        .filter(|agent| !busy.contains(&agent.name))
        .filter(|agent| effective_role(agent).as_deref() == Some(role))
        .filter(|agent| tier_rank(&effective_capability(agent)) >= tier_rank(tier))
        .cloned()
        .collect::<Vec<_>>();
    eligible.sort_by(|a, b| {
        tier_rank(&effective_capability(a))
            .cmp(&tier_rank(&effective_capability(b)))
            .then_with(|| a.name.cmp(&b.name))
    });
    eligible.into_iter().next()
}

fn pool_agent(agent: AgentRecord, busy: &BTreeSet<String>) -> PoolAgent {
    let role = effective_role(&agent);
    let capability = effective_capability(&agent);
    PoolAgent {
        busy: busy.contains(&agent.name),
        online: agent.status == "online",
        name: agent.name,
        role,
        capability,
    }
}

fn effective_role(agent: &AgentRecord) -> Option<String> {
    agent
        .role
        .as_deref()
        .and_then(|role| clean_opt(role, 64))
        .filter(|role| role != "agent")
        .or_else(|| canonical_role(&agent.name))
}

fn effective_capability(agent: &AgentRecord) -> String {
    agent
        .capability
        .as_deref()
        .and_then(clean_tier)
        .or_else(|| effective_role(agent).map(|role| default_tier(&role).to_string()))
        .unwrap_or_else(|| "medium".to_string())
}

fn canonical_role(name: &str) -> Option<String> {
    let lower = name.to_ascii_lowercase();
    if lower.contains("architect") || lower.contains("coordinator") {
        return Some("architect".to_string());
    }
    if lower.contains("final_reviewer") || lower.contains("final-reviewer") {
        return Some("review".to_string());
    }
    if lower.contains("review") {
        return Some("review".to_string());
    }
    if lower.contains("test") || lower.contains("qa") {
        return Some("testing".to_string());
    }
    if lower.contains("integrat") {
        return Some("integration".to_string());
    }
    if lower.contains("doc") {
        return Some("documentation".to_string());
    }
    if lower.contains("implement") || lower.contains("coder") || lower.contains("coding") {
        return Some("coding".to_string());
    }
    None
}

fn resolve_tier(role: &str, requested: Option<&str>) -> String {
    requested
        .and_then(clean_tier)
        .unwrap_or_else(|| default_tier(role).to_string())
}

fn default_tier(role: &str) -> &'static str {
    match role {
        "architect" | "review" => "strong",
        "documentation" => "lightweight",
        _ => "medium",
    }
}

fn tier_rank(tier: &str) -> i32 {
    match tier {
        "strong" => 2,
        "lightweight" => 0,
        _ => 1,
    }
}

fn tier_runtime(tier: &str) -> Value {
    match tier {
        "strong" => json!({"runtime": "claude", "model": "opus"}),
        "lightweight" => json!({"runtime": "claude", "model": "haiku"}),
        _ => json!({"runtime": "claude", "model": "sonnet"}),
    }
}

fn clean_tier(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "strong" => Some("strong".to_string()),
        "medium" => Some("medium".to_string()),
        "lightweight" => Some("lightweight".to_string()),
        _ => None,
    }
}

fn clean_required(value: &str, limit: usize, message: &str) -> Result<String, StoreError> {
    clean_opt(value, limit).ok_or_else(|| StoreError::Invariant(message.to_string()))
}

fn clean_opt(value: &str, limit: usize) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.chars().take(limit).collect())
}

fn optional_json_text(value: Option<&Value>) -> Result<Option<String>, StoreError> {
    value
        .map(serde_json::to_string)
        .transpose()
        .map_err(Into::into)
}

fn parse_optional_json(text: Option<&str>) -> Result<Option<Value>, StoreError> {
    text.map(serde_json::from_str)
        .transpose()
        .map_err(Into::into)
}
