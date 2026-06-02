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

/// Forward read: the OPEN task run for `(run_id, node_id)` — the one with
/// `finished_at IS NULL` — as its id plus the nullable `worktree_path`, or
/// `None` if there is none. This is the direction `lookup_park_by_task_run` does
/// not cover; the production `RunHost`'s `open_task` resolves an agent's
/// `submit_outcome(run, node)` into its `task_run_id` through it. If more than
/// one open row exists, the most recently started is returned.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn find_open_task_run(
    pool: &SqlitePool,
    run_id: &RunId,
    node_id: &NodeId,
) -> Result<Option<(TaskRunId, Option<String>)>, StoreError> {
    let row = sqlx::query(
        "SELECT id, worktree_path FROM task_runs \
         WHERE run_id = ? AND node_id = ? AND finished_at IS NULL \
         ORDER BY started_at DESC LIMIT 1",
    )
    .bind(run_id.as_str())
    .bind(node_id.as_str())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| {
        (
            TaskRunId::from_string(r.get::<String, _>("id")),
            r.get::<Option<String>, _>("worktree_path"),
        )
    }))
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
