//! The `SqliteStore` facade. P0.2 Task 1 lands connect + migrate; the
//! `agentd_core::ports::Store` trait impl and repos are wired across Tasks 2–5.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::error::StoreError;
use crate::{
    pool, review_repo, task_repo,
    worktree_cleanup_repo::{self, FailedWorktreeCleanupCandidate},
};

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

    /// Pool-owned worktree paths that daemon boot-GC must preserve.
    ///
    /// This query is intentionally outside the core `Store` trait: it is a
    /// production assembly concern for daemon restart cleanup, not an engine
    /// dependency.
    ///
    /// # Errors
    /// Returns [`StoreError`] on a database failure.
    pub async fn active_worktree_paths(&self) -> Result<Vec<PathBuf>, StoreError> {
        let mut paths = task_repo::active_task_worktree_paths(&self.pool).await?;
        paths.extend(review_repo::active_review_worktree_paths(&self.pool).await?);
        Ok(paths)
    }

    /// Worktrees that can be manually released because their parent run failed.
    ///
    /// # Errors
    /// Returns [`StoreError`] on a database failure.
    pub async fn failed_worktree_cleanup_candidates(
        &self,
    ) -> Result<Vec<FailedWorktreeCleanupCandidate>, StoreError> {
        worktree_cleanup_repo::failed_worktree_cleanup_candidates(&self.pool).await
    }

    /// Clear a failed-run worktree reference after pool release succeeds.
    ///
    /// # Errors
    /// Returns [`StoreError`] on a database failure.
    pub async fn mark_failed_worktree_released(
        &self,
        candidate: &FailedWorktreeCleanupCandidate,
    ) -> Result<(), StoreError> {
        worktree_cleanup_repo::mark_failed_worktree_released(&self.pool, candidate).await
    }
}
