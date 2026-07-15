//! Live agent-chat-compatible product task operations.
//!
//! These rows are coordination/product state, not workflow execution
//! `task_runs`. p225 introduced the compatibility table for import/shadow; p226
//! makes the same rows live through `/api/tasks`.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::{now_unix, now_unix_ms};

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

const STATUSES: &[&str] = &["created", "accepted", "in_progress", "blocked", "done"];
const PRIORITIES: &[&str] = &["p0", "p1", "p2", "p3"];
const GRANULARITIES: &[&str] = &["epic", "task", "subtask"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentChatTaskComment {
    pub author: String,
    pub text: String,
    pub ts: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatTaskRecord {
    pub id: String,
    pub title: String,
    pub description: String,
    pub status: String,
    pub priority: String,
    pub granularity: String,
    pub assignee: Option<String>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub heartbeat_at: Option<String>,
    pub waiting_reason: Option<String>,
    pub waiting_until: Option<String>,
    pub parent_id: Option<String>,
    pub labels: Vec<String>,
    pub health: Option<Value>,
    pub comments: Vec<AgentChatTaskComment>,
}

#[derive(Debug, Clone)]
pub struct CreateAgentChatTask {
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub granularity: Option<String>,
    pub assignee: Option<String>,
    pub created_by: Option<String>,
    pub parent_id: Option<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateAgentChatTask {
    pub title: Option<String>,
    pub description: Option<String>,
    pub priority: Option<String>,
    pub granularity: Option<String>,
    pub assignee: Option<Option<String>>,
    pub labels: Option<Vec<String>>,
    pub parent_id: Option<Option<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateAgentChatTaskExecution {
    pub heartbeat_at: Option<bool>,
    pub waiting_reason: Option<Option<String>>,
    pub waiting_until: Option<Option<String>>,
}

#[derive(Debug, Clone)]
pub struct TransitionAgentChatTask {
    pub status: String,
    pub waiting_reason: Option<String>,
    pub waiting_until: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AddAgentChatTaskComment {
    pub author: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct AgentChatTaskFilters {
    pub assignee: Option<String>,
    pub statuses: Vec<String>,
    pub priority: Option<String>,
    pub label: Option<String>,
    pub offset: usize,
    pub limit: Option<usize>,
}

pub async fn create_task(
    pool: &SqlitePool,
    input: CreateAgentChatTask,
) -> Result<AgentChatTaskRecord, StoreError> {
    let title = normalize_required(&input.title, 255, "title is required")?;
    let description = clean_text(input.description.as_deref(), 4096).unwrap_or_default();
    let priority = clean_text(input.priority.as_deref(), 8).unwrap_or_else(|| "p2".to_string());
    validate_member("priority", &priority, PRIORITIES)?;
    let granularity =
        clean_text(input.granularity.as_deref(), 16).unwrap_or_else(|| "task".to_string());
    validate_member("granularity", &granularity, GRANULARITIES)?;
    let assignee = clean_text(input.assignee.as_deref(), 128);
    let created_by = clean_text(input.created_by.as_deref(), 128);
    let parent_id = clean_text(input.parent_id.as_deref(), 64);
    ensure_parent_exists(pool, parent_id.as_deref()).await?;

    let now = now_text();
    let mut last_collision = false;
    for _ in 0..10 {
        let id = generate_task_id();
        if get_task(pool, &id).await?.is_some() {
            last_collision = true;
            continue;
        }
        let task = AgentChatTaskRecord {
            id,
            title,
            description,
            status: "created".to_string(),
            priority,
            granularity,
            assignee,
            created_by,
            created_at: now.clone(),
            updated_at: now,
            started_at: None,
            completed_at: None,
            heartbeat_at: None,
            waiting_reason: None,
            waiting_until: None,
            parent_id,
            labels: normalize_labels(input.labels),
            health: None,
            comments: Vec::new(),
        };
        upsert_task(pool, &task).await?;
        return Ok(task);
    }
    if last_collision {
        return Err(StoreError::Invariant(
            "failed to generate unique task id".to_string(),
        ));
    }
    unreachable!("task id loop always returns or reports a collision")
}

pub async fn get_task(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<AgentChatTaskRecord>, StoreError> {
    let row = sqlx::query(task_select_sql("WHERE id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.map(|row| row_to_task(&row)).transpose()
}

pub async fn list_tasks(
    pool: &SqlitePool,
    filters: AgentChatTaskFilters,
) -> Result<Vec<AgentChatTaskRecord>, StoreError> {
    let rows = sqlx::query(task_select_sql("ORDER BY rowid"))
        .fetch_all(pool)
        .await?;
    let statuses = filters
        .statuses
        .into_iter()
        .filter_map(|status| clean_text(Some(&status), 64))
        .collect::<Vec<_>>();
    let mut tasks = rows
        .iter()
        .map(row_to_task)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|task| {
            filters
                .assignee
                .as_deref()
                .is_none_or(|assignee| task.assignee.as_deref() == Some(assignee))
        })
        .filter(|task| statuses.is_empty() || statuses.iter().any(|status| status == &task.status))
        .filter(|task| {
            filters
                .priority
                .as_deref()
                .is_none_or(|priority| task.priority == priority)
        })
        .filter(|task| {
            filters
                .label
                .as_deref()
                .is_none_or(|label| task.labels.iter().any(|existing| existing == label))
        })
        .collect::<Vec<_>>();
    if filters.offset > 0 {
        tasks = tasks.into_iter().skip(filters.offset).collect();
    }
    if let Some(limit) = filters.limit {
        tasks.truncate(limit);
    }
    Ok(tasks)
}

pub async fn update_task(
    pool: &SqlitePool,
    id: &str,
    patch: UpdateAgentChatTask,
) -> Result<Option<AgentChatTaskRecord>, StoreError> {
    let Some(mut task) = get_task(pool, id).await? else {
        return Ok(None);
    };
    let mut changed = false;

    if let Some(title) = patch.title {
        let title = normalize_required(&title, 255, "title is required")?;
        if task.title != title {
            task.title = title;
            changed = true;
        }
    }
    if let Some(description) = patch.description {
        let description = clean_text(Some(&description), 4096).unwrap_or_default();
        if task.description != description {
            task.description = description;
            changed = true;
        }
    }
    if let Some(priority) = patch.priority {
        let priority = clean_text(Some(&priority), 8)
            .ok_or_else(|| StoreError::Invariant("invalid priority: ".to_string()))?;
        validate_member("priority", &priority, PRIORITIES)?;
        if task.priority != priority {
            task.priority = priority;
            changed = true;
        }
    }
    if let Some(granularity) = patch.granularity {
        let granularity = clean_text(Some(&granularity), 16)
            .ok_or_else(|| StoreError::Invariant("invalid granularity: ".to_string()))?;
        validate_member("granularity", &granularity, GRANULARITIES)?;
        if task.granularity != granularity {
            task.granularity = granularity;
            changed = true;
        }
    }
    if let Some(assignee) = patch.assignee {
        let assignee = assignee.and_then(|value| clean_text(Some(&value), 128));
        if task.assignee != assignee {
            task.assignee = assignee;
            changed = true;
        }
    }
    if let Some(labels) = patch.labels {
        let labels = normalize_labels(labels);
        if task.labels != labels {
            task.labels = labels;
            changed = true;
        }
    }
    if let Some(parent_id) = patch.parent_id {
        let parent_id = parent_id.and_then(|value| clean_text(Some(&value), 64));
        ensure_parent_exists(pool, parent_id.as_deref()).await?;
        if task.parent_id != parent_id {
            task.parent_id = parent_id;
            changed = true;
        }
    }
    if changed {
        task.updated_at = now_text();
        upsert_task(pool, &task).await?;
    }
    Ok(Some(task))
}

pub async fn update_task_execution(
    pool: &SqlitePool,
    id: &str,
    patch: UpdateAgentChatTaskExecution,
) -> Result<Option<AgentChatTaskRecord>, StoreError> {
    let Some(mut task) = get_task(pool, id).await? else {
        return Ok(None);
    };
    let mut changed = false;
    if patch.heartbeat_at.is_some() {
        task.heartbeat_at = Some(now_text());
        changed = true;
    }
    if let Some(waiting_reason) = patch.waiting_reason {
        let waiting_reason = waiting_reason.and_then(|value| clean_text(Some(&value), 1024));
        task.waiting_reason = waiting_reason;
        changed = true;
    }
    if let Some(waiting_until) = patch.waiting_until {
        let waiting_until = waiting_until.and_then(|value| clean_text(Some(&value), 64));
        task.waiting_until = waiting_until;
        changed = true;
    }
    if changed {
        task.updated_at = now_text();
        upsert_task(pool, &task).await?;
    }
    Ok(Some(task))
}

pub async fn transition_task(
    pool: &SqlitePool,
    id: &str,
    input: TransitionAgentChatTask,
) -> Result<Option<AgentChatTaskRecord>, StoreError> {
    let Some(mut task) = get_task(pool, id).await? else {
        return Ok(None);
    };
    let new_status = normalize_required(&input.status, 64, "status is required")?;
    validate_status(&new_status)?;
    if !transition_allowed(&task.status, &new_status) {
        return Err(StoreError::Invariant(format!(
            "cannot transition from '{}' to '{}'",
            task.status, new_status
        )));
    }
    let blocked_reason = clean_text(input.waiting_reason.as_deref(), 1024);
    let blocked_until = clean_text(input.waiting_until.as_deref(), 64);
    if new_status == "blocked" {
        if blocked_reason.is_none() {
            return Err(StoreError::Invariant(
                "waiting_reason is required when transitioning to blocked".to_string(),
            ));
        }
        if blocked_until.is_none() {
            return Err(StoreError::Invariant(
                "waiting_until is required when transitioning to blocked".to_string(),
            ));
        }
    }

    let now = now_text();
    task.status = new_status.clone();
    task.updated_at = now.clone();
    if (new_status == "accepted" || new_status == "in_progress") && task.started_at.is_none() {
        task.started_at = Some(now.clone());
    }
    if new_status == "done" {
        task.completed_at = Some(now);
        task.waiting_reason = None;
        task.waiting_until = None;
    } else if new_status == "blocked" {
        task.waiting_reason = blocked_reason;
        task.waiting_until = blocked_until;
    } else if new_status == "in_progress" {
        task.waiting_reason = None;
        task.waiting_until = None;
    }
    upsert_task(pool, &task).await?;
    Ok(Some(task))
}

pub async fn add_comment(
    pool: &SqlitePool,
    id: &str,
    input: AddAgentChatTaskComment,
) -> Result<Option<AgentChatTaskRecord>, StoreError> {
    let Some(mut task) = get_task(pool, id).await? else {
        return Ok(None);
    };
    if task.comments.len() >= 100 {
        return Err(StoreError::Invariant(
            "max 100 comments per task".to_string(),
        ));
    }
    let text = normalize_required(&input.text, 4096, "comment text is required")?;
    let author =
        clean_text(input.author.as_deref(), 128).unwrap_or_else(|| "anonymous".to_string());
    task.comments.push(AgentChatTaskComment {
        author,
        text,
        ts: now_text(),
    });
    task.updated_at = now_text();
    upsert_task(pool, &task).await?;
    Ok(Some(task))
}

pub async fn delete_task(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<AgentChatTaskRecord>, StoreError> {
    let Some(task) = get_task(pool, id).await? else {
        return Ok(None);
    };
    sqlx::query("DELETE FROM agent_chat_tasks WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(Some(task))
}

async fn upsert_task(pool: &SqlitePool, task: &AgentChatTaskRecord) -> Result<(), StoreError> {
    let labels_json = serde_json::to_string(&task.labels)?;
    let health_json = task
        .health
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let comments_json = serde_json::to_string(&task.comments)?;
    let raw_json = serde_json::to_string(&task_json(task))?;
    sqlx::query(
        "INSERT INTO agent_chat_tasks \
         (id, title, description, status, priority, granularity, assignee, created_by, \
          created_at, updated_at, started_at, completed_at, heartbeat_at, waiting_reason, \
          waiting_until, parent_id, labels_json, health_json, comments_json, raw_json, imported_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
          title = excluded.title, \
          description = excluded.description, \
          status = excluded.status, \
          priority = excluded.priority, \
          granularity = excluded.granularity, \
          assignee = excluded.assignee, \
          created_by = excluded.created_by, \
          created_at = excluded.created_at, \
          updated_at = excluded.updated_at, \
          started_at = excluded.started_at, \
          completed_at = excluded.completed_at, \
          heartbeat_at = excluded.heartbeat_at, \
          waiting_reason = excluded.waiting_reason, \
          waiting_until = excluded.waiting_until, \
          parent_id = excluded.parent_id, \
          labels_json = excluded.labels_json, \
          health_json = excluded.health_json, \
          comments_json = excluded.comments_json, \
          raw_json = excluded.raw_json, \
          imported_at = excluded.imported_at",
    )
    .bind(&task.id)
    .bind(&task.title)
    .bind(&task.description)
    .bind(&task.status)
    .bind(&task.priority)
    .bind(&task.granularity)
    .bind(task.assignee.as_deref())
    .bind(task.created_by.as_deref())
    .bind(&task.created_at)
    .bind(&task.updated_at)
    .bind(task.started_at.as_deref())
    .bind(task.completed_at.as_deref())
    .bind(task.heartbeat_at.as_deref())
    .bind(task.waiting_reason.as_deref())
    .bind(task.waiting_until.as_deref())
    .bind(task.parent_id.as_deref())
    .bind(labels_json)
    .bind(health_json.as_deref())
    .bind(comments_json)
    .bind(raw_json)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

fn task_select_sql(tail: &'static str) -> sqlx::AssertSqlSafe<String> {
    sqlx::AssertSqlSafe(format!(
        "SELECT id, title, description, status, priority, granularity, assignee, created_by, \
         created_at, updated_at, started_at, completed_at, heartbeat_at, waiting_reason, \
         waiting_until, parent_id, labels_json, health_json, comments_json \
         FROM agent_chat_tasks {tail}"
    ))
}

fn row_to_task(row: &sqlx::sqlite::SqliteRow) -> Result<AgentChatTaskRecord, StoreError> {
    let labels_json = row
        .try_get::<String, _>("labels_json")
        .unwrap_or_else(|_| "[]".to_string());
    let comments_json = row
        .try_get::<String, _>("comments_json")
        .unwrap_or_else(|_| "[]".to_string());
    let health_json = row.try_get::<Option<String>, _>("health_json")?;
    Ok(AgentChatTaskRecord {
        id: row.try_get("id")?,
        title: row
            .try_get::<Option<String>, _>("title")?
            .unwrap_or_default(),
        description: row
            .try_get::<Option<String>, _>("description")?
            .unwrap_or_default(),
        status: row
            .try_get::<Option<String>, _>("status")?
            .unwrap_or_else(|| "created".to_string()),
        priority: row
            .try_get::<Option<String>, _>("priority")?
            .unwrap_or_else(|| "p2".to_string()),
        granularity: row
            .try_get::<Option<String>, _>("granularity")?
            .unwrap_or_else(|| "task".to_string()),
        assignee: row.try_get("assignee")?,
        created_by: row.try_get("created_by")?,
        created_at: row
            .try_get::<Option<String>, _>("created_at")?
            .unwrap_or_default(),
        updated_at: row
            .try_get::<Option<String>, _>("updated_at")?
            .unwrap_or_default(),
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
        heartbeat_at: row.try_get("heartbeat_at")?,
        waiting_reason: row.try_get("waiting_reason")?,
        waiting_until: row.try_get("waiting_until")?,
        parent_id: row.try_get("parent_id")?,
        labels: serde_json::from_str(&labels_json)?,
        health: health_json
            .map(|text| serde_json::from_str(&text))
            .transpose()?,
        comments: serde_json::from_str(&comments_json)?,
    })
}

fn task_json(task: &AgentChatTaskRecord) -> Value {
    json!({
        "id": task.id,
        "title": task.title,
        "description": task.description,
        "status": task.status,
        "priority": task.priority,
        "granularity": task.granularity,
        "assignee": task.assignee,
        "created_by": task.created_by,
        "created_at": task.created_at,
        "updated_at": task.updated_at,
        "started_at": task.started_at,
        "completed_at": task.completed_at,
        "heartbeat_at": task.heartbeat_at,
        "waiting_reason": task.waiting_reason,
        "waiting_until": task.waiting_until,
        "parent_id": task.parent_id,
        "labels": task.labels,
        "health": task.health,
        "comments": task.comments,
    })
}

async fn ensure_parent_exists(
    pool: &SqlitePool,
    parent_id: Option<&str>,
) -> Result<(), StoreError> {
    let Some(parent_id) = parent_id else {
        return Ok(());
    };
    if get_task(pool, parent_id).await?.is_some() {
        return Ok(());
    }
    Err(StoreError::Invariant(format!(
        "parent task not found: {parent_id}"
    )))
}

fn generate_task_id() -> String {
    let ts = now_unix();
    let seq = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
    format!("task_{ts}_{seq:06x}")
}

fn now_text() -> String {
    format!("{}Z", now_unix_ms())
}

fn validate_status(value: &str) -> Result<(), StoreError> {
    if STATUSES.contains(&value) {
        Ok(())
    } else {
        Err(StoreError::Invariant(format!("invalid status: {value}")))
    }
}

fn validate_member(field: &str, value: &str, allowed: &[&str]) -> Result<(), StoreError> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(StoreError::Invariant(format!("invalid {field}: {value}")))
}

fn transition_allowed(from: &str, to: &str) -> bool {
    matches!(
        (from, to),
        ("created", "accepted")
            | ("accepted" | "blocked", "in_progress")
            | ("in_progress", "blocked" | "done")
    )
}

fn normalize_required(value: &str, max_len: usize, message: &str) -> Result<String, StoreError> {
    clean_text(Some(value), max_len).ok_or_else(|| StoreError::Invariant(message.to_string()))
}

fn clean_text(value: Option<&str>, max_len: usize) -> Option<String> {
    let trimmed = value?.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(max_len).collect())
}

fn normalize_labels(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        if out.len() >= 20 {
            break;
        }
        let Some(label) = clean_text(Some(&value), 64) else {
            continue;
        };
        if !out.iter().any(|existing| existing == &label) {
            out.push(label);
        }
    }
    out
}
