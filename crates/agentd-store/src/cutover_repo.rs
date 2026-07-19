//! Durable per-project agent-chat to agentd cutover state.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::StoreError;
use crate::util::now_unix;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CutoverPhase {
    Observe,
    Shadow,
    Canary,
    Cutover,
    Drain,
    Retired,
    Rollback,
}

impl CutoverPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Shadow => "shadow",
            Self::Canary => "canary",
            Self::Cutover => "cutover",
            Self::Drain => "drain",
            Self::Retired => "retired",
            Self::Rollback => "rollback",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverProjectState {
    pub project_id: String,
    pub phase: CutoverPhase,
    pub authority_revision: String,
    pub matrix_cursor: i64,
    pub lease_epoch: i64,
    pub updated_at: i64,
}

pub async fn get(
    pool: &SqlitePool,
    project_id: &str,
) -> Result<Option<CutoverProjectState>, StoreError> {
    let row = sqlx::query_as::<_, (String, String, String, i64, i64, i64)>(
        "SELECT project_id, phase, authority_revision, matrix_cursor, lease_epoch, updated_at
         FROM cutover_projects WHERE project_id = ?",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?;
    row.map(row_to_state).transpose()
}

pub async fn transition(
    pool: &SqlitePool,
    project_id: &str,
    phase: CutoverPhase,
    authority_revision: &str,
    matrix_cursor: i64,
    lease_epoch: i64,
) -> Result<CutoverProjectState, StoreError> {
    if project_id.trim().is_empty() || authority_revision.trim().is_empty() {
        return Err(StoreError::Invariant("cutover identity is required".into()));
    }
    if matrix_cursor < 0 || lease_epoch <= 0 {
        return Err(StoreError::Invariant(
            "cutover cursor and lease epoch must be positive".into(),
        ));
    }
    if let Some(current) = get(pool, project_id).await? {
        if phase == CutoverPhase::Rollback && lease_epoch <= current.lease_epoch {
            return Err(StoreError::Invariant(
                "rollback requires a new lease epoch".into(),
            ));
        }
        let allowed = matches!(
            (current.phase, phase),
            (CutoverPhase::Observe, CutoverPhase::Shadow)
                | (CutoverPhase::Shadow, CutoverPhase::Canary)
                | (CutoverPhase::Canary, CutoverPhase::Cutover)
                | (CutoverPhase::Cutover, CutoverPhase::Drain)
                | (CutoverPhase::Drain, CutoverPhase::Retired)
                | (CutoverPhase::Rollback, _)
                | (_, CutoverPhase::Rollback)
        );
        if !allowed {
            return Err(StoreError::Invariant(format!(
                "invalid cutover transition {} -> {}",
                current.phase.as_str(),
                phase.as_str()
            )));
        }
    }
    let updated_at = now_unix();
    sqlx::query(
        "INSERT INTO cutover_project_history
         (project_id, phase, authority_revision, matrix_cursor, lease_epoch, recorded_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(project_id)
    .bind(phase.as_str())
    .bind(authority_revision)
    .bind(matrix_cursor)
    .bind(lease_epoch)
    .bind(updated_at)
    .execute(pool)
    .await?;
    sqlx::query(
        "INSERT INTO cutover_projects
         (project_id, phase, authority_revision, matrix_cursor, lease_epoch, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(project_id) DO UPDATE SET phase=excluded.phase,
         authority_revision=excluded.authority_revision, matrix_cursor=excluded.matrix_cursor,
         lease_epoch=excluded.lease_epoch, updated_at=excluded.updated_at",
    )
    .bind(project_id)
    .bind(phase.as_str())
    .bind(authority_revision)
    .bind(matrix_cursor)
    .bind(lease_epoch)
    .bind(updated_at)
    .execute(pool)
    .await?;
    Ok(CutoverProjectState {
        project_id: project_id.to_owned(),
        phase,
        authority_revision: authority_revision.to_owned(),
        matrix_cursor,
        lease_epoch,
        updated_at,
    })
}

/// Restore the last non-rollback project state while requiring a new lease
/// epoch. The history row is retained and the restored state is itself
/// recorded as a new transition.
pub async fn rollback(
    pool: &SqlitePool,
    project_id: &str,
    lease_epoch: i64,
) -> Result<CutoverProjectState, StoreError> {
    let row = sqlx::query_as::<_, (String, String, i64, i64)>(
        "SELECT authority_revision, phase, matrix_cursor, lease_epoch
         FROM cutover_project_history
         WHERE project_id = ? AND phase != 'rollback'
         ORDER BY id DESC LIMIT 1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| StoreError::NotFound)?;
    if lease_epoch <= row.3 {
        return Err(StoreError::Invariant(
            "rollback requires a new lease epoch".into(),
        ));
    }
    let phase = match row.1.as_str() {
        "observe" => CutoverPhase::Observe,
        "shadow" => CutoverPhase::Shadow,
        "canary" => CutoverPhase::Canary,
        "cutover" => CutoverPhase::Cutover,
        "drain" => CutoverPhase::Drain,
        "retired" => CutoverPhase::Retired,
        value => {
            return Err(StoreError::Invariant(format!(
                "unknown rollback phase {value}"
            )));
        }
    };
    transition(
        pool,
        project_id,
        CutoverPhase::Rollback,
        &row.0,
        row.2,
        lease_epoch,
    )
    .await?;
    transition(pool, project_id, phase, &row.0, row.2, lease_epoch).await
}

fn row_to_state(
    row: (String, String, String, i64, i64, i64),
) -> Result<CutoverProjectState, StoreError> {
    let phase = match row.1.as_str() {
        "observe" => CutoverPhase::Observe,
        "shadow" => CutoverPhase::Shadow,
        "canary" => CutoverPhase::Canary,
        "cutover" => CutoverPhase::Cutover,
        "drain" => CutoverPhase::Drain,
        "retired" => CutoverPhase::Retired,
        "rollback" => CutoverPhase::Rollback,
        value => {
            return Err(StoreError::Invariant(format!(
                "unknown cutover phase {value}"
            )));
        }
    };
    Ok(CutoverProjectState {
        project_id: row.0,
        phase,
        authority_revision: row.2,
        matrix_cursor: row.3,
        lease_epoch: row.4,
        updated_at: row.5,
    })
}
