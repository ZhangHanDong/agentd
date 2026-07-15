//! Enterprise agent-profile records and explicit legacy-agent aliases.

use agentd_core::types::{AgentProfileId, AgentProfileStatus};
use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::now_unix;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfileCreate {
    pub id: AgentProfileId,
    pub role: String,
    pub capability: Option<String>,
    pub runtime: String,
    pub model: Option<String>,
    pub prompt_profile: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfileRecord {
    pub id: AgentProfileId,
    pub role: String,
    pub capability: Option<String>,
    pub runtime: String,
    pub model: Option<String>,
    pub prompt_profile: Option<String>,
    pub status: AgentProfileStatus,
    pub record_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Insert a reusable enterprise agent profile in the active state.
///
/// # Errors
/// Returns [`StoreError::Invariant`] for invalid ids/required values and
/// [`StoreError::Sqlx`] for persistence conflicts.
pub async fn create_profile(
    pool: &SqlitePool,
    profile: AgentProfileCreate,
) -> Result<AgentProfileRecord, StoreError> {
    validate_id(profile.id.as_str(), "ap_", "AgentProfileId")?;
    require_text(&profile.role, "role")?;
    require_text(&profile.runtime, "runtime")?;
    let now = now_unix();
    sqlx::query(
        "INSERT INTO agent_profiles \
         (id, role, capability, runtime, model, prompt_profile, status, record_version, \
          created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, 'active', 1, ?, ?)",
    )
    .bind(profile.id.as_str())
    .bind(&profile.role)
    .bind(&profile.capability)
    .bind(&profile.runtime)
    .bind(&profile.model)
    .bind(&profile.prompt_profile)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    get_profile(pool, &profile.id)
        .await?
        .ok_or(StoreError::NotFound)
}

/// Read one agent profile by canonical id.
///
/// # Errors
/// Returns [`StoreError`] if the database row cannot be read or decoded.
pub async fn get_profile(
    pool: &SqlitePool,
    id: &AgentProfileId,
) -> Result<Option<AgentProfileRecord>, StoreError> {
    let row = sqlx::query(
        "SELECT id, role, capability, runtime, model, prompt_profile, status, record_version, \
         created_at, updated_at FROM agent_profiles WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(row_to_profile).transpose()
}

/// Transition a profile while preventing reactivation after retirement.
///
/// # Errors
/// Returns [`StoreError::NotFound`] for an unknown profile and
/// [`StoreError::Conflict`] for an invalid or concurrent transition.
pub async fn transition_profile_status(
    pool: &SqlitePool,
    id: &AgentProfileId,
    target: AgentProfileStatus,
) -> Result<AgentProfileRecord, StoreError> {
    let current = get_profile(pool, id).await?.ok_or(StoreError::NotFound)?;
    if current.status == target {
        return Ok(current);
    }
    if !profile_transition_allowed(current.status, target) {
        return Err(StoreError::Conflict(format!(
            "agent profile transition {} -> {}",
            current.status, target
        )));
    }
    let now = now_unix();
    let updated = sqlx::query(
        "UPDATE agent_profiles \
         SET status = ?, record_version = record_version + 1, updated_at = ? \
         WHERE id = ? AND status = ? AND record_version = ?",
    )
    .bind(target.as_str())
    .bind(now)
    .bind(id.as_str())
    .bind(current.status.as_str())
    .bind(current.record_version)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(
            "agent profile changed concurrently".to_string(),
        ));
    }
    get_profile(pool, id).await?.ok_or(StoreError::NotFound)
}

/// Map one existing base agent id to a canonical agent profile.
///
/// # Errors
/// Returns [`StoreError::Invariant`] for an empty legacy id or invalid profile
/// id and [`StoreError::Sqlx`] when either referenced row is absent/conflicting.
pub async fn map_legacy_agent(
    pool: &SqlitePool,
    legacy_agent_id: &str,
    profile_id: &AgentProfileId,
) -> Result<(), StoreError> {
    require_text(legacy_agent_id, "legacy_agent_id")?;
    validate_id(profile_id.as_str(), "ap_", "AgentProfileId")?;
    sqlx::query(
        "INSERT INTO legacy_agent_aliases (legacy_agent_id, agent_profile_id, created_at) \
         VALUES (?, ?, ?)",
    )
    .bind(legacy_agent_id)
    .bind(profile_id.as_str())
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

/// Resolve an existing base agent id to its explicitly mapped profile.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on database failure.
pub async fn profile_for_legacy_agent(
    pool: &SqlitePool,
    legacy_agent_id: &str,
) -> Result<Option<AgentProfileId>, StoreError> {
    let id: Option<String> = sqlx::query_scalar(
        "SELECT agent_profile_id FROM legacy_agent_aliases WHERE legacy_agent_id = ?",
    )
    .bind(legacy_agent_id)
    .fetch_optional(pool)
    .await?;
    Ok(id.map(AgentProfileId::from_string))
}

fn row_to_profile(row: &sqlx::sqlite::SqliteRow) -> Result<AgentProfileRecord, StoreError> {
    let status_text: String = row.get("status");
    let status = AgentProfileStatus::try_from(status_text.as_str()).map_err(|_| {
        StoreError::Invariant(format!("invalid agent profile status: {status_text}"))
    })?;
    Ok(AgentProfileRecord {
        id: AgentProfileId::from_string(row.get::<String, _>("id")),
        role: row.get("role"),
        capability: row.get("capability"),
        runtime: row.get("runtime"),
        model: row.get("model"),
        prompt_profile: row.get("prompt_profile"),
        status,
        record_version: row.get("record_version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn profile_transition_allowed(from: AgentProfileStatus, to: AgentProfileStatus) -> bool {
    matches!(
        (from, to),
        (
            AgentProfileStatus::Active,
            AgentProfileStatus::Disabled | AgentProfileStatus::Retired
        ) | (
            AgentProfileStatus::Disabled,
            AgentProfileStatus::Active | AgentProfileStatus::Retired
        )
    )
}

fn validate_id(value: &str, prefix: &str, label: &str) -> Result<(), StoreError> {
    let payload = value
        .strip_prefix(prefix)
        .ok_or_else(|| StoreError::Invariant(format!("invalid {label}: {value}")))?;
    if payload.len() != 26 || payload.parse::<ulid::Ulid>().is_err() {
        return Err(StoreError::Invariant(format!("invalid {label}: {value}")));
    }
    Ok(())
}

fn require_text(value: &str, field: &str) -> Result<(), StoreError> {
    if value.trim().is_empty() {
        return Err(StoreError::Invariant(format!("{field} is empty")));
    }
    Ok(())
}
