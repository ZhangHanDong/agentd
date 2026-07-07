//! Failed-run worktree cleanup queries. These are daemon/operator concerns, not
//! engine-facing `Store` trait methods.

use std::path::PathBuf;

use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::now_unix;

/// The source row for a failed-run cleanup candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailedWorktreeKind {
    /// An implementer task worktree.
    Task { task_run_id: String },
    /// A reviewer snapshot worktree.
    Review {
        review_run_id: String,
        reviewer_id: String,
    },
}

/// A pool-owned worktree that an operator may delete after a run has failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedWorktreeCleanupCandidate {
    /// The `WorktreeAllocator::release` key that must match `path`'s basename.
    pub key: String,
    /// The persisted worktree path.
    pub path: PathBuf,
    /// The store row to clear after release succeeds.
    pub kind: FailedWorktreeKind,
}

/// List worktrees tied to failed runs only.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn failed_worktree_cleanup_candidates(
    pool: &SqlitePool,
) -> Result<Vec<FailedWorktreeCleanupCandidate>, StoreError> {
    let mut candidates = failed_task_worktrees(pool).await?;
    candidates.extend(failed_review_worktrees(pool).await?);
    Ok(candidates)
}

async fn failed_task_worktrees(
    pool: &SqlitePool,
) -> Result<Vec<FailedWorktreeCleanupCandidate>, StoreError> {
    let rows = sqlx::query(
        "SELECT task_runs.id AS task_run_id, task_runs.worktree_path AS worktree_path \
         FROM task_runs \
         JOIN runs ON runs.id = task_runs.run_id \
         WHERE task_runs.worktree_path IS NOT NULL \
           AND runs.status = 'failed' \
         ORDER BY task_runs.started_at, task_runs.id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let task_run_id: String = row.get("task_run_id");
            FailedWorktreeCleanupCandidate {
                key: task_run_id.clone(),
                path: PathBuf::from(row.get::<String, _>("worktree_path")),
                kind: FailedWorktreeKind::Task { task_run_id },
            }
        })
        .collect())
}

async fn failed_review_worktrees(
    pool: &SqlitePool,
) -> Result<Vec<FailedWorktreeCleanupCandidate>, StoreError> {
    let rows = sqlx::query(
        "SELECT review_worktrees.review_run_id AS review_run_id, \
                review_worktrees.reviewer_id AS reviewer_id, \
                review_worktrees.worktree_path AS worktree_path \
         FROM review_worktrees \
         JOIN review_runs ON review_runs.id = review_worktrees.review_run_id \
         JOIN runs ON runs.id = review_runs.run_id \
         WHERE review_worktrees.released_at IS NULL \
           AND runs.status = 'failed' \
         ORDER BY review_runs.started_at, review_worktrees.reviewer_id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let review_run_id: String = row.get("review_run_id");
            let reviewer_id: String = row.get("reviewer_id");
            FailedWorktreeCleanupCandidate {
                key: format!("review-{review_run_id}-{reviewer_id}"),
                path: PathBuf::from(row.get::<String, _>("worktree_path")),
                kind: FailedWorktreeKind::Review {
                    review_run_id,
                    reviewer_id,
                },
            }
        })
        .collect())
}

/// Clear the store reference for a candidate after its pool release succeeds.
///
/// The mutation is status-gated so a stale candidate cannot clear a run that was
/// moved back to a non-failed state between dry-run and execute.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn mark_failed_worktree_released(
    pool: &SqlitePool,
    candidate: &FailedWorktreeCleanupCandidate,
) -> Result<(), StoreError> {
    match &candidate.kind {
        FailedWorktreeKind::Task { task_run_id } => {
            sqlx::query(
                "UPDATE task_runs SET worktree_path = NULL \
                 WHERE id = ? \
                   AND worktree_path = ? \
                   AND EXISTS ( \
                       SELECT 1 FROM runs \
                       WHERE runs.id = task_runs.run_id AND runs.status = 'failed' \
                   )",
            )
            .bind(task_run_id)
            .bind(candidate.path.to_string_lossy().as_ref())
            .execute(pool)
            .await?;
        }
        FailedWorktreeKind::Review {
            review_run_id,
            reviewer_id,
        } => {
            sqlx::query(
                "UPDATE review_worktrees SET released_at = ? \
                 WHERE review_run_id = ? \
                   AND reviewer_id = ? \
                   AND worktree_path = ? \
                   AND released_at IS NULL \
                   AND EXISTS ( \
                       SELECT 1 FROM review_runs \
                       JOIN runs ON runs.id = review_runs.run_id \
                       WHERE review_runs.id = review_worktrees.review_run_id \
                         AND runs.status = 'failed' \
                   )",
            )
            .bind(now_unix())
            .bind(review_run_id)
            .bind(reviewer_id)
            .bind(candidate.path.to_string_lossy().as_ref())
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}
