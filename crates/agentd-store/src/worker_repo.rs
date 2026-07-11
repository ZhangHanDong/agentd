//! Stable worker enrollment and fenced daemon-incarnation records.

use agentd_core::types::{WorkerId, WorkerIncarnationId, WorkerStatus};
use serde_json::Value;
use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::now_unix;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerCreate {
    pub id: WorkerId,
    pub trust_domain: String,
    pub labels: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRecord {
    pub id: WorkerId,
    pub status: WorkerStatus,
    pub trust_domain: String,
    pub labels: Value,
    pub record_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub retired_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRegistration {
    pub id: WorkerIncarnationId,
    pub daemon_version: String,
    pub host_name: String,
    pub network_zone: Option<String>,
    pub capabilities: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerIncarnationRecord {
    pub id: WorkerIncarnationId,
    pub worker_id: WorkerId,
    pub daemon_version: String,
    pub host_name: String,
    pub network_zone: Option<String>,
    pub capabilities: Value,
    pub is_current: bool,
    pub registered_at: i64,
    pub last_seen_at: i64,
    pub superseded_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerHeartbeatOutcome {
    Accepted(WorkerIncarnationRecord),
    Stale,
}

/// Enroll a stable worker in the offline state.
///
/// # Errors
/// Returns [`StoreError::Invariant`] for invalid input and [`StoreError::Sqlx`]
/// for database conflicts.
pub async fn create_worker(
    pool: &SqlitePool,
    worker: WorkerCreate,
) -> Result<WorkerRecord, StoreError> {
    validate_id(worker.id.as_str(), "wk_", "WorkerId")?;
    require_text(&worker.trust_domain, "trust_domain")?;
    let labels_json = serde_json::to_string(&worker.labels)?;
    let now = now_unix();
    sqlx::query(
        "INSERT INTO workers \
         (id, status, trust_domain, labels_json, record_version, created_at, updated_at) \
         VALUES (?, 'offline', ?, ?, 1, ?, ?)",
    )
    .bind(worker.id.as_str())
    .bind(&worker.trust_domain)
    .bind(labels_json)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    get_worker(pool, &worker.id)
        .await?
        .ok_or(StoreError::NotFound)
}

/// Read one stable worker enrollment.
///
/// # Errors
/// Returns [`StoreError`] if the row cannot be read or decoded.
pub async fn get_worker(
    pool: &SqlitePool,
    id: &WorkerId,
) -> Result<Option<WorkerRecord>, StoreError> {
    let row = sqlx::query(
        "SELECT id, status, trust_domain, labels_json, record_version, created_at, \
         updated_at, retired_at FROM workers WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(row_to_worker).transpose()
}

/// Register a new daemon incarnation and atomically supersede the old one.
///
/// # Errors
/// Returns [`StoreError::Conflict`] for a retired worker,
/// [`StoreError::NotFound`] for an unknown worker, and [`StoreError`] for input
/// or persistence failures.
pub async fn register_incarnation(
    pool: &SqlitePool,
    worker_id: &WorkerId,
    registration: WorkerRegistration,
) -> Result<WorkerIncarnationRecord, StoreError> {
    validate_id(worker_id.as_str(), "wk_", "WorkerId")?;
    validate_id(registration.id.as_str(), "wi_", "WorkerIncarnationId")?;
    require_text(&registration.daemon_version, "daemon_version")?;
    require_text(&registration.host_name, "host_name")?;
    let capabilities_json = serde_json::to_string(&registration.capabilities)?;
    let mut tx = pool.begin().await?;
    let worker_row = sqlx::query("SELECT status FROM workers WHERE id = ?")
        .bind(worker_id.as_str())
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(StoreError::NotFound)?;
    let status_text: String = worker_row.get("status");
    let status = WorkerStatus::try_from(status_text.as_str())
        .map_err(|_| StoreError::Invariant(format!("invalid worker status: {status_text}")))?;
    if status.is_terminal() {
        return Err(StoreError::Conflict(
            "retired worker cannot register".to_string(),
        ));
    }

    let now = now_unix();
    sqlx::query(
        "UPDATE worker_incarnations \
         SET is_current = 0, superseded_at = ? \
         WHERE worker_id = ? AND is_current = 1",
    )
    .bind(now)
    .bind(worker_id.as_str())
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO worker_incarnations \
         (id, worker_id, daemon_version, host_name, network_zone, capabilities_json, \
          is_current, registered_at, last_seen_at, superseded_at) \
         VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, NULL)",
    )
    .bind(registration.id.as_str())
    .bind(worker_id.as_str())
    .bind(&registration.daemon_version)
    .bind(&registration.host_name)
    .bind(&registration.network_zone)
    .bind(capabilities_json)
    .bind(now)
    .bind(now)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE workers \
         SET status = 'online', record_version = record_version + 1, \
             updated_at = ?, retired_at = NULL \
         WHERE id = ?",
    )
    .bind(now)
    .bind(worker_id.as_str())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    get_incarnation(pool, &registration.id)
        .await?
        .ok_or(StoreError::NotFound)
}

/// Read one worker daemon incarnation.
///
/// # Errors
/// Returns [`StoreError`] if the row cannot be read or decoded.
pub async fn get_incarnation(
    pool: &SqlitePool,
    id: &WorkerIncarnationId,
) -> Result<Option<WorkerIncarnationRecord>, StoreError> {
    let row = sqlx::query(
        "SELECT id, worker_id, daemon_version, host_name, network_zone, capabilities_json, \
         is_current, registered_at, last_seen_at, superseded_at \
         FROM worker_incarnations WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(row_to_incarnation).transpose()
}

/// Read the only current incarnation for a worker.
///
/// # Errors
/// Returns [`StoreError`] if the row cannot be read or decoded.
pub async fn current_incarnation(
    pool: &SqlitePool,
    worker_id: &WorkerId,
) -> Result<Option<WorkerIncarnationRecord>, StoreError> {
    let row = sqlx::query(
        "SELECT id, worker_id, daemon_version, host_name, network_zone, capabilities_json, \
         is_current, registered_at, last_seen_at, superseded_at \
         FROM worker_incarnations WHERE worker_id = ? AND is_current = 1",
    )
    .bind(worker_id.as_str())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(row_to_incarnation).transpose()
}

/// Accept heartbeat only from the current incarnation.
///
/// # Errors
/// Returns [`StoreError::NotFound`] when the worker/incarnation pair is unknown
/// and [`StoreError`] on database/decoding failure.
pub async fn heartbeat_incarnation(
    pool: &SqlitePool,
    worker_id: &WorkerId,
    incarnation_id: &WorkerIncarnationId,
) -> Result<WorkerHeartbeatOutcome, StoreError> {
    let now = now_unix();
    let updated = sqlx::query(
        "UPDATE worker_incarnations SET last_seen_at = ? \
         WHERE id = ? AND worker_id = ? AND is_current = 1",
    )
    .bind(now)
    .bind(incarnation_id.as_str())
    .bind(worker_id.as_str())
    .execute(pool)
    .await?;
    if updated.rows_affected() == 1 {
        sqlx::query("UPDATE workers SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(worker_id.as_str())
            .execute(pool)
            .await?;
        let incarnation = get_incarnation(pool, incarnation_id)
            .await?
            .ok_or(StoreError::NotFound)?;
        return Ok(WorkerHeartbeatOutcome::Accepted(incarnation));
    }

    let known: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM worker_incarnations WHERE id = ? AND worker_id = ?",
    )
    .bind(incarnation_id.as_str())
    .bind(worker_id.as_str())
    .fetch_one(pool)
    .await?;
    if known == 1 {
        Ok(WorkerHeartbeatOutcome::Stale)
    } else {
        Err(StoreError::NotFound)
    }
}

/// Transition a worker and supersede its current incarnation when it goes
/// offline or retires.
///
/// # Errors
/// Returns [`StoreError::Conflict`] for invalid/concurrent transitions and
/// [`StoreError::NotFound`] for an unknown worker.
pub async fn transition_worker_status(
    pool: &SqlitePool,
    id: &WorkerId,
    target: WorkerStatus,
) -> Result<WorkerRecord, StoreError> {
    let current = get_worker(pool, id).await?.ok_or(StoreError::NotFound)?;
    if current.status == target {
        return Ok(current);
    }
    if !worker_transition_allowed(current.status, target) {
        return Err(StoreError::Conflict(format!(
            "worker transition {} -> {}",
            current.status, target
        )));
    }

    let now = now_unix();
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE workers \
         SET status = ?, record_version = record_version + 1, updated_at = ?, \
             retired_at = CASE WHEN ? = 'retired' THEN ? ELSE retired_at END \
         WHERE id = ? AND status = ? AND record_version = ?",
    )
    .bind(target.as_str())
    .bind(now)
    .bind(target.as_str())
    .bind(now)
    .bind(id.as_str())
    .bind(current.status.as_str())
    .bind(current.record_version)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(
            "worker changed concurrently".to_string(),
        ));
    }
    if matches!(target, WorkerStatus::Offline | WorkerStatus::Retired) {
        sqlx::query(
            "UPDATE worker_incarnations SET is_current = 0, superseded_at = ? \
             WHERE worker_id = ? AND is_current = 1",
        )
        .bind(now)
        .bind(id.as_str())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    get_worker(pool, id).await?.ok_or(StoreError::NotFound)
}

fn row_to_worker(row: &sqlx::sqlite::SqliteRow) -> Result<WorkerRecord, StoreError> {
    let status_text: String = row.get("status");
    let labels_json: String = row.get("labels_json");
    Ok(WorkerRecord {
        id: WorkerId::from_string(row.get::<String, _>("id")),
        status: WorkerStatus::try_from(status_text.as_str())
            .map_err(|_| StoreError::Invariant(format!("invalid worker status: {status_text}")))?,
        trust_domain: row.get("trust_domain"),
        labels: serde_json::from_str(&labels_json)?,
        record_version: row.get("record_version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
        retired_at: row.get("retired_at"),
    })
}

fn row_to_incarnation(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<WorkerIncarnationRecord, StoreError> {
    let capabilities_json: String = row.get("capabilities_json");
    Ok(WorkerIncarnationRecord {
        id: WorkerIncarnationId::from_string(row.get::<String, _>("id")),
        worker_id: WorkerId::from_string(row.get::<String, _>("worker_id")),
        daemon_version: row.get("daemon_version"),
        host_name: row.get("host_name"),
        network_zone: row.get("network_zone"),
        capabilities: serde_json::from_str(&capabilities_json)?,
        is_current: row.get::<i64, _>("is_current") != 0,
        registered_at: row.get("registered_at"),
        last_seen_at: row.get("last_seen_at"),
        superseded_at: row.get("superseded_at"),
    })
}

fn worker_transition_allowed(from: WorkerStatus, to: WorkerStatus) -> bool {
    matches!(
        (from, to),
        (
            WorkerStatus::Online,
            WorkerStatus::Draining | WorkerStatus::Offline | WorkerStatus::Retired
        ) | (
            WorkerStatus::Draining,
            WorkerStatus::Online | WorkerStatus::Offline | WorkerStatus::Retired
        ) | (WorkerStatus::Offline, WorkerStatus::Retired)
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
