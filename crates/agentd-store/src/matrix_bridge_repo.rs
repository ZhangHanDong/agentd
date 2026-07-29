//! Backend-facing Matrix bridge compatibility state.
//!
//! This stores the durable contract an external Matrix bridge process needs:
//! trusted room mappings and inbound event idempotency. Actual agent messages
//! continue to live in `message_repo`.

use sha2::{Digest, Sha256};
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

/// Durable, agentd-owned Matrix gateway inbound cursor.
#[derive(Debug, Clone)]
pub struct MatrixGatewayCursorRecord {
    pub gateway_id: String,
    pub sync_token: Option<String>,
    pub last_event_id: Option<String>,
    pub record_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One cursor advance. `expected_version` is the compare-and-set predicate:
/// `None` creates the row and fails if it already exists; `Some(v)` updates
/// only the row still at `v`.
#[derive(Debug, Clone)]
pub struct MatrixGatewayCursorInput {
    pub gateway_id: String,
    pub sync_token: Option<String>,
    pub last_event_id: Option<String>,
    pub expected_version: Option<i64>,
}

pub async fn get_gateway_cursor(
    pool: &SqlitePool,
    gateway_id: &str,
) -> Result<Option<MatrixGatewayCursorRecord>, StoreError> {
    let gateway_id = required(gateway_id.to_string(), "matrix gateway id required")?;
    let row = sqlx::query(
        "SELECT gateway_id, sync_token, last_event_id, record_version, created_at, updated_at \
         FROM matrix_gateway_cursors WHERE gateway_id = ?",
    )
    .bind(gateway_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row_to_gateway_cursor(&row)))
}

/// Create or advance one gateway cursor under compare-and-set.
///
/// # Errors
/// [`StoreError::Invariant`] when the gateway id is blank;
/// [`StoreError::Conflict`] when `expected_version` does not match the stored
/// `record_version` (including a `None` against an existing row). The message
/// deliberately does not end in `"changed concurrently"`: that suffix is the
/// task-graph retry sentinel and must not be borrowed here.
pub async fn advance_gateway_cursor(
    pool: &SqlitePool,
    input: MatrixGatewayCursorInput,
) -> Result<MatrixGatewayCursorRecord, StoreError> {
    let gateway_id = required(input.gateway_id, "matrix gateway id required")?;
    let sync_token = clean_opt(input.sync_token);
    let last_event_id = clean_opt(input.last_event_id);
    let now = now_unix();

    let mut connection = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await?;
    let result = advance_gateway_cursor_in_transaction(
        &mut connection,
        &gateway_id,
        sync_token.as_deref(),
        last_event_id.as_deref(),
        input.expected_version,
        now,
    )
    .await;
    match result {
        Ok(record) => {
            sqlx::query("COMMIT").execute(&mut *connection).await?;
            Ok(record)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

async fn advance_gateway_cursor_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    gateway_id: &str,
    sync_token: Option<&str>,
    last_event_id: Option<&str>,
    expected_version: Option<i64>,
    now: i64,
) -> Result<MatrixGatewayCursorRecord, StoreError> {
    match expected_version {
        None => {
            let inserted = sqlx::query(
                "INSERT INTO matrix_gateway_cursors \
                 (gateway_id, sync_token, last_event_id, record_version, created_at, updated_at) \
                 VALUES (?, ?, ?, 1, ?, ?) \
                 ON CONFLICT(gateway_id) DO NOTHING",
            )
            .bind(gateway_id)
            .bind(sync_token)
            .bind(last_event_id)
            .bind(now)
            .bind(now)
            .execute(&mut *connection)
            .await?;
            if inserted.rows_affected() != 1 {
                return Err(gateway_cursor_version_mismatch(gateway_id));
            }
        }
        Some(version) => {
            let updated = sqlx::query(
                "UPDATE matrix_gateway_cursors \
                 SET sync_token = ?, last_event_id = ?, \
                     record_version = record_version + 1, updated_at = ? \
                 WHERE gateway_id = ? AND record_version = ?",
            )
            .bind(sync_token)
            .bind(last_event_id)
            .bind(now)
            .bind(gateway_id)
            .bind(version)
            .execute(&mut *connection)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(gateway_cursor_version_mismatch(gateway_id));
            }
        }
    }

    let row = sqlx::query(
        "SELECT gateway_id, sync_token, last_event_id, record_version, created_at, updated_at \
         FROM matrix_gateway_cursors WHERE gateway_id = ?",
    )
    .bind(gateway_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| {
        StoreError::Invariant(format!("matrix gateway cursor '{gateway_id}' is missing"))
    })?;
    Ok(row_to_gateway_cursor(&row))
}

fn gateway_cursor_version_mismatch(gateway_id: &str) -> StoreError {
    StoreError::Conflict(format!(
        "matrix gateway cursor '{gateway_id}' record version mismatch"
    ))
}

fn row_to_gateway_cursor(row: &sqlx::sqlite::SqliteRow) -> MatrixGatewayCursorRecord {
    MatrixGatewayCursorRecord {
        gateway_id: row.get("gateway_id"),
        sync_token: row.get("sync_token"),
        last_event_id: row.get("last_event_id"),
        record_version: row.get("record_version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

/// The canonical agentd command id for one Matrix event.
///
/// Deterministic: a replayed event recomputes the identical id with no read,
/// which is what lets the inbound transaction be a pure insert-or-conflict.
/// The domain prefix and the unit-separator framing keep `(room, event)` pairs
/// from colliding across concatenations.
#[must_use]
pub fn matrix_command_id(room_id: &str, event_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"agentd.matrix.command.v1\x1f");
    hasher.update(room_id.trim().as_bytes());
    hasher.update(b"\x1f");
    hasher.update(event_id.trim().as_bytes());
    format!("mxc_{}", hex32(&hasher.finalize()))
}

/// The room/project dedup key for one command payload.
///
/// The normalization here is deliberately minimal — trim, ASCII-lowercase,
/// collapse internal whitespace. Full command normalization (mention
/// stripping, bang-command parsing) is M4 Plan B, and it replaces this
/// function and the dedup key together.
#[must_use]
pub fn matrix_command_dedup_key(body: &str) -> String {
    let normalized = body
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(b"agentd.matrix.dedup.v1\x1f");
    hasher.update(normalized.as_bytes());
    hex32(&hasher.finalize())
}

fn hex32(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    digest.iter().take(16).fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// The run a Matrix command asks agentd to create, as stored on the command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MatrixCommandRunPlan {
    pub label: String,
    pub owner: String,
    pub assignee: String,
    pub description: String,
}

/// One inbound Matrix command as accepted by the gateway.
#[derive(Debug, Clone)]
pub struct MatrixCommandInput {
    pub event_id: String,
    pub room_id: String,
    pub project_id: Option<String>,
    pub sender_mxid: String,
    pub route: String,
    pub body: String,
    /// `true` when the command requests a run and must hold the open-dedup
    /// slot until it settles; `false` for plain chat, which is recorded
    /// `settled` and never contends. Always equals `run_request.is_some()` for
    /// callers going through the inbound route, but stays a separate field so
    /// the index predicate is explicit at the insert site.
    pub open: bool,
    /// The run to create, or `None` for plain chat.
    pub run_request: Option<MatrixCommandRunPlan>,
}

/// Durable canonical command record.
#[derive(Debug, Clone)]
pub struct MatrixCommandRecord {
    pub command_id: String,
    pub event_id: String,
    pub room_id: String,
    pub project_key: String,
    pub dedup_key: String,
    pub sender_mxid: String,
    pub route: String,
    pub status: String,
    pub message_id: Option<String>,
    pub run_id: Option<String>,
    pub run_request_json: Option<String>,
    pub record_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

const COMMAND_SELECT_SQL: &str = "SELECT command_id, event_id, room_id, project_key, dedup_key, \
     sender_mxid, route, status, message_id, run_id, run_request_json, record_version, \
     created_at, updated_at FROM matrix_commands";

pub async fn get_command(
    pool: &SqlitePool,
    command_id: &str,
) -> Result<Option<MatrixCommandRecord>, StoreError> {
    let command_id = required(command_id.to_string(), "matrix command id required")?;
    let row = sqlx::query(&format!("{COMMAND_SELECT_SQL} WHERE command_id = ?"))
        .bind(command_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|row| row_to_command(&row)))
}

/// Commands that were accepted but have no run yet, oldest first.
pub async fn list_accepted_commands(
    pool: &SqlitePool,
) -> Result<Vec<MatrixCommandRecord>, StoreError> {
    let rows = sqlx::query(&format!(
        "{COMMAND_SELECT_SQL} WHERE status = 'accepted' AND run_id IS NULL \
         ORDER BY created_at ASC, command_id ASC"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_command).collect())
}

fn row_to_command(row: &sqlx::sqlite::SqliteRow) -> MatrixCommandRecord {
    MatrixCommandRecord {
        command_id: row.get("command_id"),
        event_id: row.get("event_id"),
        room_id: row.get("room_id"),
        project_key: row.get("project_key"),
        dedup_key: row.get("dedup_key"),
        sender_mxid: row.get("sender_mxid"),
        route: row.get("route"),
        status: row.get("status"),
        message_id: row.get("message_id"),
        run_id: row.get("run_id"),
        run_request_json: row.get("run_request_json"),
        record_version: row.get("record_version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

/// Everything one accepted inbound Matrix event writes, as one unit.
#[derive(Debug, Clone)]
pub struct MatrixInboundAcceptance {
    pub command: MatrixCommandInput,
    pub direct: Option<crate::message_repo::DirectMessageInput>,
    pub group: Option<crate::message_repo::GroupMessageInput>,
    pub relay_payload: serde_json::Value,
}

/// What the acceptance produced. `duplicate` means the event was already
/// accepted and nothing new was written.
#[derive(Debug, Clone)]
pub struct MatrixInboundAcceptanceResult {
    pub command: MatrixCommandRecord,
    pub duplicate: bool,
    pub direct: Option<crate::message_repo::DirectMessageRecord>,
    pub group: Option<crate::message_repo::GroupMessageRecord>,
}

/// Accept one inbound Matrix event: processed-event row, canonical command
/// row, inbox message, and outbox event, in one `BEGIN IMMEDIATE` or none.
///
/// Before this, the four writes were independent pool calls behind a
/// read-only duplicate check, so a crash between the message insert and the
/// event insert let the replay create a second message under a fresh ULID.
/// Here the message id is caller-supplied and derived from the command id, so
/// even a torn write is repaired by `ON CONFLICT(id) DO NOTHING`.
///
/// # Errors
/// [`StoreError::Invariant`] on blank required fields or when `open` disagrees
/// with `run_request`; [`StoreError::Conflict`] when an open command for the
/// same `(room, project, payload)` already exists.
pub async fn accept_inbound_event(
    pool: &SqlitePool,
    acceptance: MatrixInboundAcceptance,
) -> Result<MatrixInboundAcceptanceResult, StoreError> {
    let mut connection = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await?;
    let result = accept_inbound_event_in_transaction(&mut connection, acceptance).await;
    match result {
        Ok(value) => {
            sqlx::query("COMMIT").execute(&mut *connection).await?;
            Ok(value)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

/// One inbound command with every field trimmed, derived, and checked, so the
/// transaction body below is nothing but writes.
struct NormalizedCommand {
    event_id: String,
    room_id: String,
    sender_mxid: String,
    route: String,
    project_key: String,
    dedup_key: String,
    command_id: String,
    status: &'static str,
    run_request_json: Option<String>,
}

fn normalize_command(command: MatrixCommandInput) -> Result<NormalizedCommand, StoreError> {
    // `open` is what the partial unique index keys on and `run_request` is
    // what the Task 6 sweep acts on. Coercing a mismatch silently would either
    // disable the dedup slot or leave the sweep a run nobody holds a slot for,
    // so a caller that computed them inconsistently is stopped here.
    if command.open != command.run_request.is_some() {
        return Err(StoreError::Invariant(
            "matrix command open flag must match the presence of a run request".to_string(),
        ));
    }
    let event_id = required(command.event_id, "matrix event id required")?;
    let room_id = required(command.room_id, "matrix room id required")?;
    let run_request_json = command
        .run_request
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    Ok(NormalizedCommand {
        sender_mxid: required(command.sender_mxid, "matrix sender mxid required")?,
        route: required(command.route, "matrix route required")?,
        project_key: clean_opt(command.project_id).unwrap_or_default(),
        dedup_key: matrix_command_dedup_key(&command.body),
        command_id: matrix_command_id(&room_id, &event_id),
        status: if command.open { "accepted" } else { "settled" },
        run_request_json,
        event_id,
        room_id,
    })
}

async fn accept_inbound_event_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    acceptance: MatrixInboundAcceptance,
) -> Result<MatrixInboundAcceptanceResult, StoreError> {
    let MatrixInboundAcceptance {
        command,
        direct,
        group,
        relay_payload,
    } = acceptance;
    let command = normalize_command(command)?;
    let now = now_unix();

    // The duplicate check is inside the transaction, so a concurrent POST of
    // the same event cannot slip between the read and the writes.
    if let Some(row) = sqlx::query(&format!("{COMMAND_SELECT_SQL} WHERE command_id = ?"))
        .bind(&command.command_id)
        .fetch_optional(&mut *connection)
        .await?
    {
        return Ok(MatrixInboundAcceptanceResult {
            command: row_to_command(&row),
            duplicate: true,
            direct: None,
            group: None,
        });
    }

    insert_event_and_command(&mut *connection, &command, now).await?;
    let NormalizedCommand {
        event_id,
        command_id,
        ..
    } = command;

    let direct_record = match direct {
        Some(input) => {
            Some(crate::message_repo::insert_direct_message_on(&mut *connection, input).await?)
        }
        None => None,
    };
    let group_record = match group {
        Some(input) => {
            Some(crate::message_repo::insert_group_message_on(&mut *connection, input).await?)
        }
        None => None,
    };

    let message_id = direct_record
        .as_ref()
        .map(|record| record.id.clone())
        .or_else(|| group_record.as_ref().map(|record| record.id.clone()));
    if let Some(message_id) = message_id.as_deref() {
        link_message(connection, &event_id, &command_id, message_id, now).await?;
    }

    let mut payload = match relay_payload {
        serde_json::Value::Object(_) => relay_payload,
        other => serde_json::json!({ "value": other }),
    };
    payload["commandId"] = serde_json::json!(command_id);
    if let Some(message_id) = message_id.as_deref() {
        payload["messageId"] = serde_json::json!(message_id);
    }
    crate::relay_repo::append_relay_stream_event_on(&mut *connection, "message", payload).await?;

    let row = sqlx::query(&format!("{COMMAND_SELECT_SQL} WHERE command_id = ?"))
        .bind(&command_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(|| {
            StoreError::Invariant(format!("matrix command '{command_id}' is missing"))
        })?;
    Ok(MatrixInboundAcceptanceResult {
        command: row_to_command(&row),
        duplicate: false,
        direct: direct_record,
        group: group_record,
    })
}

/// Does this insert failure come from the open-command dedup slot?
///
/// Write the processed-event row and the canonical command row.
///
/// Both are guarded inserts rather than upserts: the acceptance either claims
/// this event or refuses it, and refusing is the point.
async fn insert_event_and_command(
    connection: &mut sqlx::SqliteConnection,
    command: &NormalizedCommand,
    now: i64,
) -> Result<(), StoreError> {
    // `DO NOTHING` rather than a bare insert: an event recorded before this
    // transaction existed (an `[AGENTIGNORE]` row, or a pre-M4 message) has no
    // command row, so the duplicate read above cannot see it. That is a
    // non-retryable conflict, not a 500.
    let inserted_event = sqlx::query(
        "INSERT INTO matrix_bridge_events \
         (event_id, room_id, sender_mxid, message_id, route, ignored, created_at) \
         VALUES (?, ?, ?, NULL, ?, 0, ?) \
         ON CONFLICT(event_id) DO NOTHING",
    )
    .bind(&command.event_id)
    .bind(&command.room_id)
    .bind(&command.sender_mxid)
    .bind(&command.route)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    if inserted_event.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "matrix event '{}' was already accepted",
            command.event_id
        )));
    }

    let inserted_command = sqlx::query(
        "INSERT INTO matrix_commands \
         (command_id, event_id, room_id, project_key, dedup_key, sender_mxid, route, status, \
          message_id, run_id, run_request_json, record_version, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, 1, ?, ?)",
    )
    .bind(&command.command_id)
    .bind(&command.event_id)
    .bind(&command.room_id)
    .bind(&command.project_key)
    .bind(&command.dedup_key)
    .bind(&command.sender_mxid)
    .bind(&command.route)
    .bind(command.status)
    .bind(command.run_request_json.as_deref())
    .bind(now)
    .bind(now)
    .execute(&mut *connection)
    .await
    .map_err(|error| {
        if is_open_command_clash(&error) {
            // The partial unique index fired: an equivalent command for this
            // room and project is still running. Not "changed concurrently" —
            // the caller must not retry this, it must wait for the open one.
            StoreError::Conflict(format!(
                "matrix command for room '{}' is already open",
                command.room_id
            ))
        } else {
            StoreError::Sqlx(error)
        }
    })?;
    if inserted_command.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "matrix command '{}' was already accepted",
            command.command_id
        )));
    }
    Ok(())
}

/// Point the freshly written event and command rows at the inbox message they
/// produced. Both updates are guarded, so a row that moved underneath us — it
/// cannot, inside `BEGIN IMMEDIATE`, but the guard is what makes that true
/// rather than assumed — aborts the whole acceptance.
async fn link_message(
    connection: &mut sqlx::SqliteConnection,
    event_id: &str,
    command_id: &str,
    message_id: &str,
    now: i64,
) -> Result<(), StoreError> {
    let linked = sqlx::query(
        "UPDATE matrix_bridge_events SET message_id = ? WHERE event_id = ? AND message_id IS NULL",
    )
    .bind(message_id)
    .bind(event_id)
    .execute(&mut *connection)
    .await?;
    if linked.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "matrix event '{event_id}' record version mismatch"
        )));
    }
    let linked_command = sqlx::query(
        "UPDATE matrix_commands SET message_id = ?, record_version = record_version + 1, \
         updated_at = ? WHERE command_id = ? AND record_version = 1",
    )
    .bind(message_id)
    .bind(now)
    .bind(command_id)
    .execute(&mut *connection)
    .await?;
    if linked_command.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "matrix command '{command_id}' record version mismatch"
        )));
    }
    Ok(())
}

/// `SQLite` names the *columns* of the violated unique index, never the index
/// itself, so this matches on `dedup_key` — the one column that appears in no
/// other constraint on `matrix_commands`.
fn is_open_command_clash(error: &sqlx::Error) -> bool {
    error.as_database_error().is_some_and(|db| {
        let message = db.message();
        message.contains("UNIQUE constraint failed")
            && message.contains("matrix_commands.dedup_key")
    })
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
