//! `human_waits` operations. Open/answer/lookup with the park-open invariant
//! `answered_at IS NULL` (parity with the in-memory fake's `answer.is_none()`).

use agentd_core::types::{NodeId, RunId};
use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::now_unix;

/// Open a human-wait row, returning its generated id.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn open_human_wait(
    pool: &SqlitePool,
    run_id: &RunId,
    node_id: &NodeId,
    prompt: &str,
) -> Result<String, StoreError> {
    let wait_id = format!("wait_{}", ulid::Ulid::new());
    sqlx::query(
        "INSERT INTO human_waits (id, run_id, node_id, prompt, opened_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&wait_id)
    .bind(run_id.as_str())
    .bind(node_id.as_str())
    .bind(prompt)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(wait_id)
}

/// Answer an open wait. Errors if the wait is unknown or already answered
/// (the `answered_at IS NULL` guard makes it idempotency-safe).
///
/// # Errors
/// Returns [`StoreError::Conflict`] if no OPEN wait matches, [`StoreError::Sqlx`] on failure.
pub async fn answer_human_wait(
    pool: &SqlitePool,
    wait_id: &str,
    answer: &str,
    feedback: Option<&str>,
) -> Result<(), StoreError> {
    let result = sqlx::query(
        "UPDATE human_waits SET answer = ?, feedback = ?, answered_at = ? \
         WHERE id = ? AND answered_at IS NULL",
    )
    .bind(answer)
    .bind(feedback)
    .bind(now_unix())
    .bind(wait_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(StoreError::Conflict(format!(
            "wait {wait_id} is unknown or already answered"
        )));
    }
    Ok(())
}

/// Resolve the parked `(run_id, node_id)` for a wait — only while still open.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn lookup_park_by_wait_id(
    pool: &SqlitePool,
    wait_id: &str,
) -> Result<Option<(RunId, NodeId)>, StoreError> {
    let row =
        sqlx::query("SELECT run_id, node_id FROM human_waits WHERE id = ? AND answered_at IS NULL")
            .bind(wait_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| {
        (
            RunId::from_string(r.get::<String, _>("run_id")),
            NodeId::from_string(r.get::<String, _>("node_id")),
        )
    }))
}
