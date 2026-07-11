//! Backend-facing remote relay compatibility state.
//!
//! This is control-plane state for agent-chat replacement work: remote host
//! heartbeats, relay delivery audit events, and a message wakeup stream. It
//! stays outside the engine-facing `Store` trait.

use serde_json::{Value, json};
use sqlx::{Row, SqlitePool};
use ulid::Ulid;

use crate::error::StoreError;
use crate::util::now_unix;

#[derive(Debug, Clone)]
pub struct ServerHeartbeatInput {
    pub server: String,
    pub instance_id: Option<String>,
    pub boot_ts: Option<i64>,
    pub agents: Vec<String>,
    pub sessions: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct RelayServerRecord {
    pub id: String,
    pub instance_id: Option<String>,
    pub boot_ts: Option<i64>,
    pub agents: Vec<String>,
    pub sessions: Vec<String>,
    pub agent_count: i64,
    pub online: bool,
    pub maintenance: bool,
    pub last_seen_at: i64,
    pub heartbeat_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct DeliveryEventInput {
    pub event_type: String,
    pub message_id: Option<String>,
    pub queue_entry_id: Option<String>,
    pub agent: Option<String>,
    pub target: Option<String>,
    pub reason: Option<String>,
    pub source: Option<String>,
    pub context: Value,
}

#[derive(Debug, Clone)]
pub struct DeliveryEventRecord {
    pub id: String,
    pub seq: i64,
    pub event_type: String,
    pub message_id: Option<String>,
    pub queue_entry_id: Option<String>,
    pub agent: Option<String>,
    pub target: Option<String>,
    pub reason: Option<String>,
    pub source: Option<String>,
    pub context: Value,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct RelayStreamEventRecord {
    pub seq: i64,
    pub event: String,
    pub payload: Value,
    pub created_at: i64,
}

pub async fn record_server_heartbeat(
    pool: &SqlitePool,
    input: ServerHeartbeatInput,
) -> Result<RelayServerRecord, StoreError> {
    let server = required(input.server, "server required")?;
    let instance_id = clean_opt(input.instance_id);
    let agents = clean_list(input.agents);
    let sessions = clean_list(input.sessions);
    let agents_json = serde_json::to_string(&agents)?;
    let sessions_json = serde_json::to_string(&sessions)?;
    let now = now_unix();
    let agent_count = i64::try_from(agents.len()).unwrap_or(i64::MAX);

    sqlx::query(
        "INSERT INTO relay_servers \
         (id, instance_id, boot_ts, agents_json, sessions_json, agent_count, online, \
          maintenance, last_seen_at, heartbeat_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 1, 0, ?, ?, ?) \
         ON CONFLICT(id) DO UPDATE SET \
          instance_id = excluded.instance_id, \
          boot_ts = excluded.boot_ts, \
          agents_json = excluded.agents_json, \
          sessions_json = excluded.sessions_json, \
          agent_count = excluded.agent_count, \
          online = 1, \
          last_seen_at = excluded.last_seen_at, \
          heartbeat_at = excluded.heartbeat_at, \
          updated_at = excluded.updated_at",
    )
    .bind(&server)
    .bind(instance_id.as_deref())
    .bind(input.boot_ts)
    .bind(agents_json)
    .bind(sessions_json)
    .bind(agent_count)
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    get_server(pool, &server)
        .await?
        .ok_or_else(|| StoreError::Invariant(format!("relay server '{server}' is missing")))
}

pub async fn get_server(
    pool: &SqlitePool,
    server: &str,
) -> Result<Option<RelayServerRecord>, StoreError> {
    let server = required(server.to_string(), "server required")?;
    let row = sqlx::query(
        "SELECT id, instance_id, boot_ts, agents_json, sessions_json, agent_count, \
         online, maintenance, last_seen_at, heartbeat_at, updated_at \
         FROM relay_servers WHERE id = ?",
    )
    .bind(server)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row_to_server(&row)))
}

pub async fn append_delivery_event(
    pool: &SqlitePool,
    input: DeliveryEventInput,
) -> Result<DeliveryEventRecord, StoreError> {
    let event_type = required(input.event_type, "delivery event type required")?;
    let id = format!("del_{}", Ulid::new());
    let created_at = now_unix();
    let message_id = clean_opt(input.message_id);
    let queue_entry_id = clean_opt(input.queue_entry_id);
    let agent = clean_opt(input.agent);
    let target = clean_opt(input.target);
    let reason = clean_opt(input.reason);
    let source = clean_opt(input.source);
    let context = match input.context {
        Value::Object(_) => input.context,
        Value::Null => json!({}),
        other => json!({ "value": other }),
    };
    let context_json = serde_json::to_string(&context)?;

    sqlx::query(
        "INSERT INTO delivery_events \
         (id, seq, type, message_id, queue_entry_id, agent, target, reason, source, \
          context_json, created_at) \
         VALUES (?, COALESCE((SELECT MAX(seq) FROM delivery_events), 0) + 1, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&event_type)
    .bind(message_id.as_deref())
    .bind(queue_entry_id.as_deref())
    .bind(agent.as_deref())
    .bind(target.as_deref())
    .bind(reason.as_deref())
    .bind(source.as_deref())
    .bind(context_json)
    .bind(created_at)
    .execute(pool)
    .await?;

    let row = sqlx::query(
        "SELECT id, seq, type, message_id, queue_entry_id, agent, target, reason, source, \
         context_json, created_at FROM delivery_events WHERE id = ?",
    )
    .bind(id)
    .fetch_one(pool)
    .await?;
    Ok(row_to_delivery_event(&row))
}

pub async fn list_delivery_events_for_agent(
    pool: &SqlitePool,
    agent: &str,
    limit: i64,
) -> Result<Vec<DeliveryEventRecord>, StoreError> {
    let agent = required(agent.to_string(), "agent required")?;
    let limit = limit.clamp(1, 200);
    let rows = sqlx::query(
        "SELECT id, seq, type, message_id, queue_entry_id, agent, target, reason, source, \
         context_json, created_at FROM delivery_events \
         WHERE agent = ? ORDER BY seq DESC LIMIT ?",
    )
    .bind(agent)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_delivery_event).collect())
}

pub async fn append_relay_stream_event(
    pool: &SqlitePool,
    event: &str,
    payload: Value,
) -> Result<RelayStreamEventRecord, StoreError> {
    let event = required(event.to_string(), "stream event required")?;
    let payload = match payload {
        Value::Object(_) => payload,
        other => json!({ "value": other }),
    };
    let payload_json = serde_json::to_string(&payload)?;
    let created_at = now_unix();
    let result = sqlx::query(
        "INSERT INTO relay_stream_events (event, payload_json, created_at) VALUES (?, ?, ?)",
    )
    .bind(&event)
    .bind(payload_json)
    .bind(created_at)
    .execute(pool)
    .await?;
    let seq = result.last_insert_rowid();
    Ok(RelayStreamEventRecord {
        seq,
        event,
        payload,
        created_at,
    })
}

pub async fn list_relay_stream_events(
    pool: &SqlitePool,
    after_seq: i64,
) -> Result<Vec<RelayStreamEventRecord>, StoreError> {
    let rows = sqlx::query(
        "SELECT seq, event, payload_json, created_at FROM relay_stream_events \
         WHERE seq > ? ORDER BY seq ASC",
    )
    .bind(after_seq.max(0))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_stream_event).collect())
}

fn row_to_server(row: &sqlx::sqlite::SqliteRow) -> RelayServerRecord {
    let agents_json: String = row.get("agents_json");
    let sessions_json: String = row.get("sessions_json");
    RelayServerRecord {
        id: row.get("id"),
        instance_id: row.get("instance_id"),
        boot_ts: row.get("boot_ts"),
        agents: serde_json::from_str(&agents_json).unwrap_or_default(),
        sessions: serde_json::from_str(&sessions_json).unwrap_or_default(),
        agent_count: row.get("agent_count"),
        online: row.get::<i64, _>("online") != 0,
        maintenance: row.get::<i64, _>("maintenance") != 0,
        last_seen_at: row.get("last_seen_at"),
        heartbeat_at: row.get("heartbeat_at"),
        updated_at: row.get("updated_at"),
    }
}

fn row_to_delivery_event(row: &sqlx::sqlite::SqliteRow) -> DeliveryEventRecord {
    let context_json: String = row.get("context_json");
    DeliveryEventRecord {
        id: row.get("id"),
        seq: row.get("seq"),
        event_type: row.get("type"),
        message_id: row.get("message_id"),
        queue_entry_id: row.get("queue_entry_id"),
        agent: row.get("agent"),
        target: row.get("target"),
        reason: row.get("reason"),
        source: row.get("source"),
        context: serde_json::from_str(&context_json).unwrap_or_else(|_| json!({})),
        created_at: row.get("created_at"),
    }
}

fn row_to_stream_event(row: &sqlx::sqlite::SqliteRow) -> RelayStreamEventRecord {
    let payload_json: String = row.get("payload_json");
    RelayStreamEventRecord {
        seq: row.get("seq"),
        event: row.get("event"),
        payload: serde_json::from_str(&payload_json).unwrap_or_else(|_| json!({})),
        created_at: row.get("created_at"),
    }
}

fn required(value: String, message: &str) -> Result<String, StoreError> {
    clean_opt(Some(value)).ok_or_else(|| StoreError::Invariant(message.to_string()))
}

fn clean_opt(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn clean_list(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .filter_map(|value| clean_opt(Some(value)))
        .collect()
}
