//! `projects` table — minimal insert + count for the daemon smoke wiring.
//! (Projects are seeded by the daemon/Specify client, not the engine.)

use sqlx::SqlitePool;

use crate::error::StoreError;
use crate::util::now_unix;

/// Insert a project (idempotent on id).
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn insert_project(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    repo_path: &str,
    mempal_wing: &str,
) -> Result<(), StoreError> {
    let now = now_unix();
    sqlx::query(
        "INSERT INTO projects (id, name, repo_path, mempal_wing, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(id) DO NOTHING",
    )
    .bind(id)
    .bind(name)
    .bind(repo_path)
    .bind(mempal_wing)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Number of projects.
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn count_projects(pool: &SqlitePool) -> Result<usize, StoreError> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM projects")
        .fetch_one(pool)
        .await?;
    Ok(usize::try_from(n).unwrap_or(0))
}
