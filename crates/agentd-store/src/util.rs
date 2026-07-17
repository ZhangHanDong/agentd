//! Small shared helpers: unix time and enum <-> TEXT column conversions.

use std::ops::{Deref, DerefMut};
use std::time::{SystemTime, UNIX_EPOCH};

use agentd_core::ports::RunStatus;
use agentd_core::types::Status;
use sqlx::pool::PoolConnection;
use sqlx::{Sqlite, SqliteConnection, SqlitePool};

use crate::error::StoreError;

/// `BEGIN IMMEDIATE` connection that cannot return an open transaction to the
/// pool when an async operation exits through `?`.
pub(crate) struct SqliteImmediateTransaction {
    connection: PoolConnection<Sqlite>,
    finished: bool,
}

impl SqliteImmediateTransaction {
    pub(crate) async fn begin(pool: &SqlitePool) -> Result<Self, sqlx::Error> {
        let mut connection = pool.acquire().await?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await?;
        Ok(Self {
            connection,
            finished: false,
        })
    }

    pub(crate) async fn commit(&mut self) -> Result<(), sqlx::Error> {
        sqlx::query("COMMIT").execute(&mut *self.connection).await?;
        self.finished = true;
        Ok(())
    }

    pub(crate) async fn rollback(&mut self) -> Result<(), sqlx::Error> {
        sqlx::query("ROLLBACK")
            .execute(&mut *self.connection)
            .await?;
        self.finished = true;
        Ok(())
    }
}

impl Deref for SqliteImmediateTransaction {
    type Target = SqliteConnection;

    fn deref(&self) -> &Self::Target {
        self.connection.as_ref()
    }
}

impl DerefMut for SqliteImmediateTransaction {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection.as_mut()
    }
}

impl Drop for SqliteImmediateTransaction {
    fn drop(&mut self) {
        if !self.finished {
            self.connection.close_on_drop();
        }
    }
}

/// Current unix time in seconds (saturating; never panics on a backwards clock).
#[must_use]
pub(crate) fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// Current unix time in milliseconds (saturating; never panics on a backwards
/// clock). Message/inbox compatibility uses agent-chat-style millisecond `ts`
/// values, unlike most execution rows which store seconds.
#[must_use]
pub(crate) fn now_unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

pub(crate) fn run_status_str(s: RunStatus) -> &'static str {
    match s {
        RunStatus::Running => "running",
        RunStatus::Parked => "parked",
        RunStatus::Finished => "finished",
        RunStatus::Failed => "failed",
    }
}

pub(crate) fn outcome_status_str(s: Status) -> &'static str {
    match s {
        Status::Success => "success",
        Status::Fail => "fail",
        Status::Retry => "retry",
        Status::PartialSuccess => "partial_success",
    }
}

pub(crate) fn parse_outcome_status(s: &str) -> Result<Status, StoreError> {
    match s {
        "success" => Ok(Status::Success),
        "fail" => Ok(Status::Fail),
        "retry" => Ok(Status::Retry),
        "partial_success" => Ok(Status::PartialSuccess),
        other => Err(StoreError::Invariant(format!(
            "unknown node outcome status '{other}'"
        ))),
    }
}
