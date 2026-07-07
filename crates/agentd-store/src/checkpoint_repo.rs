//! `checkpoints` operations (1:1 with `agentd_core::engine::Checkpoint`).
//! `write_checkpoint` upserts per node; `load_checkpoint` reconstructs the
//! struct. The run must exist first (FK to `runs`); the engine inserts the run
//! before its first checkpoint.

use agentd_core::engine::Checkpoint;
use agentd_core::types::{NodeId, RunId};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::error::StoreError;
use crate::util::now_unix;

/// Upsert the run's checkpoint (called after every node).
///
/// # Errors
/// Returns [`StoreError::Serde`] on JSON-encode failure, [`StoreError::Sqlx`] on db failure.
pub async fn write_checkpoint(
    pool: &SqlitePool,
    checkpoint: &Checkpoint,
) -> Result<(), StoreError> {
    let mut conn = pool.acquire().await?;
    write_checkpoint_on_conn(&mut conn, checkpoint).await
}

/// Upsert a checkpoint using the caller's connection or transaction.
///
/// # Errors
/// Returns [`StoreError::Serde`] on JSON-encode failure, [`StoreError::Sqlx`] on db failure.
pub async fn write_checkpoint_on_conn(
    conn: &mut SqliteConnection,
    checkpoint: &Checkpoint,
) -> Result<(), StoreError> {
    let completed = serde_json::to_string(&checkpoint.completed_nodes)?;
    let retry = serde_json::to_string(&checkpoint.retry_counts)?;
    let context = serde_json::to_string(&checkpoint.context_snapshot)?;
    sqlx::query(
        "INSERT INTO checkpoints \
         (run_id, current_node, completed_nodes, retry_counts, context_snapshot, workflow_sha, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(run_id) DO UPDATE SET \
            current_node = excluded.current_node, \
            completed_nodes = excluded.completed_nodes, \
            retry_counts = excluded.retry_counts, \
            context_snapshot = excluded.context_snapshot, \
            workflow_sha = excluded.workflow_sha, \
            updated_at = excluded.updated_at",
    )
    .bind(checkpoint.run_id.as_str())
    .bind(checkpoint.current_node.as_str())
    .bind(completed)
    .bind(retry)
    .bind(context)
    .bind(&checkpoint.workflow_sha)
    .bind(now_unix())
    .execute(conn)
    .await?;
    Ok(())
}

/// Load a run's checkpoint, or `None` if it never checkpointed.
///
/// # Errors
/// Returns [`StoreError::Serde`] on JSON-decode failure, [`StoreError::Sqlx`] on db failure.
pub async fn load_checkpoint(
    pool: &SqlitePool,
    run_id: &RunId,
) -> Result<Option<Checkpoint>, StoreError> {
    let row = sqlx::query(
        "SELECT current_node, completed_nodes, retry_counts, context_snapshot, workflow_sha \
         FROM checkpoints WHERE run_id = ?",
    )
    .bind(run_id.as_str())
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else { return Ok(None) };
    Ok(Some(Checkpoint {
        run_id: run_id.clone(),
        current_node: NodeId::parsed(row.get::<String, _>("current_node")),
        completed_nodes: serde_json::from_str(&row.get::<String, _>("completed_nodes"))?,
        retry_counts: serde_json::from_str(&row.get::<String, _>("retry_counts"))?,
        context_snapshot: serde_json::from_str(&row.get::<String, _>("context_snapshot"))?,
        workflow_sha: row.get::<String, _>("workflow_sha"),
    }))
}
