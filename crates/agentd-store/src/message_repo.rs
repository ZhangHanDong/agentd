//! Durable direct-message inbox operations for agent-chat replacement work.
//! This is daemon/control state, not workflow engine state: the engine-facing
//! `Store` trait remains unchanged.

use std::collections::HashSet;

use serde_json::Value;
use sqlx::{Row, SqlitePool};
use ulid::Ulid;

use crate::error::StoreError;
use crate::util::{now_unix, now_unix_ms};

#[derive(Debug, Clone)]
pub struct DirectMessageInput {
    pub message_id: Option<String>,
    pub ts: Option<i64>,
    pub from: String,
    pub to: String,
    pub message_type: Option<String>,
    pub priority: Option<String>,
    pub summary: String,
    pub full: String,
    pub reply_to: Option<String>,
    pub source: Option<String>,
    pub source_room: Option<String>,
    pub sender_mxid: Option<String>,
    pub trust_level: Option<String>,
    pub from_id: Option<String>,
    pub schema: Option<Value>,
    pub attachments: Vec<Value>,
}

#[derive(Debug, Clone, Copy)]
pub struct InboxReadOptions {
    pub drain: bool,
}

#[derive(Debug, Clone)]
pub struct DirectMessageRecord {
    pub id: String,
    pub ts: i64,
    pub at: String,
    pub time: String,
    pub from: String,
    pub to: String,
    pub message_type: String,
    pub priority: String,
    pub summary: String,
    pub full: String,
    pub reply_to: Option<String>,
    pub source: String,
    pub source_room: Option<String>,
    pub sender_mxid: Option<String>,
    pub trust_level: Option<String>,
    pub from_id: Option<String>,
    pub schema: Option<Value>,
    pub attachments: Vec<Value>,
    pub read_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct GroupCreateInput {
    pub name: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRecord {
    pub name: String,
    pub members: Vec<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct GroupMessageInput {
    pub message_id: Option<String>,
    pub ts: Option<i64>,
    pub from: String,
    pub group: String,
    pub message_type: Option<String>,
    pub priority: Option<String>,
    pub summary: String,
    pub full: String,
    pub mentions: Vec<String>,
    pub reply_to: Option<String>,
    pub source: Option<String>,
    pub schema: Option<Value>,
    pub attachments: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupMessageRecord {
    pub id: String,
    pub ts: i64,
    pub at: String,
    pub time: String,
    pub from: String,
    pub group: String,
    pub message_type: String,
    pub priority: String,
    pub summary: String,
    pub full: String,
    pub mentions: Vec<String>,
    pub reply_to: Option<String>,
    pub source: String,
    pub schema: Option<Value>,
    pub attachments: Vec<Value>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentInboxReadResult {
    pub dm: Vec<DirectMessageRecord>,
    pub group: Vec<GroupMessageRecord>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GroupReadAdvance {
    #[default]
    None,
    All,
}

impl GroupReadAdvance {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupReadOptions {
    pub limit: usize,
    pub unread_limit: Option<usize>,
    pub advance: GroupReadAdvance,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GroupReadResult {
    pub group: String,
    pub unread: Vec<GroupMessageRecord>,
    pub read: Vec<GroupMessageRecord>,
    pub unread_total: usize,
    pub unread_returned: usize,
    pub unread_omitted: usize,
    pub advance: GroupReadAdvance,
}

pub async fn create_group(
    pool: &SqlitePool,
    input: GroupCreateInput,
) -> Result<GroupRecord, StoreError> {
    let name = required(input.name, "group name required")?;
    let members = clean_dedup(input.members);
    let created_at = now_unix();
    let mut tx = pool.begin().await?;
    sqlx::query("INSERT INTO groups (name, created_at) VALUES (?, ?)")
        .bind(&name)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
    for member in &members {
        sqlx::query(
            "INSERT OR IGNORE INTO group_members (group_name, agent_name, created_at) \
             VALUES (?, ?, ?)",
        )
        .bind(&name)
        .bind(member)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(GroupRecord {
        name,
        members,
        created_at,
    })
}

pub async fn list_groups(pool: &SqlitePool) -> Result<Vec<GroupRecord>, StoreError> {
    let rows = sqlx::query("SELECT name, created_at FROM groups ORDER BY name")
        .fetch_all(pool)
        .await?;
    let mut groups = Vec::with_capacity(rows.len());
    for row in rows {
        let name: String = row.get("name");
        groups.push(GroupRecord {
            members: group_members(pool, &name).await?,
            name,
            created_at: row.get("created_at"),
        });
    }
    Ok(groups)
}

pub async fn get_group(pool: &SqlitePool, name: &str) -> Result<Option<GroupRecord>, StoreError> {
    let Some(name) = clean_str(name) else {
        return Ok(None);
    };
    let row = sqlx::query("SELECT name, created_at FROM groups WHERE name = ?")
        .bind(&name)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    Ok(Some(GroupRecord {
        members: group_members(pool, &name).await?,
        name: row.get("name"),
        created_at: row.get("created_at"),
    }))
}

pub async fn update_group_members(
    pool: &SqlitePool,
    name: &str,
    add: &[String],
    remove: &[String],
) -> Result<Option<GroupRecord>, StoreError> {
    let Some(name) = clean_str(name) else {
        return Ok(None);
    };
    let Some(group) = get_group(pool, &name).await? else {
        return Ok(None);
    };
    let mut members = group.members;
    for member in clean_dedup(add.to_vec()) {
        if !members
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&member))
        {
            members.push(member);
        }
    }
    let remove_members = clean_dedup(remove.to_vec());
    members.retain(|member| {
        !remove_members
            .iter()
            .any(|removed| removed.eq_ignore_ascii_case(member))
    });

    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM group_members WHERE group_name = ?")
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    let now = now_unix();
    for member in &members {
        sqlx::query(
            "INSERT INTO group_members (group_name, agent_name, created_at) VALUES (?, ?, ?)",
        )
        .bind(&name)
        .bind(member)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    get_group(pool, &name).await
}

pub async fn delete_group(
    pool: &SqlitePool,
    name: &str,
) -> Result<Option<GroupRecord>, StoreError> {
    let Some(name) = clean_str(name) else {
        return Ok(None);
    };
    let Some(group) = get_group(pool, &name).await? else {
        return Ok(None);
    };
    sqlx::query("DELETE FROM groups WHERE name = ?")
        .bind(&name)
        .execute(pool)
        .await?;
    Ok(Some(group))
}

pub async fn insert_direct_message(
    pool: &SqlitePool,
    input: DirectMessageInput,
) -> Result<DirectMessageRecord, StoreError> {
    let id = clean_opt(input.message_id).unwrap_or_else(generate_message_id);
    let from = required(input.from, "message from required")?;
    let to = required(input.to, "message to required")?;
    let message_type = clean_opt(input.message_type).unwrap_or_else(|| "human".to_string());
    let priority = clean_opt(input.priority).unwrap_or_else(|| "normal".to_string());
    let summary = required(input.summary, "message summary required")?;
    let full = input.full;
    let reply_to = clean_opt(input.reply_to);
    let source = clean_opt(input.source).unwrap_or_else(|| "api".to_string());
    let source_room = clean_opt(input.source_room);
    let sender_mxid = clean_opt(input.sender_mxid);
    let trust_level = clean_opt(input.trust_level);
    let from_id = clean_opt(input.from_id);
    let schema_json = input
        .schema
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let attachments_json = serde_json::to_string(&input.attachments)?;
    let ts = input.ts.unwrap_or_else(now_unix_ms);
    let created_at = now_unix();

    sqlx::query(
        "INSERT INTO direct_messages \
         (id, ts, from_agent, to_agent, message_type, priority, summary, full, \
          reply_to, source, source_room, sender_mxid, trust_level, from_id, \
          schema_json, attachments_json, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(&id)
    .bind(ts)
    .bind(&from)
    .bind(&to)
    .bind(&message_type)
    .bind(&priority)
    .bind(&summary)
    .bind(&full)
    .bind(reply_to.as_deref())
    .bind(&source)
    .bind(source_room.as_deref())
    .bind(sender_mxid.as_deref())
    .bind(trust_level.as_deref())
    .bind(from_id.as_deref())
    .bind(schema_json.as_deref())
    .bind(&attachments_json)
    .bind(created_at)
    .execute(pool)
    .await?;

    get_direct_message(pool, &id)
        .await?
        .ok_or_else(|| StoreError::Invariant(format!("direct message '{id}' is missing")))
}

pub async fn read_direct_inbox(
    pool: &SqlitePool,
    agent_id: &str,
    options: InboxReadOptions,
) -> Result<Vec<DirectMessageRecord>, StoreError> {
    let agent_id = required(agent_id.to_string(), "agent id required")?;
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        direct_message_select_sql("WHERE to_agent = ? AND read_at IS NULL ORDER BY ts, id")
            .as_str(),
    )
    .bind(&agent_id)
    .fetch_all(&mut *tx)
    .await?;
    let messages = rows
        .iter()
        .map(row_to_message)
        .collect::<Result<Vec<_>, _>>()?;

    if options.drain && !messages.is_empty() {
        let read_at = now_unix();
        for message in &messages {
            sqlx::query("UPDATE direct_messages SET read_at = ? WHERE id = ? AND read_at IS NULL")
                .bind(read_at)
                .bind(&message.id)
                .execute(&mut *tx)
                .await?;
        }
    }

    tx.commit().await?;
    Ok(messages)
}

pub async fn insert_group_message(
    pool: &SqlitePool,
    input: GroupMessageInput,
) -> Result<GroupMessageRecord, StoreError> {
    let id = clean_opt(input.message_id).unwrap_or_else(generate_message_id);
    let from = required(input.from, "message from required")?;
    let group = required(input.group, "group required")?;
    let message_type = clean_opt(input.message_type).unwrap_or_else(|| "inform".to_string());
    let priority = clean_opt(input.priority).unwrap_or_else(|| "normal".to_string());
    let summary = required(input.summary, "message summary required")?;
    let full = input.full;
    let mentions = clean_dedup(input.mentions);
    let mentions_json = serde_json::to_string(&mentions)?;
    let schema_json = input
        .schema
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let attachments_json = serde_json::to_string(&input.attachments)?;
    let reply_to = clean_opt(input.reply_to);
    let source = clean_opt(input.source).unwrap_or_else(|| "api".to_string());
    let ts = input.ts.unwrap_or_else(now_unix_ms);
    let created_at = now_unix();

    sqlx::query(
        "INSERT INTO group_messages \
         (id, ts, from_agent, group_name, message_type, priority, summary, full, \
          mentions_json, reply_to, schema_json, source, attachments_json, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(&id)
    .bind(ts)
    .bind(&from)
    .bind(&group)
    .bind(&message_type)
    .bind(&priority)
    .bind(&summary)
    .bind(&full)
    .bind(&mentions_json)
    .bind(reply_to.as_deref())
    .bind(schema_json.as_deref())
    .bind(&source)
    .bind(&attachments_json)
    .bind(created_at)
    .execute(pool)
    .await?;

    get_group_message(pool, &id)
        .await?
        .ok_or_else(|| StoreError::Invariant(format!("group message '{id}' is missing")))
}

pub async fn read_agent_inbox(
    pool: &SqlitePool,
    agent_id: &str,
    options: InboxReadOptions,
) -> Result<AgentInboxReadResult, StoreError> {
    let agent_id = required(agent_id.to_string(), "agent id required")?;
    let dm = read_direct_inbox(pool, &agent_id, options).await?;
    let group = read_group_mentions(pool, &agent_id, options).await?;
    Ok(AgentInboxReadResult { dm, group })
}

pub async fn read_group_messages(
    pool: &SqlitePool,
    group: &str,
    agent_id: &str,
    options: GroupReadOptions,
) -> Result<GroupReadResult, StoreError> {
    let group = required(group.to_string(), "group required")?;
    let agent_id = required(agent_id.to_string(), "agent id required")?;
    let mut tx = pool.begin().await?;
    let rows =
        sqlx::query(group_message_select_sql("WHERE group_name = ? ORDER BY ts, rowid").as_str())
            .bind(&group)
            .fetch_all(&mut *tx)
            .await?;
    let messages = rows
        .iter()
        .map(row_to_group_message)
        .collect::<Result<Vec<_>, _>>()?;
    let read_rows = sqlx::query(
        "SELECT message_id FROM group_message_reads WHERE agent_name = ? AND group_name = ?",
    )
    .bind(&agent_id)
    .bind(&group)
    .fetch_all(&mut *tx)
    .await?;
    let read_ids = read_rows
        .iter()
        .map(|row| row.get::<String, _>("message_id"))
        .collect::<HashSet<_>>();
    let mut unread_all = Vec::new();
    let mut read_all = Vec::new();
    for message in messages {
        if read_ids.contains(&message.id) {
            read_all.push(message);
        } else {
            unread_all.push(message);
        }
    }

    if options.advance == GroupReadAdvance::All && !unread_all.is_empty() {
        let read_at = now_unix();
        for message in &unread_all {
            sqlx::query(
                "INSERT OR IGNORE INTO group_message_reads \
                 (agent_name, group_name, message_id, read_at) VALUES (?, ?, ?, ?)",
            )
            .bind(&agent_id)
            .bind(&group)
            .bind(&message.id)
            .bind(read_at)
            .execute(&mut *tx)
            .await?;
        }
    }

    let unread_total = unread_all.len();
    let unread_cap = options
        .unread_limit
        .unwrap_or(if options.advance == GroupReadAdvance::All {
            usize::MAX
        } else {
            options.limit
        })
        .max(1);
    let unread = unread_all
        .iter()
        .take(unread_cap)
        .cloned()
        .collect::<Vec<_>>();
    let unread_returned = unread.len();
    let unread_omitted = unread_total.saturating_sub(unread_returned);
    let read = read_all
        .into_iter()
        .rev()
        .take(options.limit.max(1))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();

    tx.commit().await?;
    Ok(GroupReadResult {
        group,
        unread,
        read,
        unread_total,
        unread_returned,
        unread_omitted,
        advance: options.advance,
    })
}

async fn get_direct_message(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<DirectMessageRecord>, StoreError> {
    let row = sqlx::query(direct_message_select_sql("WHERE id = ?").as_str())
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.map(|r| row_to_message(&r)).transpose()
}

async fn get_group_message(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<GroupMessageRecord>, StoreError> {
    let row = sqlx::query(group_message_select_sql("WHERE id = ?").as_str())
        .bind(id)
        .fetch_optional(pool)
        .await?;
    row.map(|row| row_to_group_message(&row)).transpose()
}

async fn group_members(pool: &SqlitePool, group: &str) -> Result<Vec<String>, StoreError> {
    let rows =
        sqlx::query("SELECT agent_name FROM group_members WHERE group_name = ? ORDER BY rowid")
            .bind(group)
            .fetch_all(pool)
            .await?;
    Ok(rows.iter().map(|row| row.get("agent_name")).collect())
}

async fn read_group_mentions(
    pool: &SqlitePool,
    agent_id: &str,
    options: InboxReadOptions,
) -> Result<Vec<GroupMessageRecord>, StoreError> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        group_message_select_sql(
            "WHERE id NOT IN (SELECT message_id FROM group_mention_reads WHERE agent_name = ?) \
             ORDER BY ts, rowid",
        )
        .as_str(),
    )
    .bind(agent_id)
    .fetch_all(&mut *tx)
    .await?;
    let messages = rows
        .iter()
        .map(row_to_group_message)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|message| {
            message
                .mentions
                .iter()
                .any(|mention| mention.eq_ignore_ascii_case(agent_id))
        })
        .collect::<Vec<_>>();
    if options.drain && !messages.is_empty() {
        let read_at = now_unix();
        for message in &messages {
            sqlx::query(
                "INSERT OR IGNORE INTO group_mention_reads (agent_name, message_id, read_at) \
                 VALUES (?, ?, ?)",
            )
            .bind(agent_id)
            .bind(&message.id)
            .bind(read_at)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(messages)
}

fn direct_message_select_sql(tail: &str) -> String {
    format!(
        "SELECT id, ts, from_agent, to_agent, message_type, priority, summary, full, \
         reply_to, source, source_room, sender_mxid, trust_level, from_id, \
         schema_json, attachments_json, read_at \
         FROM direct_messages {tail}"
    )
}

fn group_message_select_sql(tail: &str) -> String {
    format!(
        "SELECT id, ts, from_agent, group_name, message_type, priority, summary, full, \
         mentions_json, reply_to, schema_json, source, attachments_json FROM group_messages {tail}"
    )
}

fn row_to_message(row: &sqlx::sqlite::SqliteRow) -> Result<DirectMessageRecord, StoreError> {
    let ts = row.get::<i64, _>("ts");
    let schema_json: Option<String> = row.get("schema_json");
    let attachments_json: String = row.get("attachments_json");
    Ok(DirectMessageRecord {
        id: row.get("id"),
        ts,
        at: iso_utc_from_millis(ts),
        time: relative_time(ts, now_unix_ms()),
        from: row.get("from_agent"),
        to: row.get("to_agent"),
        message_type: row.get("message_type"),
        priority: row.get("priority"),
        summary: row.get("summary"),
        full: row.get("full"),
        reply_to: row.get("reply_to"),
        source: row.get("source"),
        source_room: row.get("source_room"),
        sender_mxid: row.get("sender_mxid"),
        trust_level: row.get("trust_level"),
        from_id: row.get("from_id"),
        schema: schema_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        attachments: serde_json::from_str(&attachments_json)?,
        read_at: row.get("read_at"),
    })
}

fn row_to_group_message(row: &sqlx::sqlite::SqliteRow) -> Result<GroupMessageRecord, StoreError> {
    let ts = row.get::<i64, _>("ts");
    let mentions_json: String = row.get("mentions_json");
    let schema_json: Option<String> = row.get("schema_json");
    let attachments_json: String = row.get("attachments_json");
    Ok(GroupMessageRecord {
        id: row.get("id"),
        ts,
        at: iso_utc_from_millis(ts),
        time: relative_time(ts, now_unix_ms()),
        from: row.get("from_agent"),
        group: row.get("group_name"),
        message_type: row.get("message_type"),
        priority: row.get("priority"),
        summary: row.get("summary"),
        full: row.get("full"),
        mentions: serde_json::from_str(&mentions_json)?,
        reply_to: row.get("reply_to"),
        source: row.get("source"),
        schema: schema_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?,
        attachments: serde_json::from_str(&attachments_json)?,
    })
}

fn required(value: String, message: &str) -> Result<String, StoreError> {
    clean_opt(Some(value)).ok_or_else(|| StoreError::Invariant(message.to_string()))
}

fn clean_opt(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn clean_str(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn clean_dedup(values: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for value in values {
        let Some(value) = clean_opt(Some(value)) else {
            continue;
        };
        if !out
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&value))
        {
            out.push(value);
        }
    }
    out
}

fn generate_message_id() -> String {
    format!("msg_{}", Ulid::new())
}

fn relative_time(ts: i64, now: i64) -> String {
    let diff = now.saturating_sub(ts);
    if diff < 60_000 {
        return format!("{}s ago", diff / 1000);
    }
    if diff < 3_600_000 {
        return format!("{}m ago", diff / 60_000);
    }
    if diff < 86_400_000 {
        return format!("{}h ago", diff / 3_600_000);
    }
    format!("{}d ago", diff / 86_400_000)
}

fn iso_utc_from_millis(ts_ms: i64) -> String {
    let secs = ts_ms.div_euclid(1000);
    let millis = ts_ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let second_of_day = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3600;
    let minute = (second_of_day % 3600) / 60;
    let second = second_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + i64::from(month <= 2);
    (year, month, day)
}
