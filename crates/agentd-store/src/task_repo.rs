//! `task_runs` operations (codergen). The park-open invariant is
//! `finished_at IS NULL`; `complete_task_run` closes it so a replayed
//! `AgentOutcomeSubmitted` resolves to `None` (parity with the fake).

use std::path::PathBuf;

use agentd_core::types::{AgentId, NativeExecutionSpec, NodeId, RunId, TaskRunId};
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

/// Insert a task run and its immutable execution input in one transaction.
pub async fn insert_task_run_with_spec(
    pool: &SqlitePool,
    run_id: &RunId,
    node_id: &NodeId,
    spec: &NativeExecutionSpec,
) -> Result<TaskRunId, StoreError> {
    spec.validate()
        .map_err(|message| StoreError::Invariant(message.into()))?;
    let encoded = serde_json::to_string(spec)?;
    let id = format!("tr_{}", ulid::Ulid::new());
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "INSERT INTO task_runs (id, run_id, node_id, status, started_at, execution_spec_json) \
         VALUES (?, ?, ?, 'running', ?, ?)",
    )
    .bind(&id)
    .bind(run_id.as_str())
    .bind(node_id.as_str())
    .bind(now_unix())
    .bind(encoded)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(TaskRunId::from_string(id))
}

/// Persist the worktree path allocated to a task run.
///
/// # Errors
/// Returns [`StoreError::NotFound`] if unknown, [`StoreError::Sqlx`] on failure.
pub async fn set_task_run_worktree(
    pool: &SqlitePool,
    task_run_id: &TaskRunId,
    worktree_path: &str,
) -> Result<(), StoreError> {
    let result = sqlx::query("UPDATE task_runs SET worktree_path = ? WHERE id = ?")
        .bind(worktree_path)
        .bind(task_run_id.as_str())
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
}

/// Persist the versioned provider execution input used by a worker claim.
pub async fn set_task_execution_spec(
    pool: &SqlitePool,
    task_run_id: &TaskRunId,
    spec: &NativeExecutionSpec,
) -> Result<(), StoreError> {
    spec.validate()
        .map_err(|message| StoreError::Invariant(message.into()))?;
    let encoded =
        serde_json::to_string(spec).map_err(|error| StoreError::Invariant(error.to_string()))?;
    let result = sqlx::query(
        "UPDATE task_runs SET execution_spec_json = ? WHERE id = ? \
         AND NOT EXISTS (SELECT 1 FROM execution_task_leases \
                         WHERE execution_task_id = task_runs.id AND status = 'active')",
    )
    .bind(encoded)
    .bind(task_run_id.as_str())
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        let exists =
            sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM task_runs WHERE id = ?)")
                .bind(task_run_id.as_str())
                .fetch_one(pool)
                .await?;
        return if exists == 1 {
            Err(StoreError::Conflict(
                "execution spec cannot change while task lease is active".into(),
            ))
        } else {
            Err(StoreError::NotFound)
        };
    }
    Ok(())
}

pub async fn get_task_execution_spec(
    pool: &SqlitePool,
    task_run_id: &TaskRunId,
) -> Result<Option<NativeExecutionSpec>, StoreError> {
    let encoded = sqlx::query_scalar::<_, Option<String>>(
        "SELECT execution_spec_json FROM task_runs WHERE id = ?",
    )
    .bind(task_run_id.as_str())
    .fetch_optional(pool)
    .await?
    .ok_or(StoreError::NotFound)?;
    encoded
        .map(|value| {
            let spec = serde_json::from_str::<NativeExecutionSpec>(&value)
                .map_err(|error| StoreError::Invariant(error.to_string()))?;
            spec.validate()
                .map_err(|message| StoreError::Invariant(message.into()))?;
            Ok(spec)
        })
        .transpose()
}

pub async fn get_task_run_run_id(
    pool: &SqlitePool,
    task_run_id: &TaskRunId,
) -> Result<RunId, StoreError> {
    let run_id = sqlx::query_scalar::<_, String>("SELECT run_id FROM task_runs WHERE id = ?")
        .bind(task_run_id.as_str())
        .fetch_optional(pool)
        .await?
        .ok_or(StoreError::NotFound)?;
    Ok(RunId::from_string(run_id))
}

/// Persist the agent id that owns a task run.
///
/// # Errors
/// Returns [`StoreError::NotFound`] if unknown, [`StoreError::Sqlx`] on failure.
pub async fn set_task_run_agent(
    pool: &SqlitePool,
    task_run_id: &TaskRunId,
    agent_id: &AgentId,
) -> Result<(), StoreError> {
    let result = sqlx::query("UPDATE task_runs SET agent_id = ? WHERE id = ?")
        .bind(agent_id.as_str())
        .bind(task_run_id.as_str())
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(StoreError::NotFound);
    }
    Ok(())
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
/// `finished_at IS NULL` — as its id plus the nullable `worktree_path` and
/// nullable `agent_id`, or `None` if there is none. This is the direction
/// `lookup_park_by_task_run` does not cover; the production `RunHost`'s
/// `open_task` resolves an agent's `submit_outcome(run, node)` into its
/// `task_run_id` through it. If more than one open row exists, the most recently
/// started is returned.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn find_open_task_run(
    pool: &SqlitePool,
    run_id: &RunId,
    node_id: &NodeId,
) -> Result<Option<(TaskRunId, Option<String>, Option<String>)>, StoreError> {
    let row = sqlx::query(
        "SELECT id, worktree_path, agent_id FROM task_runs \
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
            r.get::<Option<String>, _>("agent_id"),
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

/// List task-run worktrees still needed by non-finished workflows.
///
/// This deliberately keys off the parent run status, not `task_runs.finished_at`:
/// the codergen task can be complete while downstream verify/review/publish
/// nodes still need the same implementation tree. Failed runs are included for
/// debugging/recovery; finished runs are boot-GC cleanup candidates.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn active_task_worktree_paths(pool: &SqlitePool) -> Result<Vec<PathBuf>, StoreError> {
    let rows = sqlx::query(
        "SELECT DISTINCT task_runs.worktree_path AS worktree_path \
         FROM task_runs \
         JOIN runs ON runs.id = task_runs.run_id \
         WHERE task_runs.worktree_path IS NOT NULL \
           AND runs.status <> 'finished' \
         ORDER BY task_runs.started_at, task_runs.id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| PathBuf::from(r.get::<String, _>("worktree_path")))
        .collect())
}
