//! Agent registry operations for the agent-chat replacement lifecycle surface.
//! This repo stays outside the engine-facing `Store` trait: it is daemon/control
//! state, not workflow execution state.

use serde_json::{Value, json};
use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::now_unix;

#[derive(Debug, Clone)]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
    pub capability: Option<String>,
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub tmux_target: Option<String>,
    pub home_dir: Option<String>,
    pub workdir: Option<String>,
    pub state_dir: Option<String>,
    pub server: Option<String>,
    pub status: String,
    pub offline_reason: Option<String>,
    pub last_seen_at: Option<i64>,
    pub registered_at: i64,
    pub updated_at: i64,
    pub runtime_profile: Value,
    pub runtime_state: Value,
}

#[derive(Debug, Clone)]
pub struct RegisterAgent {
    pub name: String,
    pub role: Option<String>,
    pub capability: Option<String>,
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub tmux_target: Option<String>,
    pub home_dir: Option<String>,
    pub workdir: Option<String>,
    pub state_dir: Option<String>,
    pub server: Option<String>,
    pub runtime_profile: Value,
}

#[derive(Debug, Clone)]
pub struct HeartbeatAgent {
    pub server: Option<String>,
    pub tmux_target: Option<String>,
    pub workspace_path: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OfflineAgent {
    pub reason: Option<String>,
    pub clear_tmux: bool,
}

#[derive(Debug, Clone)]
pub struct StartedAgent {
    pub tmux_target: String,
}

#[derive(Debug, Clone)]
pub struct RuntimeUpdate {
    pub blocked: Option<bool>,
    pub blocked_reason: Option<String>,
    pub active_now: Option<bool>,
    pub active_duration_sec: Option<i64>,
    pub idle_duration_sec: Option<i64>,
    pub last_tmux_activity_sec: Option<i64>,
    pub workspace_path: Option<String>,
    pub mcp_present: Option<bool>,
}

pub async fn update_agent_identity(
    pool: &SqlitePool,
    name: &str,
    identity: &str,
) -> Result<Option<AgentRecord>, StoreError> {
    let name = normalize_name(name)?;
    let identity = clean_text(identity, "identity text required")?;
    let Some(agent) = get_agent(pool, &name).await? else {
        return Ok(None);
    };

    let now = now_unix();
    let mut runtime_profile = match agent.runtime_profile {
        Value::Object(_) => agent.runtime_profile,
        _ => json!({}),
    };
    runtime_profile
        .as_object_mut()
        .expect("runtime_profile normalized to object")
        .insert("identity".to_string(), json!(identity));

    let runtime_profile_text = serde_json::to_string(&runtime_profile)?;
    sqlx::query("UPDATE agents SET runtime_profile = ?, updated_at = ? WHERE name = ? OR id = ?")
        .bind(runtime_profile_text)
        .bind(now)
        .bind(&name)
        .bind(&name)
        .execute(pool)
        .await?;

    get_agent(pool, &name).await
}

pub async fn register_agent(
    pool: &SqlitePool,
    input: RegisterAgent,
) -> Result<AgentRecord, StoreError> {
    let name = normalize_name(&input.name)?;
    let role = clean_opt(input.role).unwrap_or_else(|| "agent".to_string());
    let capability = clean_opt(input.capability);
    let runtime = clean_opt(input.runtime);
    let model = clean_opt(input.model);
    let tmux_target = clean_opt(input.tmux_target);
    let home_dir = clean_opt(input.home_dir);
    let workdir = clean_opt(input.workdir);
    let state_dir = clean_opt(input.state_dir);
    let server = clean_opt(input.server);
    let runtime_profile = normalize_runtime_profile(input.runtime_profile);
    let runtime_profile_text = serde_json::to_string(&runtime_profile)?;
    let now = now_unix();
    let status = if tmux_target.is_some() {
        "online"
    } else {
        "offline"
    };
    let offline_reason = (status == "offline").then_some("offline".to_string());
    let backend = runtime.clone().unwrap_or_else(|| "agent".to_string());
    let mxid = local_mxid(&name);

    sqlx::query(
        "INSERT INTO agents \
         (id, mxid, role, backend, backend_target, prompt_profile, enabled, created_at, \
          name, capability, runtime, model, tmux_target, home_dir, workdir, state_dir, \
          server, status, offline_reason, last_seen_at, registered_at, updated_at, runtime_profile) \
         VALUES (?, ?, ?, ?, ?, NULL, 1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
          name = excluded.name, \
          role = excluded.role, \
          backend = excluded.backend, \
          backend_target = excluded.backend_target, \
          enabled = 1, \
          capability = excluded.capability, \
          runtime = excluded.runtime, \
          model = excluded.model, \
          tmux_target = excluded.tmux_target, \
          home_dir = excluded.home_dir, \
          workdir = excluded.workdir, \
          state_dir = excluded.state_dir, \
          server = excluded.server, \
          status = excluded.status, \
          offline_reason = excluded.offline_reason, \
          last_seen_at = excluded.last_seen_at, \
          updated_at = excluded.updated_at, \
          runtime_profile = excluded.runtime_profile",
    )
    .bind(&name)
    .bind(mxid)
    .bind(role)
    .bind(backend)
    .bind(tmux_target.as_deref())
    .bind(now)
    .bind(&name)
    .bind(capability.as_deref())
    .bind(runtime.as_deref())
    .bind(model.as_deref())
    .bind(tmux_target.as_deref())
    .bind(home_dir.as_deref())
    .bind(workdir.as_deref())
    .bind(state_dir.as_deref())
    .bind(server.as_deref())
    .bind(status)
    .bind(offline_reason.as_deref())
    .bind((status == "online").then_some(now))
    .bind(now)
    .bind(now)
    .bind(runtime_profile_text)
    .execute(pool)
    .await?;

    get_agent(pool, &name)
        .await?
        .ok_or_else(|| StoreError::Invariant(format!("registered agent '{name}' is missing")))
}

pub async fn list_agents(pool: &SqlitePool) -> Result<Vec<AgentRecord>, StoreError> {
    let rows = sqlx::query(agent_select_sql("WHERE name IS NOT NULL ORDER BY name").as_str())
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(row_to_agent).collect())
}

pub async fn get_agent(pool: &SqlitePool, name: &str) -> Result<Option<AgentRecord>, StoreError> {
    let name = normalize_name(name)?;
    let row = sqlx::query(agent_select_sql("WHERE name = ? OR id = ?").as_str())
        .bind(&name)
        .bind(&name)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| row_to_agent(&r)))
}

pub async fn heartbeat_agent(
    pool: &SqlitePool,
    name: &str,
    input: HeartbeatAgent,
) -> Result<(AgentRecord, bool), StoreError> {
    let name = normalize_name(name)?;
    let server = clean_opt(input.server);
    let tmux_target = clean_opt(input.tmux_target);
    let workspace_path = clean_opt(input.workspace_path);
    let now = now_unix();
    let created = get_agent(pool, &name).await?.is_none();

    if created {
        let role = "agent";
        let backend = "agent";
        let mxid = local_mxid(&name);
        let runtime_profile_text = "{}";
        sqlx::query(
            "INSERT INTO agents \
             (id, mxid, role, backend, backend_target, prompt_profile, enabled, created_at, \
              name, tmux_target, workdir, server, status, offline_reason, last_seen_at, \
              registered_at, updated_at, runtime_profile) \
             VALUES (?, ?, ?, ?, ?, NULL, 1, ?, ?, ?, ?, ?, 'online', NULL, ?, ?, ?, ?)",
        )
        .bind(&name)
        .bind(mxid)
        .bind(role)
        .bind(backend)
        .bind(tmux_target.as_deref())
        .bind(now)
        .bind(&name)
        .bind(tmux_target.as_deref())
        .bind(workspace_path.as_deref())
        .bind(server.as_deref())
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(runtime_profile_text)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE agents SET \
              server = COALESCE(?, server), \
              tmux_target = COALESCE(?, tmux_target), \
              backend_target = COALESCE(?, backend_target), \
              workdir = COALESCE(?, workdir), \
              status = 'online', \
              offline_reason = NULL, \
              last_seen_at = ?, \
              updated_at = ? \
             WHERE name = ? OR id = ?",
        )
        .bind(server.as_deref())
        .bind(tmux_target.as_deref())
        .bind(tmux_target.as_deref())
        .bind(workspace_path.as_deref())
        .bind(now)
        .bind(now)
        .bind(&name)
        .bind(&name)
        .execute(pool)
        .await?;
    }

    let agent = get_agent(pool, &name)
        .await?
        .ok_or_else(|| StoreError::Invariant(format!("heartbeat agent '{name}' is missing")))?;
    Ok((agent, created))
}

pub async fn mark_agent_offline(
    pool: &SqlitePool,
    name: &str,
    input: OfflineAgent,
) -> Result<Option<AgentRecord>, StoreError> {
    let name = normalize_name(name)?;
    if get_agent(pool, &name).await?.is_none() {
        return Ok(None);
    }
    let reason = clean_opt(input.reason).unwrap_or_else(|| "manual-offline".to_string());
    let now = now_unix();
    if input.clear_tmux {
        sqlx::query(
            "UPDATE agents SET status = 'offline', offline_reason = ?, tmux_target = NULL, \
             backend_target = NULL, last_seen_at = ?, updated_at = ? WHERE name = ? OR id = ?",
        )
        .bind(&reason)
        .bind(now)
        .bind(now)
        .bind(&name)
        .bind(&name)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "UPDATE agents SET status = 'offline', offline_reason = ?, last_seen_at = ?, \
             updated_at = ? WHERE name = ? OR id = ?",
        )
        .bind(&reason)
        .bind(now)
        .bind(now)
        .bind(&name)
        .bind(&name)
        .execute(pool)
        .await?;
    }
    get_agent(pool, &name).await
}

pub async fn mark_agent_started(
    pool: &SqlitePool,
    name: &str,
    input: StartedAgent,
) -> Result<Option<AgentRecord>, StoreError> {
    let name = normalize_name(name)?;
    if get_agent(pool, &name).await?.is_none() {
        return Ok(None);
    }
    let tmux_target = clean_opt(Some(input.tmux_target))
        .ok_or_else(|| StoreError::Invariant("tmux target required".to_string()))?;
    let now = now_unix();
    sqlx::query(
        "UPDATE agents SET \
          tmux_target = ?, \
          backend_target = ?, \
          status = 'online', \
          offline_reason = NULL, \
          last_seen_at = ?, \
          updated_at = ? \
         WHERE name = ? OR id = ?",
    )
    .bind(&tmux_target)
    .bind(&tmux_target)
    .bind(now)
    .bind(now)
    .bind(&name)
    .bind(&name)
    .execute(pool)
    .await?;
    get_agent(pool, &name).await
}

pub async fn update_agent_runtime(
    pool: &SqlitePool,
    name: &str,
    input: RuntimeUpdate,
) -> Result<Option<Value>, StoreError> {
    let name = normalize_name(name)?;
    let Some(agent) = get_agent(pool, &name).await? else {
        return Ok(None);
    };
    let now = now_unix();
    let workspace_path = clean_opt(input.workspace_path);
    let blocked_reason = clean_opt(input.blocked_reason);
    let mut runtime_state = match agent.runtime_state {
        Value::Object(_) => agent.runtime_state,
        _ => json!({}),
    };
    let object = runtime_state
        .as_object_mut()
        .expect("runtime_state normalized to object");
    object.insert("agent".to_string(), json!(name));
    if let Some(blocked) = input.blocked {
        object.insert("blocked".to_string(), json!(blocked));
    }
    if let Some(reason) = blocked_reason {
        object.insert("blockedReason".to_string(), json!(reason));
    }
    if let Some(active_now) = input.active_now {
        object.insert("activeNow".to_string(), json!(active_now));
    }
    if let Some(active_duration_sec) = input.active_duration_sec {
        object.insert("activeDurationSec".to_string(), json!(active_duration_sec));
    }
    if let Some(idle_duration_sec) = input.idle_duration_sec {
        object.insert("idleDurationSec".to_string(), json!(idle_duration_sec));
    }
    if let Some(last_tmux_activity_sec) = input.last_tmux_activity_sec {
        object.insert(
            "lastTmuxActivitySec".to_string(),
            json!(last_tmux_activity_sec),
        );
    }
    if let Some(path) = workspace_path.as_deref() {
        object.insert("workspacePath".to_string(), json!(path));
    }
    if let Some(mcp_present) = input.mcp_present {
        object.insert("mcpPresent".to_string(), json!(mcp_present));
    }
    object.insert("updatedAt".to_string(), json!(now));

    let runtime_state_text = serde_json::to_string(&runtime_state)?;
    sqlx::query(
        "UPDATE agents SET \
          runtime_state = ?, \
          workdir = COALESCE(?, workdir), \
          last_seen_at = ?, \
          updated_at = ? \
         WHERE name = ? OR id = ?",
    )
    .bind(runtime_state_text)
    .bind(workspace_path.as_deref())
    .bind(now)
    .bind(now)
    .bind(&name)
    .bind(&name)
    .execute(pool)
    .await?;
    Ok(Some(runtime_state))
}

pub async fn merge_agent_runtime_state(
    pool: &SqlitePool,
    name: &str,
    patch: Value,
) -> Result<Option<Value>, StoreError> {
    let name = normalize_name(name)?;
    let Some(agent) = get_agent(pool, &name).await? else {
        return Ok(None);
    };
    let Value::Object(patch) = patch else {
        return Err(StoreError::Invariant(
            "runtime state patch must be a JSON object".to_string(),
        ));
    };
    let now = now_unix();
    let mut runtime_state = match agent.runtime_state {
        Value::Object(_) => agent.runtime_state,
        _ => json!({}),
    };
    let object = runtime_state
        .as_object_mut()
        .expect("runtime_state normalized to object");
    object.insert("agent".to_string(), json!(name));
    for (key, value) in patch {
        object.insert(key, value);
    }
    object.insert("updatedAt".to_string(), json!(now));

    let runtime_state_text = serde_json::to_string(&runtime_state)?;
    sqlx::query(
        "UPDATE agents SET \
          runtime_state = ?, \
          last_seen_at = ?, \
          updated_at = ? \
         WHERE name = ? OR id = ?",
    )
    .bind(runtime_state_text)
    .bind(now)
    .bind(now)
    .bind(&name)
    .bind(&name)
    .execute(pool)
    .await?;
    Ok(Some(runtime_state))
}

fn normalize_name(name: &str) -> Result<String, StoreError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(StoreError::Invariant("agent name required".to_string()));
    }
    Ok(trimmed.to_string())
}

fn clean_opt(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn clean_text<'a>(value: &'a str, message: &str) -> Result<&'a str, StoreError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(StoreError::Invariant(message.to_string()));
    }
    Ok(trimmed)
}

fn normalize_runtime_profile(value: Value) -> Value {
    match value {
        Value::Null => json!({}),
        other => other,
    }
}

fn local_mxid(name: &str) -> String {
    format!("agentd-local:{name}")
}

fn agent_select_sql(tail: &str) -> String {
    format!(
        "SELECT id, name, role, capability, runtime, model, tmux_target, home_dir, \
         workdir, state_dir, server, status, offline_reason, last_seen_at, \
         registered_at, updated_at, runtime_profile, runtime_state FROM agents {tail}"
    )
}

fn row_to_agent(row: &sqlx::sqlite::SqliteRow) -> AgentRecord {
    let runtime_profile_text = row.get::<String, _>("runtime_profile");
    let runtime_profile = serde_json::from_str(&runtime_profile_text).unwrap_or_else(|_| json!({}));
    let runtime_state_text = row.get::<String, _>("runtime_state");
    let runtime_state = serde_json::from_str(&runtime_state_text).unwrap_or_else(|_| json!({}));
    AgentRecord {
        id: row.get("id"),
        name: row.get("name"),
        role: row.get("role"),
        capability: row.get("capability"),
        runtime: row.get("runtime"),
        model: row.get("model"),
        tmux_target: row.get("tmux_target"),
        home_dir: row.get("home_dir"),
        workdir: row.get("workdir"),
        state_dir: row.get("state_dir"),
        server: row.get("server"),
        status: row.get("status"),
        offline_reason: row.get("offline_reason"),
        last_seen_at: row.get("last_seen_at"),
        registered_at: row.get("registered_at"),
        updated_at: row.get("updated_at"),
        runtime_profile,
        runtime_state,
    }
}
