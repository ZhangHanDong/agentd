//! Backend-facing Matrix bridge compatibility state.
//!
//! This stores the durable contract an external Matrix bridge process needs:
//! trusted room mappings and inbound event idempotency. Actual agent messages
//! continue to live in `message_repo`.

use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::now_unix;

pub async fn get_outbox_cursor(pool: &SqlitePool, bridge_id: &str) -> Result<i64, StoreError> {
    let bridge_id = required(bridge_id.to_string(), "matrix bridge id required")?;
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT last_seq FROM matrix_outbox_cursors WHERE bridge_id = ?",
    )
    .bind(bridge_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or(0))
}

pub async fn acknowledge_outbox_cursor(
    pool: &SqlitePool,
    bridge_id: &str,
    last_seq: i64,
) -> Result<i64, StoreError> {
    let bridge_id = required(bridge_id.to_string(), "matrix bridge id required")?;
    if last_seq < 0 {
        return Err(StoreError::Invariant(
            "matrix cursor must be non-negative".into(),
        ));
    }
    let now = now_unix();
    sqlx::query(
        "INSERT INTO matrix_outbox_cursors (bridge_id, last_seq, updated_at) VALUES (?, ?, ?) \
         ON CONFLICT(bridge_id) DO UPDATE SET last_seq = MAX(matrix_outbox_cursors.last_seq, excluded.last_seq), updated_at = excluded.updated_at",
    )
    .bind(bridge_id.clone())
    .bind(last_seq)
    .bind(now)
    .execute(pool)
    .await?;
    get_outbox_cursor(pool, &bridge_id).await
}

#[derive(Debug, Clone)]
pub struct MatrixBridgeRoomInput {
    pub room_id: String,
    pub project_id: Option<String>,
    pub group_name: Option<String>,
    pub agent_name: Option<String>,
    pub trusted: bool,
    pub trust_reason: String,
    pub inviter_mxid: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MatrixBridgeRoomRecord {
    pub room_id: String,
    pub project_id: Option<String>,
    pub group_name: Option<String>,
    pub agent_name: Option<String>,
    pub trusted: bool,
    pub trust_reason: String,
    pub inviter_mxid: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone)]
pub struct MatrixBridgeEventInput {
    pub event_id: String,
    pub room_id: String,
    pub sender_mxid: String,
    pub message_id: Option<String>,
    pub route: String,
    pub ignored: bool,
}

#[derive(Debug, Clone)]
pub struct MatrixBridgeEventRecord {
    pub event_id: String,
    pub room_id: String,
    pub sender_mxid: String,
    pub message_id: Option<String>,
    pub route: String,
    pub ignored: bool,
    pub created_at: i64,
}

pub async fn upsert_room(
    pool: &SqlitePool,
    input: MatrixBridgeRoomInput,
) -> Result<MatrixBridgeRoomRecord, StoreError> {
    let room_id = required(input.room_id, "matrix room id required")?;
    let project_id = clean_opt(input.project_id);
    let group_name = clean_opt(input.group_name);
    let agent_name = clean_opt(input.agent_name);
    let trust_reason = clean_opt(Some(input.trust_reason)).unwrap_or_else(|| "managed".to_string());
    let inviter_mxid = clean_opt(input.inviter_mxid);
    let now = now_unix();

    sqlx::query(
        "INSERT INTO matrix_bridge_rooms \
         (room_id, project_id, group_name, agent_name, trusted, trust_reason, inviter_mxid, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(room_id) DO UPDATE SET \
          project_id = excluded.project_id, \
          group_name = excluded.group_name, \
          agent_name = excluded.agent_name, \
          trusted = excluded.trusted, \
          trust_reason = excluded.trust_reason, \
          inviter_mxid = excluded.inviter_mxid, \
          updated_at = excluded.updated_at",
    )
    .bind(&room_id)
    .bind(project_id.as_deref())
    .bind(group_name.as_deref())
    .bind(agent_name.as_deref())
    .bind(i64::from(input.trusted))
    .bind(&trust_reason)
    .bind(inviter_mxid.as_deref())
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    get_room(pool, &room_id)
        .await?
        .ok_or_else(|| StoreError::Invariant(format!("matrix bridge room '{room_id}' is missing")))
}

pub async fn get_room(
    pool: &SqlitePool,
    room_id: &str,
) -> Result<Option<MatrixBridgeRoomRecord>, StoreError> {
    let room_id = required(room_id.to_string(), "matrix room id required")?;
    let row = sqlx::query(
        "SELECT room_id, project_id, group_name, agent_name, trusted, trust_reason, inviter_mxid, \
         created_at, updated_at FROM matrix_bridge_rooms WHERE room_id = ?",
    )
    .bind(room_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row_to_room(&row)))
}

pub async fn get_event(
    pool: &SqlitePool,
    event_id: &str,
) -> Result<Option<MatrixBridgeEventRecord>, StoreError> {
    let event_id = required(event_id.to_string(), "matrix event id required")?;
    let row = sqlx::query(
        "SELECT event_id, room_id, sender_mxid, message_id, route, ignored, created_at \
         FROM matrix_bridge_events WHERE event_id = ?",
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row_to_event(&row)))
}

pub async fn record_event(
    pool: &SqlitePool,
    input: MatrixBridgeEventInput,
) -> Result<MatrixBridgeEventRecord, StoreError> {
    let event_id = required(input.event_id, "matrix event id required")?;
    let room_id = required(input.room_id, "matrix room id required")?;
    let sender_mxid = required(input.sender_mxid, "matrix sender mxid required")?;
    let message_id = clean_opt(input.message_id);
    let route = required(input.route, "matrix route required")?;
    let created_at = now_unix();

    sqlx::query(
        "INSERT INTO matrix_bridge_events \
         (event_id, room_id, sender_mxid, message_id, route, ignored, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(event_id) DO NOTHING",
    )
    .bind(&event_id)
    .bind(&room_id)
    .bind(&sender_mxid)
    .bind(message_id.as_deref())
    .bind(&route)
    .bind(i64::from(input.ignored))
    .bind(created_at)
    .execute(pool)
    .await?;

    get_event(pool, &event_id).await?.ok_or_else(|| {
        StoreError::Invariant(format!("matrix bridge event '{event_id}' is missing"))
    })
}

fn row_to_room(row: &sqlx::sqlite::SqliteRow) -> MatrixBridgeRoomRecord {
    MatrixBridgeRoomRecord {
        room_id: row.get("room_id"),
        project_id: row.get("project_id"),
        group_name: row.get("group_name"),
        agent_name: row.get("agent_name"),
        trusted: row.get::<i64, _>("trusted") != 0,
        trust_reason: row.get("trust_reason"),
        inviter_mxid: row.get("inviter_mxid"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn row_to_event(row: &sqlx::sqlite::SqliteRow) -> MatrixBridgeEventRecord {
    MatrixBridgeEventRecord {
        event_id: row.get("event_id"),
        room_id: row.get("room_id"),
        sender_mxid: row.get("sender_mxid"),
        message_id: row.get("message_id"),
        route: row.get("route"),
        ignored: row.get::<i64, _>("ignored") != 0,
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
