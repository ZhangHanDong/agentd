//! Content-addressed artifact pointers. Inherent `SqliteStore` methods (not part
//! of the engine-facing `Store` trait) — keyed by `sha256`, idempotent on insert.

use std::path::PathBuf;

use agentd_core::types::{Artifact, ArtifactKind, NodeId, RunId};
use sqlx::{Row, SqlitePool};

use crate::error::StoreError;
use crate::util::now_unix;

fn kind_str(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Spec => "spec",
        ArtifactKind::Plan => "plan",
        ArtifactKind::Diff => "diff",
        ArtifactKind::Transcript => "transcript",
        ArtifactKind::Verdict => "verdict",
        ArtifactKind::ContextPack => "context-pack",
    }
}

fn parse_kind(s: &str) -> Result<ArtifactKind, StoreError> {
    Ok(match s {
        "spec" => ArtifactKind::Spec,
        "plan" => ArtifactKind::Plan,
        "diff" => ArtifactKind::Diff,
        "transcript" => ArtifactKind::Transcript,
        "verdict" => ArtifactKind::Verdict,
        "context-pack" => ArtifactKind::ContextPack,
        other => {
            return Err(StoreError::Invariant(format!(
                "unknown artifact kind '{other}'"
            )));
        }
    })
}

/// Record an artifact pointer (idempotent on its content hash).
///
/// # Errors
/// Returns [`StoreError::Sqlx`] on a database failure.
pub async fn insert_artifact(
    pool: &SqlitePool,
    artifact: &Artifact,
    run_id: Option<&RunId>,
    node_id: Option<&NodeId>,
) -> Result<(), StoreError> {
    sqlx::query(
        "INSERT INTO artifacts (sha256, kind, path, bytes, created_at, run_id, node_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(sha256) DO NOTHING",
    )
    .bind(&artifact.sha256)
    .bind(kind_str(artifact.kind))
    .bind(artifact.path.to_string_lossy().into_owned())
    .bind(i64::try_from(artifact.bytes).unwrap_or(i64::MAX))
    .bind(now_unix())
    .bind(run_id.map(RunId::as_str))
    .bind(node_id.map(NodeId::as_str))
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch an artifact pointer by its content hash.
///
/// # Errors
/// Returns [`StoreError`] on a decode or database failure.
pub async fn get_artifact(pool: &SqlitePool, sha256: &str) -> Result<Option<Artifact>, StoreError> {
    let row = sqlx::query("SELECT kind, path, bytes FROM artifacts WHERE sha256 = ?")
        .bind(sha256)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(Artifact {
        kind: parse_kind(&row.get::<String, _>("kind"))?,
        path: PathBuf::from(row.get::<String, _>("path")),
        sha256: sha256.to_string(),
        bytes: u64::try_from(row.get::<i64, _>("bytes")).unwrap_or(0),
    }))
}
