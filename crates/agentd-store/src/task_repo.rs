//! `task_runs` operations (codergen). The park-open invariant is
//! `finished_at IS NULL`; `complete_task_run` closes it so a replayed
//! `AgentOutcomeSubmitted` resolves to `None` (parity with the fake).

use agentd_core::types::{NodeId, RunId, TaskRunId};
use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::now_unix;

/// Insert a task run, returning its generated id.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure (e.g. an unknown run FK).
pub async fn insert_task_run(
    pool: &SqlitePool,
    run_id: &RunId,
    node_id: &NodeId,
) -> Result<TaskRunId, StoreError> {
    let id = format!("tr_{}", ulid::Ulid::new());
    sqlx::query(
        "INSERT INTO task_runs (id, run_id, node_id, status, started_at) \
         VALUES (?, ?, ?, 'running', ?)",
    )
    .bind(&id)
    .bind(run_id.as_str())
    .bind(node_id.as_str())
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(TaskRunId::from_string(id))
}

/// Mark a task run finished so it no longer parks.
///
/// # Errors
/// Returns [`StoreError::NotFound`] if unknown, [`StoreError::Sqlx`] on failure.
pub async fn complete_task_run(
    pool: &SqlitePool,
    task_run_id: &TaskRunId,
) -> Result<(), StoreError> {
    let result =
        sqlx::query("UPDATE task_runs SET finished_at = ?, status = 'finished' WHERE id = ?")
            .bind(now_unix())
            .bind(task_run_id.as_str())
            .execute(pool)
            .await?;
    if result.rows_affected() == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

/// Resolve the parked `(run_id, node_id)` — only while not yet finished.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn lookup_park_by_task_run(
    pool: &SqlitePool,
    task_run_id: &TaskRunId,
) -> Result<Option<(RunId, NodeId)>, StoreError> {
    let row =
        sqlx::query("SELECT run_id, node_id FROM task_runs WHERE id = ? AND finished_at IS NULL")
            .bind(task_run_id.as_str())
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| {
        (
            RunId::from_string(r.get::<String, _>("run_id")),
            NodeId::from_string(r.get::<String, _>("node_id")),
        )
    }))
}
