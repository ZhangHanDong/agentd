//! Open + migrate the `SQLite` database. Enables WAL journaling and foreign-key
//! enforcement at the connection level (a PRAGMA in the migration would be a
//! no-op inside sqlx's migration transaction).

use std::path::Path;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

use crate::error::StoreError;

/// Migrations embedded at compile time from `crates/agentd-store/migrations/`.
pub static MIGRATIONS: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Open a pool to `db_path` (creating the file and parent dir if missing),
/// then run all pending migrations.
///
/// # Errors
/// Returns [`StoreError`] on a directory-create, connection, or migration failure.
pub async fn open(db_path: &Path) -> Result<SqlitePool, StoreError> {
    if let Some(parent) = db_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let opts = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await?;
    MIGRATIONS.run(&pool).await?;
    Ok(pool)
}
