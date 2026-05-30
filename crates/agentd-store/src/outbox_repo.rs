//! `mempal_outbox` operations (design §3.4). [`enqueue`] runs inside the outcome
//! transaction (so a row lands atomically with its `node_outcome`);
//! [`claim_pending`] is the FIFO read the background drainer (Task 3) consumes.
//! Nothing here calls mempal — delivery is the drainer's job.

use agentd_core::types::{MempalWrite, NodeId, RunId};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::error::StoreError;

/// A pending outbox row, in the shape the drainer needs.
#[derive(Debug, Clone)]
pub struct OutboxRow {
    pub id: i64,
    pub run_id: String,
    pub node_id: String,
    pub kind: String,
    pub payload: String,
    pub attempts: i64,
}

/// The op discriminant stored in the `kind` column.
fn outbox_kind(write: &MempalWrite) -> &'static str {
    match write {
        MempalWrite::Ingest { .. } => "ingest",
        MempalWrite::KgAdd { .. } => "kg_add",
        MempalWrite::FactCheck { .. } => "fact_check",
    }
}

/// Enqueue one row per write on `conn` — the caller's transaction (design §3.4).
///
/// # Errors
/// [`StoreError::Serde`] on a JSON-encode failure, [`StoreError::Sqlx`] on a db failure.
pub async fn enqueue(
    conn: &mut SqliteConnection,
    run_id: &RunId,
    node_id: &NodeId,
    writes: &[MempalWrite],
    enqueued_at: i64,
) -> Result<(), StoreError> {
    for write in writes {
        let payload = serde_json::to_string(write)?;
        sqlx::query(
            "INSERT INTO mempal_outbox (run_id, node_id, kind, payload, enqueued_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(run_id.as_str())
        .bind(node_id.as_str())
        .bind(outbox_kind(write))
        .bind(payload)
        .bind(enqueued_at)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

/// Pending rows (`drained_at IS NULL`) in FIFO `enqueued_at` order, up to `limit`.
///
/// # Errors
/// [`StoreError::Sqlx`] on a database failure.
pub async fn claim_pending(pool: &SqlitePool, limit: i64) -> Result<Vec<OutboxRow>, StoreError> {
    let rows = sqlx::query(
        "SELECT id, run_id, node_id, kind, payload, attempts FROM mempal_outbox \
         WHERE drained_at IS NULL ORDER BY enqueued_at ASC, id ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| OutboxRow {
            id: row.get("id"),
            run_id: row.get("run_id"),
            node_id: row.get("node_id"),
            kind: row.get("kind"),
            payload: row.get("payload"),
            attempts: row.get("attempts"),
        })
        .collect())
}
