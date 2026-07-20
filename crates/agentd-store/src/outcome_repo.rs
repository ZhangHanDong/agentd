//! `node_outcomes` operations. The PK `(run_id, node_id, attempt)` makes attempt
//! counting (and the retry bound) a row count. `mempal_writes` are not persisted
//! in `node_outcomes`; they are enqueued into `mempal_outbox` in the SAME
//! transaction as the outcome row (design §3.4), so `latest_outcome`
//! reconstructs an empty `mempal_writes` (all the engine reads).

use agentd_core::engine::Checkpoint;
use agentd_core::types::{Artifact, NodeId, Outcome, RunId};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::error::StoreError;
use crate::util::{now_unix, outcome_status_str, parse_outcome_status};

/// Append a node outcome at the next attempt number for `(run, node)`.
///
/// # Errors
/// Returns [`StoreError::Serde`] on a JSON-encode failure, [`StoreError::Sqlx`] on db failure.
pub async fn insert_node_outcome(
    pool: &SqlitePool,
    run_id: &RunId,
    node_id: &NodeId,
    outcome: &Outcome,
) -> Result<(), StoreError> {
    // The outcome row and its outbox enqueues are one transaction (design §3.4):
    // a failure anywhere commits neither, so a partial enqueue is impossible.
    let mut tx = pool.begin().await?;
    insert_node_outcome_on_conn(&mut tx, run_id, node_id, outcome).await?;
    tx.commit().await?;
    Ok(())
}

/// Append an outcome and write the resulting checkpoint in one transaction.
///
/// # Errors
/// Returns [`StoreError::Serde`] on JSON-encode failure, [`StoreError::Sqlx`] on db failure.
pub async fn insert_node_outcome_and_checkpoint(
    pool: &SqlitePool,
    run_id: &RunId,
    node_id: &NodeId,
    outcome: &Outcome,
    checkpoint: &Checkpoint,
) -> Result<(), StoreError> {
    let mut tx = pool.begin().await?;
    insert_node_outcome_on_conn(&mut tx, run_id, node_id, outcome).await?;
    crate::checkpoint_repo::write_checkpoint_on_conn(&mut tx, checkpoint).await?;
    tx.commit().await?;
    Ok(())
}

/// Append a node outcome at the next attempt number using the caller's transaction.
///
/// # Errors
/// Returns [`StoreError::Serde`] on JSON-encode failure, [`StoreError::Sqlx`] on db failure.
pub async fn insert_node_outcome_on_conn(
    conn: &mut SqliteConnection,
    run_id: &RunId,
    node_id: &NodeId,
    outcome: &Outcome,
) -> Result<(), StoreError> {
    let next_attempt: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(attempt), 0) + 1 FROM node_outcomes WHERE run_id = ? AND node_id = ?",
    )
    .bind(run_id.as_str())
    .bind(node_id.as_str())
    .fetch_one(&mut *conn)
    .await?;

    let context_delta = serde_json::to_string(&outcome.context_updates)?;
    let artifacts = serde_json::to_string(&outcome.artifacts)?;
    let suggested_next = if outcome.suggested_next_ids.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&outcome.suggested_next_ids)?)
    };
    let now = now_unix();

    sqlx::query(
        "INSERT INTO node_outcomes \
         (run_id, node_id, attempt, status, preferred_label, suggested_next, \
          context_delta, artifacts, started_at, finished_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(run_id.as_str())
    .bind(node_id.as_str())
    .bind(next_attempt)
    .bind(outcome_status_str(outcome.status))
    .bind(outcome.preferred_label.as_deref())
    .bind(suggested_next)
    .bind(context_delta)
    .bind(artifacts)
    .bind(now)
    .bind(now)
    .execute(&mut *conn)
    .await?;

    crate::outbox_repo::enqueue(conn, run_id, node_id, &outcome.mempal_writes, now).await?;
    Ok(())
}

/// The latest (highest-attempt) outcome for `(run, node)`, or `None`.
///
/// # Errors
/// Returns [`StoreError::Serde`] on a JSON-decode failure, [`StoreError::Sqlx`] on db failure.
pub async fn latest_outcome(
    pool: &SqlitePool,
    run_id: &RunId,
    node_id: &NodeId,
) -> Result<Option<Outcome>, StoreError> {
    let row = sqlx::query(
        "SELECT status, preferred_label, suggested_next, context_delta, artifacts \
         FROM node_outcomes WHERE run_id = ? AND node_id = ? ORDER BY attempt DESC LIMIT 1",
    )
    .bind(run_id.as_str())
    .bind(node_id.as_str())
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else { return Ok(None) };
    let status = parse_outcome_status(&row.get::<String, _>("status"))?;
    let preferred_label: Option<String> = row.get("preferred_label");
    let suggested_next_ids: Vec<NodeId> = match row.get::<Option<String>, _>("suggested_next") {
        Some(json) => serde_json::from_str(&json)?,
        None => Vec::new(),
    };
    let context_updates: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(&row.get::<String, _>("context_delta"))?;
    let artifacts: Vec<Artifact> = serde_json::from_str(&row.get::<String, _>("artifacts"))?;

    Ok(Some(Outcome {
        status,
        preferred_label,
        suggested_next_ids,
        context_updates,
        artifacts,
        mempal_writes: Vec::new(),
    }))
}

/// How many attempts `(run, node)` has recorded.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn count_attempts(
    pool: &SqlitePool,
    run_id: &RunId,
    node_id: &NodeId,
) -> Result<usize, StoreError> {
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM node_outcomes WHERE run_id = ? AND node_id = ?")
            .bind(run_id.as_str())
            .bind(node_id.as_str())
            .fetch_one(pool)
            .await?;
    Ok(usize::try_from(n).unwrap_or(0))
}
