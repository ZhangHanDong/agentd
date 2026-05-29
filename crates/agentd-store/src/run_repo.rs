//! `runs` table operations backing the engine-facing run lifecycle methods.
//! Free functions over a `SqlitePool`; the `ports::Store` impl (Task 5) wraps them.

use agentd_core::ports::RunStatus;
use agentd_core::types::{NodeId, RunId};
use sqlx::SqlitePool;

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
