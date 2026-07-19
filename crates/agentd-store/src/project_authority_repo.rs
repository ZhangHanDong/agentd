//! Durable immutable projection of resolved project-authority snapshots.

use agentd_core::types::ProjectExecutionSnapshot;
use sqlx::SqlitePool;

use crate::error::StoreError;
use crate::util::now_unix;

pub async fn record_snapshot(
    pool: &SqlitePool,
    snapshot: &ProjectExecutionSnapshot,
) -> Result<(), StoreError> {
    snapshot
        .validate()
        .map_err(|error| StoreError::Invariant(error.to_string()))?;
    let snapshot_json = serde_json::to_string(snapshot)?;
    let snapshot_ref = resource_ref_string(snapshot.snapshot_ref.as_resource_ref());
    let project_ref = resource_ref_string(snapshot.project_ref.as_resource_ref());
    if let Some(existing) = sqlx::query_scalar::<_, String>(
        "SELECT snapshot_json FROM project_authority_snapshots WHERE snapshot_ref = ?",
    )
    .bind(&snapshot_ref)
    .fetch_optional(pool)
    .await?
    {
        if existing != snapshot_json {
            return Err(StoreError::Conflict(format!(
                "project authority snapshot '{snapshot_ref}' is immutable"
            )));
        }
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO project_authority_snapshots \
         (snapshot_ref, authority_key, project_ref, authority_revision, issued_at, valid_until, \
          content_sha256, snapshot_json, recorded_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(snapshot_ref) DO NOTHING",
    )
    .bind(snapshot_ref)
    .bind(snapshot.authority_key.as_str())
    .bind(project_ref)
    .bind(i64::try_from(snapshot.authority_revision).map_err(|_| {
        StoreError::Invariant("authority revision exceeds SQLite range".to_string())
    })?)
    .bind(snapshot.issued_at)
    .bind(snapshot.valid_until)
    .bind(&snapshot.content_sha256)
    .bind(snapshot_json)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_snapshot(
    pool: &SqlitePool,
    snapshot_ref: &str,
) -> Result<ProjectExecutionSnapshot, StoreError> {
    let snapshot_json = sqlx::query_scalar::<_, String>(
        "SELECT snapshot_json FROM project_authority_snapshots WHERE snapshot_ref = ?",
    )
    .bind(snapshot_ref)
    .fetch_optional(pool)
    .await?
    .ok_or(StoreError::NotFound)?;
    Ok(serde_json::from_str(&snapshot_json)?)
}

pub async fn current_snapshot_for_project(
    pool: &SqlitePool,
    authority_key: &str,
    project_ref: &str,
) -> Result<ProjectExecutionSnapshot, StoreError> {
    let snapshot_json = sqlx::query_scalar::<_, String>(
        "SELECT snapshot_json FROM project_authority_snapshots \
         WHERE authority_key = ? AND project_ref = ? \
         ORDER BY authority_revision DESC, recorded_at DESC LIMIT 1",
    )
    .bind(authority_key)
    .bind(project_ref)
    .fetch_optional(pool)
    .await?
    .ok_or(StoreError::NotFound)?;
    Ok(serde_json::from_str(&snapshot_json)?)
}

pub async fn count_snapshots(pool: &SqlitePool) -> Result<i64, StoreError> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM project_authority_snapshots")
            .fetch_one(pool)
            .await?,
    )
}

pub async fn count_expired(pool: &SqlitePool, observed_at: i64) -> Result<i64, StoreError> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM project_authority_snapshots WHERE valid_until <= ?",
    )
    .bind(observed_at)
    .fetch_one(pool)
    .await?)
}

fn resource_ref_string(reference: &agentd_core::types::AuthorityResourceRef) -> String {
    format!(
        "{}:{}:{}:{}",
        reference.authority_key(),
        reference.resource_kind().as_str(),
        reference.resource_id(),
        reference.resource_version()
    )
}
