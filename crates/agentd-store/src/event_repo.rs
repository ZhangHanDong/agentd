//! `events` table operations: the append-only broadcast log (design §4) that
//! backs SSE replay. `append` writes one row and returns its autoincrement
//! `seq`; `read_from` returns a run's events after a `seq` cursor, in order —
//! the replay primitive the HTTP+SSE surface reads through (P0.7 7b).
//!
//! Free functions over a `SqlitePool`, like the other repos. The emit point
//! (one event per `RunProgress`) is the daemon's job (P0.9); this is only the
//! persistence + cursor read.

use agentd_core::types::RunId;
use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::now_unix;

/// One row of the `events` log.
#[derive(Debug, Clone)]
pub struct EventRow {
    pub seq: i64,
    pub run_id: String,
    pub kind: String,
    pub payload: String,
    pub emitted_at: i64,
}

/// Append one event for a run and return its new `seq` (the autoincrement PK,
/// strictly increasing across the database). `payload` is opaque JSON text.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure — including the foreign-key
/// violation when `run_id` names a run that does not exist.
pub async fn append(
    pool: &SqlitePool,
    run_id: &RunId,
    kind: &str,
    payload: &str,
) -> Result<i64, StoreError> {
    let seq: i64 = sqlx::query_scalar(
        "INSERT INTO events (run_id, kind, payload, emitted_at) VALUES (?, ?, ?, ?) RETURNING seq",
    )
    .bind(run_id.as_str())
    .bind(kind)
    .bind(payload)
    .bind(now_unix())
    .fetch_one(pool)
    .await?;
    Ok(seq)
}

/// Return a run's events with `seq > after_seq`, ordered by `seq` ascending —
/// the SSE replay cursor. Pass `after_seq = 0` to read from the start. Uses the
/// `idx_events_run_seq` index.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn read_from(
    pool: &SqlitePool,
    run_id: &RunId,
    after_seq: i64,
) -> Result<Vec<EventRow>, StoreError> {
    let rows = sqlx::query(
        "SELECT seq, run_id, kind, payload, emitted_at FROM events \
         WHERE run_id = ? AND seq > ? ORDER BY seq ASC",
    )
    .bind(run_id.as_str())
    .bind(after_seq)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| EventRow {
            seq: row.get("seq"),
            run_id: row.get("run_id"),
            kind: row.get("kind"),
            payload: row.get("payload"),
            emitted_at: row.get("emitted_at"),
        })
        .collect())
}
