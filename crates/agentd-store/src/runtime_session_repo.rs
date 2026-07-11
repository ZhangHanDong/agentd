//! Durable logical runtime sessions and worker-bound process attempts.

use agentd_core::types::{
    AgentProfileId, RuntimeAttemptId, RuntimeAttemptStatus, RuntimeSessionId, RuntimeSessionStatus,
    TaskRunId, WorkerIncarnationId,
};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::error::StoreError;
use crate::util::now_unix;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionSnapshotRef {
    pub authority_key: String,
    pub resource_kind: String,
    pub resource_id: String,
    pub resource_version: String,
    pub content_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSessionCreate {
    pub id: RuntimeSessionId,
    pub execution_task_id: TaskRunId,
    pub agent_profile_id: AgentProfileId,
    pub snapshot: ExecutionSnapshotRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSessionRecord {
    pub id: RuntimeSessionId,
    pub execution_task_id: TaskRunId,
    pub agent_profile_id: AgentProfileId,
    pub snapshot: ExecutionSnapshotRef,
    pub status: RuntimeSessionStatus,
    pub record_version: i64,
    pub terminal_reason: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAttemptCreate {
    pub id: RuntimeAttemptId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub backend_target: Option<String>,
    pub session_name: Option<String>,
    pub pane_id: Option<String>,
    pub pid: Option<u32>,
    pub native_session_ref: Option<String>,
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAttemptRecord {
    pub id: RuntimeAttemptId,
    pub runtime_session_id: RuntimeSessionId,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub status: RuntimeAttemptStatus,
    pub backend_target: Option<String>,
    pub session_name: Option<String>,
    pub pane_id: Option<String>,
    pub pid: Option<u32>,
    pub native_session_ref: Option<String>,
    pub workdir: Option<String>,
    pub is_current: bool,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub superseded_at: Option<i64>,
}

/// Create a requested logical runtime session with an immutable snapshot tuple.
///
/// # Errors
/// Returns [`StoreError::Invariant`] for invalid ids/snapshot fields and
/// [`StoreError::Sqlx`] for unknown task/profile parents or persistence errors.
pub async fn create_session(
    pool: &SqlitePool,
    request: RuntimeSessionCreate,
) -> Result<RuntimeSessionRecord, StoreError> {
    validate_id(request.id.as_str(), "rs_", "RuntimeSessionId")?;
    validate_id(request.agent_profile_id.as_str(), "ap_", "AgentProfileId")?;
    validate_snapshot(&request.snapshot)?;
    let now = now_unix();
    sqlx::query(
        "INSERT INTO runtime_sessions \
         (id, execution_task_id, agent_profile_id, snapshot_authority_key, \
          snapshot_resource_kind, snapshot_resource_id, snapshot_resource_version, \
          snapshot_content_sha256, status, record_version, terminal_reason, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'requested', 1, NULL, ?, ?)",
    )
    .bind(request.id.as_str())
    .bind(request.execution_task_id.as_str())
    .bind(request.agent_profile_id.as_str())
    .bind(&request.snapshot.authority_key)
    .bind(&request.snapshot.resource_kind)
    .bind(&request.snapshot.resource_id)
    .bind(&request.snapshot.resource_version)
    .bind(&request.snapshot.content_sha256)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    get_session(pool, &request.id)
        .await?
        .ok_or(StoreError::NotFound)
}

/// Read one logical runtime session.
///
/// # Errors
/// Returns [`StoreError`] if the row cannot be read or decoded.
pub async fn get_session(
    pool: &SqlitePool,
    id: &RuntimeSessionId,
) -> Result<Option<RuntimeSessionRecord>, StoreError> {
    let row = sqlx::query(
        "SELECT id, execution_task_id, agent_profile_id, snapshot_authority_key, \
         snapshot_resource_kind, snapshot_resource_id, snapshot_resource_version, \
         snapshot_content_sha256, status, record_version, terminal_reason, created_at, updated_at \
         FROM runtime_sessions WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(row_to_session).transpose()
}

/// Start one process attempt for a requested/resume-pending session on a
/// current worker incarnation.
///
/// # Errors
/// Returns [`StoreError::Conflict`] for invalid session/worker state,
/// [`StoreError::NotFound`] for unknown parents, and [`StoreError`] for invalid
/// ids or persistence failures.
pub async fn start_attempt(
    pool: &SqlitePool,
    session_id: &RuntimeSessionId,
    request: RuntimeAttemptCreate,
) -> Result<RuntimeAttemptRecord, StoreError> {
    validate_attempt_request(session_id, &request)?;
    let mut tx = pool.begin().await?;
    let (status, version) = load_startable_session(&mut tx, session_id).await?;
    require_current_worker(&mut tx, &request.worker_incarnation_id).await?;
    let now = now_unix();
    supersede_current_attempt(&mut tx, session_id, status, now).await?;
    insert_attempt(&mut tx, session_id, &request, now).await?;
    let updated = sqlx::query(
        "UPDATE runtime_sessions \
         SET status = 'starting', record_version = record_version + 1, \
             terminal_reason = NULL, updated_at = ? \
         WHERE id = ? AND status = ? AND record_version = ?",
    )
    .bind(now)
    .bind(session_id.as_str())
    .bind(status.as_str())
    .bind(version)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(
            "runtime session changed concurrently".to_string(),
        ));
    }
    tx.commit().await?;
    get_attempt(pool, &request.id)
        .await?
        .ok_or(StoreError::NotFound)
}

fn validate_attempt_request(
    session_id: &RuntimeSessionId,
    request: &RuntimeAttemptCreate,
) -> Result<(), StoreError> {
    validate_id(session_id.as_str(), "rs_", "RuntimeSessionId")?;
    validate_id(request.id.as_str(), "ra_", "RuntimeAttemptId")?;
    validate_id(
        request.worker_incarnation_id.as_str(),
        "wi_",
        "WorkerIncarnationId",
    )?;
    if matches!(request.pid, Some(0)) {
        return Err(StoreError::Invariant("pid must be positive".to_string()));
    }
    Ok(())
}

async fn load_startable_session(
    tx: &mut Transaction<'_, Sqlite>,
    session_id: &RuntimeSessionId,
) -> Result<(RuntimeSessionStatus, i64), StoreError> {
    let row = sqlx::query("SELECT status, record_version FROM runtime_sessions WHERE id = ?")
        .bind(session_id.as_str())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(StoreError::NotFound)?;
    let status_text: String = row.get("status");
    let status = RuntimeSessionStatus::try_from(status_text.as_str()).map_err(|_| {
        StoreError::Invariant(format!("invalid runtime session status: {status_text}"))
    })?;
    if !matches!(
        status,
        RuntimeSessionStatus::Requested | RuntimeSessionStatus::ResumePending
    ) {
        return Err(StoreError::Conflict(format!(
            "runtime session {session_id} cannot start attempt from {status}"
        )));
    }
    Ok((status, row.get("record_version")))
}

async fn require_current_worker(
    tx: &mut Transaction<'_, Sqlite>,
    incarnation_id: &WorkerIncarnationId,
) -> Result<(), StoreError> {
    let is_current: Option<i64> =
        sqlx::query_scalar("SELECT is_current FROM worker_incarnations WHERE id = ?")
            .bind(incarnation_id.as_str())
            .fetch_optional(&mut **tx)
            .await?;
    match is_current {
        None => Err(StoreError::NotFound),
        Some(0) => Err(StoreError::Conflict(
            "worker incarnation is superseded".to_string(),
        )),
        Some(_) => Ok(()),
    }
}

async fn supersede_current_attempt(
    tx: &mut Transaction<'_, Sqlite>,
    session_id: &RuntimeSessionId,
    session_status: RuntimeSessionStatus,
    now: i64,
) -> Result<(), StoreError> {
    let current_id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM runtime_attempts WHERE runtime_session_id = ? AND is_current = 1",
    )
    .bind(session_id.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    let Some(current_id) = current_id else {
        return Ok(());
    };
    if session_status != RuntimeSessionStatus::ResumePending {
        return Err(StoreError::Conflict(
            "runtime session already has a current attempt".to_string(),
        ));
    }
    sqlx::query(
        "UPDATE runtime_attempts \
         SET status = 'gone', is_current = 0, finished_at = COALESCE(finished_at, ?), \
             superseded_at = ? WHERE id = ? AND is_current = 1",
    )
    .bind(now)
    .bind(now)
    .bind(current_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_attempt(
    tx: &mut Transaction<'_, Sqlite>,
    session_id: &RuntimeSessionId,
    request: &RuntimeAttemptCreate,
    now: i64,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO runtime_attempts \
         (id, runtime_session_id, worker_incarnation_id, status, backend_target, session_name, \
          pane_id, pid, native_session_ref, workdir, is_current, started_at, finished_at, \
          superseded_at) \
         VALUES (?, ?, ?, 'starting', ?, ?, ?, ?, ?, ?, 1, ?, NULL, NULL)",
    )
    .bind(request.id.as_str())
    .bind(session_id.as_str())
    .bind(request.worker_incarnation_id.as_str())
    .bind(&request.backend_target)
    .bind(&request.session_name)
    .bind(&request.pane_id)
    .bind(request.pid.map(i64::from))
    .bind(&request.native_session_ref)
    .bind(&request.workdir)
    .bind(now)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Mark the current attempt gone and move its logical session to
/// `resume_pending` atomically.
///
/// # Errors
/// Returns [`StoreError::Conflict`] when the attempt is stale/terminal or the
/// session cannot recover, and [`StoreError::NotFound`] for unknown ids.
pub async fn mark_attempt_gone(
    pool: &SqlitePool,
    session_id: &RuntimeSessionId,
    attempt_id: &RuntimeAttemptId,
) -> Result<RuntimeAttemptRecord, StoreError> {
    let mut tx = pool.begin().await?;
    let attempt_row = sqlx::query(
        "SELECT status, is_current FROM runtime_attempts \
         WHERE id = ? AND runtime_session_id = ?",
    )
    .bind(attempt_id.as_str())
    .bind(session_id.as_str())
    .fetch_optional(&mut *tx)
    .await?
    .ok_or(StoreError::NotFound)?;
    let attempt_status_text: String = attempt_row.get("status");
    let attempt_status =
        RuntimeAttemptStatus::try_from(attempt_status_text.as_str()).map_err(|_| {
            StoreError::Invariant(format!(
                "invalid runtime attempt status: {attempt_status_text}"
            ))
        })?;
    if attempt_row.get::<i64, _>("is_current") == 0 || attempt_status.is_terminal() {
        return Err(StoreError::Conflict(
            "runtime attempt is not current".to_string(),
        ));
    }

    let session_row =
        sqlx::query("SELECT status, record_version FROM runtime_sessions WHERE id = ?")
            .bind(session_id.as_str())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StoreError::NotFound)?;
    let session_status_text: String = session_row.get("status");
    let session_status =
        RuntimeSessionStatus::try_from(session_status_text.as_str()).map_err(|_| {
            StoreError::Invariant(format!(
                "invalid runtime session status: {session_status_text}"
            ))
        })?;
    if !matches!(
        session_status,
        RuntimeSessionStatus::Starting | RuntimeSessionStatus::Running
    ) {
        return Err(StoreError::Conflict(format!(
            "runtime session cannot become resume_pending from {session_status}"
        )));
    }

    let now = now_unix();
    sqlx::query(
        "UPDATE runtime_attempts \
         SET status = 'gone', is_current = 0, finished_at = ?, superseded_at = ? \
         WHERE id = ? AND is_current = 1",
    )
    .bind(now)
    .bind(now)
    .bind(attempt_id.as_str())
    .execute(&mut *tx)
    .await?;
    let updated = sqlx::query(
        "UPDATE runtime_sessions \
         SET status = 'resume_pending', record_version = record_version + 1, updated_at = ? \
         WHERE id = ? AND status = ? AND record_version = ?",
    )
    .bind(now)
    .bind(session_id.as_str())
    .bind(session_status.as_str())
    .bind(session_row.get::<i64, _>("record_version"))
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(
            "runtime session changed concurrently".to_string(),
        ));
    }
    tx.commit().await?;
    get_attempt(pool, attempt_id)
        .await?
        .ok_or(StoreError::NotFound)
}

/// Transition session status with optimistic versioning and terminal-state
/// protection.
///
/// # Errors
/// Returns [`StoreError::Conflict`] for invalid/concurrent transitions and
/// [`StoreError::NotFound`] for an unknown session.
pub async fn transition_session_status(
    pool: &SqlitePool,
    id: &RuntimeSessionId,
    target: RuntimeSessionStatus,
    terminal_reason: Option<&str>,
) -> Result<RuntimeSessionRecord, StoreError> {
    let current = get_session(pool, id).await?.ok_or(StoreError::NotFound)?;
    if current.status == target {
        return Ok(current);
    }
    if !session_transition_allowed(current.status, target) {
        return Err(StoreError::Conflict(format!(
            "runtime session transition {} -> {}",
            current.status, target
        )));
    }
    let now = now_unix();
    let mut tx = pool.begin().await?;
    let updated = sqlx::query(
        "UPDATE runtime_sessions \
         SET status = ?, record_version = record_version + 1, terminal_reason = ?, updated_at = ? \
         WHERE id = ? AND status = ? AND record_version = ?",
    )
    .bind(target.as_str())
    .bind(terminal_reason)
    .bind(now)
    .bind(id.as_str())
    .bind(current.status.as_str())
    .bind(current.record_version)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(
            "runtime session changed concurrently".to_string(),
        ));
    }
    if target.is_terminal() {
        sqlx::query(
            "UPDATE runtime_attempts \
             SET status = 'gone', is_current = 0, finished_at = COALESCE(finished_at, ?), \
                 superseded_at = ? \
             WHERE runtime_session_id = ? AND is_current = 1",
        )
        .bind(now)
        .bind(now)
        .bind(id.as_str())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    get_session(pool, id).await?.ok_or(StoreError::NotFound)
}

/// Read one runtime attempt.
///
/// # Errors
/// Returns [`StoreError`] if the row cannot be read or decoded.
pub async fn get_attempt(
    pool: &SqlitePool,
    id: &RuntimeAttemptId,
) -> Result<Option<RuntimeAttemptRecord>, StoreError> {
    let row = sqlx::query(
        "SELECT id, runtime_session_id, worker_incarnation_id, status, backend_target, \
         session_name, pane_id, pid, native_session_ref, workdir, is_current, started_at, \
         finished_at, superseded_at FROM runtime_attempts WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(row_to_attempt).transpose()
}

/// Read the only current process attempt for a logical runtime session.
///
/// # Errors
/// Returns [`StoreError`] if the row cannot be read or decoded.
pub async fn current_attempt(
    pool: &SqlitePool,
    session_id: &RuntimeSessionId,
) -> Result<Option<RuntimeAttemptRecord>, StoreError> {
    let row = sqlx::query(
        "SELECT id, runtime_session_id, worker_incarnation_id, status, backend_target, \
         session_name, pane_id, pid, native_session_ref, workdir, is_current, started_at, \
         finished_at, superseded_at FROM runtime_attempts \
         WHERE runtime_session_id = ? AND is_current = 1",
    )
    .bind(session_id.as_str())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(row_to_attempt).transpose()
}

fn row_to_session(row: &sqlx::sqlite::SqliteRow) -> Result<RuntimeSessionRecord, StoreError> {
    let status_text: String = row.get("status");
    Ok(RuntimeSessionRecord {
        id: RuntimeSessionId::from_string(row.get::<String, _>("id")),
        execution_task_id: TaskRunId::from_string(row.get::<String, _>("execution_task_id")),
        agent_profile_id: AgentProfileId::from_string(row.get::<String, _>("agent_profile_id")),
        snapshot: ExecutionSnapshotRef {
            authority_key: row.get("snapshot_authority_key"),
            resource_kind: row.get("snapshot_resource_kind"),
            resource_id: row.get("snapshot_resource_id"),
            resource_version: row.get("snapshot_resource_version"),
            content_sha256: row.get("snapshot_content_sha256"),
        },
        status: RuntimeSessionStatus::try_from(status_text.as_str()).map_err(|_| {
            StoreError::Invariant(format!("invalid runtime session status: {status_text}"))
        })?,
        record_version: row.get("record_version"),
        terminal_reason: row.get("terminal_reason"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

fn row_to_attempt(row: &sqlx::sqlite::SqliteRow) -> Result<RuntimeAttemptRecord, StoreError> {
    let status_text: String = row.get("status");
    let pid = row
        .get::<Option<i64>, _>("pid")
        .map(u32::try_from)
        .transpose()
        .map_err(|_| StoreError::Invariant("runtime attempt pid out of range".to_string()))?;
    Ok(RuntimeAttemptRecord {
        id: RuntimeAttemptId::from_string(row.get::<String, _>("id")),
        runtime_session_id: RuntimeSessionId::from_string(
            row.get::<String, _>("runtime_session_id"),
        ),
        worker_incarnation_id: WorkerIncarnationId::from_string(
            row.get::<String, _>("worker_incarnation_id"),
        ),
        status: RuntimeAttemptStatus::try_from(status_text.as_str()).map_err(|_| {
            StoreError::Invariant(format!("invalid runtime attempt status: {status_text}"))
        })?,
        backend_target: row.get("backend_target"),
        session_name: row.get("session_name"),
        pane_id: row.get("pane_id"),
        pid,
        native_session_ref: row.get("native_session_ref"),
        workdir: row.get("workdir"),
        is_current: row.get::<i64, _>("is_current") != 0,
        started_at: row.get("started_at"),
        finished_at: row.get("finished_at"),
        superseded_at: row.get("superseded_at"),
    })
}

fn session_transition_allowed(from: RuntimeSessionStatus, to: RuntimeSessionStatus) -> bool {
    matches!(
        (from, to),
        (
            RuntimeSessionStatus::Requested,
            RuntimeSessionStatus::Starting | RuntimeSessionStatus::Cancelled
        ) | (
            RuntimeSessionStatus::Starting,
            RuntimeSessionStatus::Running
                | RuntimeSessionStatus::ResumePending
                | RuntimeSessionStatus::Failed
                | RuntimeSessionStatus::Cancelled
                | RuntimeSessionStatus::Lost
        ) | (
            RuntimeSessionStatus::Running,
            RuntimeSessionStatus::ResumePending
                | RuntimeSessionStatus::Completed
                | RuntimeSessionStatus::Failed
                | RuntimeSessionStatus::Cancelled
                | RuntimeSessionStatus::Lost
        ) | (
            RuntimeSessionStatus::ResumePending,
            RuntimeSessionStatus::Starting
                | RuntimeSessionStatus::Cancelled
                | RuntimeSessionStatus::Lost
        )
    )
}

fn validate_snapshot(snapshot: &ExecutionSnapshotRef) -> Result<(), StoreError> {
    for (field, value) in [
        ("authority_key", snapshot.authority_key.as_str()),
        ("resource_id", snapshot.resource_id.as_str()),
        ("resource_version", snapshot.resource_version.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(StoreError::Invariant(format!("snapshot {field} is empty")));
        }
    }
    if snapshot.resource_kind != "execution_snapshot" {
        return Err(StoreError::Invariant(
            "snapshot resource_kind must be execution_snapshot".to_string(),
        ));
    }
    if snapshot.content_sha256.len() != 64
        || !snapshot
            .content_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(StoreError::Invariant(
            "snapshot content_sha256 must be lowercase hex".to_string(),
        ));
    }
    Ok(())
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
