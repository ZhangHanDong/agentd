//! The `SqliteStore` facade. P0.2 Task 1 lands connect + migrate; the
//! `agentd_core::ports::Store` trait impl and repos are wired across Tasks 2–5.

use std::path::Path;

use sqlx::SqlitePool;

use crate::error::StoreError;
use crate::pool;

/// Owns the connection pool and (once wired) implements `ports::Store`.
#[derive(Debug, Clone)]
pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Open (creating if missing) and migrate the database at `db_path`.
    ///
    /// # Errors
    /// Returns [`StoreError`] on a connection or migration failure.
    pub async fn connect(db_path: &Path) -> Result<Self, StoreError> {
        let pool = pool::open(db_path).await?;
        Ok(Self { pool })
    }

    /// Build a store around an already-open pool (tests / shared pools).
    #[must_use]
    pub fn from_pool(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// The underlying pool, for repos and inherent queries.
    #[must_use]
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}
