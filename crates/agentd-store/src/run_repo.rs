//! `runs` table operations backing the engine-facing run lifecycle methods.
//! Free functions over a `SqlitePool`; the `ports::Store` impl (Task 5) wraps them.

use agentd_core::ports::RunStatus;
use agentd_core::types::{NodeId, RunId};
use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::{now_unix, run_status_str};

/// Insert a minimal run row. Idempotent on the run id (`ON CONFLICT DO NOTHING`)
/// so a daemon-pre-created rich row is preserved when the engine later calls
/// this. `project_id`/`workflow_path`/`issue_id` stay NULL (the engine doesn't
/// have them — see the migration Δ); the store fills status + timestamps.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn insert_run(
    pool: &SqlitePool,
    run_id: &RunId,
    workflow_sha: &str,
) -> Result<(), StoreError> {
    let now = now_unix();
    sqlx::query(
        "INSERT INTO runs (id, workflow_sha, status, started_at, last_heartbeat) \
         VALUES (?, ?, 'running', ?, ?) ON CONFLICT(id) DO NOTHING",
    )
    .bind(run_id.as_str())
    .bind(workflow_sha)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Set a run's status (and `finished_at` when terminal). Errors if the run is unknown.
///
/// # Errors
/// Returns [`StoreError::NotFound`] if no run matches, [`StoreError::Sqlx`] on failure.
pub async fn update_run_status(
    pool: &SqlitePool,
    run_id: &RunId,
    status: RunStatus,
) -> Result<(), StoreError> {
    let finished_at = matches!(status, RunStatus::Finished | RunStatus::Failed).then(now_unix);
    let result = sqlx::query(
        "UPDATE runs SET status = ?, finished_at = COALESCE(?, finished_at), last_heartbeat = ? \
         WHERE id = ?",
    )
    .bind(run_status_str(status))
    .bind(finished_at)
    .bind(now_unix())
    .bind(run_id.as_str())
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

/// Create a run with its workflow file path + sha (the daemon's run-creation
/// path, P0.9). The engine-side [`insert_run`] leaves `workflow_path` NULL; this
/// sets it so the daemon can re-resolve the run's graph on deliver/resume.
/// Idempotent on the run id (fills `workflow_path` if a minimal row pre-exists).
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn record_run(
    pool: &SqlitePool,
    run_id: &RunId,
    workflow_path: &str,
    workflow_sha: &str,
) -> Result<(), StoreError> {
    let now = now_unix();
    sqlx::query(
        "INSERT INTO runs (id, workflow_path, workflow_sha, status, started_at, last_heartbeat) \
         VALUES (?, ?, ?, 'running', ?, ?) \
         ON CONFLICT(id) DO UPDATE SET workflow_path = excluded.workflow_path",
    )
    .bind(run_id.as_str())
    .bind(workflow_path)
    .bind(workflow_sha)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// The run's workflow file path (`NULL` for engine-only runs), used by the
/// daemon to re-read + re-resolve the graph on deliver/resume.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn workflow_path(
    pool: &SqlitePool,
    run_id: &RunId,
) -> Result<Option<String>, StoreError> {
    let row = sqlx::query("SELECT workflow_path FROM runs WHERE id = ?")
        .bind(run_id.as_str())
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.get::<Option<String>, _>("workflow_path")))
}

/// The run's `(status, current_node)` for `run_snapshot`, or `None` if unknown.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn read_status(
    pool: &SqlitePool,
    run_id: &RunId,
) -> Result<Option<(String, Option<String>)>, StoreError> {
    let row = sqlx::query("SELECT status, current_node FROM runs WHERE id = ?")
        .bind(run_id.as_str())
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| {
        (
            r.get::<String, _>("status"),
            r.get::<Option<String>, _>("current_node"),
        )
    }))
}

/// List every run as `(id, status, current_node, started_at)`, most-recently-
/// started first — the at-a-glance overview behind `GET /runs` (P1). No new
/// columns: the `runs` table already has all four.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn list_runs(
    pool: &SqlitePool,
) -> Result<Vec<(String, String, Option<String>, i64)>, StoreError> {
    let rows = sqlx::query(
        "SELECT id, status, current_node, started_at FROM runs ORDER BY started_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| {
            (
                r.get::<String, _>("id"),
                r.get::<String, _>("status"),
                r.get::<Option<String>, _>("current_node"),
                r.get::<i64, _>("started_at"),
            )
        })
        .collect())
}

/// Count runs that are neither finished nor failed — the in-flight runs the
/// daemon re-attaches to on boot (each resumable from its checkpoint).
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn count_in_flight(pool: &SqlitePool) -> Result<i64, StoreError> {
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM runs WHERE status NOT IN ('finished', 'failed')")
            .fetch_one(pool)
            .await?;
    Ok(count)
}

pub async fn count_in_flight_for_project(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<i64, StoreError> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM runs WHERE project_id = ? AND status NOT IN ('finished', 'failed')",
    )
    .bind(project_id)
    .fetch_one(pool)
    .await?)
}

/// Record the run's current node (the park/resume cursor). Errors if unknown.
///
/// # Errors
/// Returns [`StoreError::NotFound`] if no run matches, [`StoreError::Sqlx`] on failure.
pub async fn set_current_node(
    pool: &SqlitePool,
    run_id: &RunId,
    node_id: &NodeId,
) -> Result<(), StoreError> {
    let result = sqlx::query("UPDATE runs SET current_node = ?, last_heartbeat = ? WHERE id = ?")
        .bind(node_id.as_str())
        .bind(now_unix())
        .bind(run_id.as_str())
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}
