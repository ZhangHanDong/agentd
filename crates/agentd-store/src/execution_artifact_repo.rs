//! Immutable enterprise execution-artifact metadata and external references.

use agentd_core::types::{
    ExecutionArtifactId, RunId, RuntimeAttemptId, RuntimeSessionId, TaskRunId, WorkerIncarnationId,
};
use serde_json::Value;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::error::StoreError;
use crate::runtime_session_repo::ExecutionSnapshotRef;
use crate::util::now_unix;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionArtifactKind {
    Requirements,
    Spec,
    Plan,
    Review,
    RuntimeSummary,
    Transcript,
    Log,
    Patch,
    Commit,
    TestReport,
}

impl ExecutionArtifactKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requirements => "requirements",
            Self::Spec => "spec",
            Self::Plan => "plan",
            Self::Review => "review",
            Self::RuntimeSummary => "runtime_summary",
            Self::Transcript => "transcript",
            Self::Log => "log",
            Self::Patch => "patch",
            Self::Commit => "commit",
            Self::TestReport => "test_report",
        }
    }
}

impl TryFrom<&str> for ExecutionArtifactKind {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "requirements" => Ok(Self::Requirements),
            "spec" => Ok(Self::Spec),
            "plan" => Ok(Self::Plan),
            "review" => Ok(Self::Review),
            "runtime_summary" => Ok(Self::RuntimeSummary),
            "transcript" => Ok(Self::Transcript),
            "log" => Ok(Self::Log),
            "patch" => Ok(Self::Patch),
            "commit" => Ok(Self::Commit),
            "test_report" => Ok(Self::TestReport),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionArtifactCreate {
    pub id: ExecutionArtifactId,
    pub kind: ExecutionArtifactKind,
    pub content_sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub storage_ref: String,
    pub provenance: Value,
    pub execution_run_id: RunId,
    pub execution_task_id: Option<TaskRunId>,
    pub runtime_session_id: Option<RuntimeSessionId>,
    pub runtime_attempt_id: Option<RuntimeAttemptId>,
    pub snapshot: ExecutionSnapshotRef,
    pub target_repository_id: String,
    pub target_base_commit: String,
    pub producer_worker_incarnation_id: Option<WorkerIncarnationId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionArtifactRecord {
    pub id: ExecutionArtifactId,
    pub kind: ExecutionArtifactKind,
    pub content_sha256: String,
    pub size_bytes: u64,
    pub media_type: String,
    pub storage_ref: String,
    pub provenance: Value,
    pub execution_run_id: RunId,
    pub execution_task_id: Option<TaskRunId>,
    pub runtime_session_id: Option<RuntimeSessionId>,
    pub runtime_attempt_id: Option<RuntimeAttemptId>,
    pub snapshot: ExecutionSnapshotRef,
    pub target_repository_id: String,
    pub target_base_commit: String,
    pub producer_worker_incarnation_id: Option<WorkerIncarnationId>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyArtifactMappingRecord {
    pub legacy_sha256: String,
    pub execution_artifact_id: ExecutionArtifactId,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CertificationRefKind {
    Request,
    Result,
    Signature,
    Attestation,
}

impl CertificationRefKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Request => "request",
            Self::Result => "result",
            Self::Signature => "signature",
            Self::Attestation => "attestation",
        }
    }
}

impl TryFrom<&str> for CertificationRefKind {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "request" => Ok(Self::Request),
            "result" => Ok(Self::Result),
            "signature" => Ok(Self::Signature),
            "attestation" => Ok(Self::Attestation),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificationRefRecord {
    pub id: i64,
    pub execution_artifact_id: ExecutionArtifactId,
    pub authority_key: String,
    pub kind: CertificationRefKind,
    pub external_ref: String,
    pub recorded_at: i64,
}

/// Insert immutable artifact metadata after validating its complete parent graph.
///
/// # Errors
/// Returns [`StoreError::Invariant`] for malformed metadata,
/// [`StoreError::NotFound`] for unknown parents, and [`StoreError::Conflict`]
/// for relationships that do not form one execution graph.
pub async fn create_artifact(
    pool: &SqlitePool,
    request: ExecutionArtifactCreate,
) -> Result<ExecutionArtifactRecord, StoreError> {
    validate_artifact(&request)?;
    let provenance_json = serde_json::to_string(&request.provenance)?;
    let size_bytes = i64::try_from(request.size_bytes)
        .map_err(|_| StoreError::Invariant("artifact size exceeds SQLite i64".to_string()))?;
    let mut tx = pool.begin().await?;
    validate_parent_graph(&mut tx, &request).await?;
    let now = now_unix();
    sqlx::query(
        "INSERT INTO execution_artifacts \
         (id, kind, content_sha256, size_bytes, media_type, storage_ref, provenance_json, \
          execution_run_id, execution_task_id, runtime_session_id, runtime_attempt_id, \
          snapshot_authority_key, snapshot_resource_kind, snapshot_resource_id, \
          snapshot_resource_version, snapshot_content_sha256, target_repository_id, \
          target_base_commit, producer_worker_incarnation_id, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(request.id.as_str())
    .bind(request.kind.as_str())
    .bind(&request.content_sha256)
    .bind(size_bytes)
    .bind(&request.media_type)
    .bind(&request.storage_ref)
    .bind(provenance_json)
    .bind(request.execution_run_id.as_str())
    .bind(request.execution_task_id.as_ref().map(TaskRunId::as_str))
    .bind(
        request
            .runtime_session_id
            .as_ref()
            .map(RuntimeSessionId::as_str),
    )
    .bind(
        request
            .runtime_attempt_id
            .as_ref()
            .map(RuntimeAttemptId::as_str),
    )
    .bind(&request.snapshot.authority_key)
    .bind(&request.snapshot.resource_kind)
    .bind(&request.snapshot.resource_id)
    .bind(&request.snapshot.resource_version)
    .bind(&request.snapshot.content_sha256)
    .bind(&request.target_repository_id)
    .bind(&request.target_base_commit)
    .bind(
        request
            .producer_worker_incarnation_id
            .as_ref()
            .map(WorkerIncarnationId::as_str),
    )
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    get_artifact(pool, &request.id)
        .await?
        .ok_or(StoreError::NotFound)
}

/// Read one enterprise execution artifact.
///
/// # Errors
/// Returns [`StoreError`] when persisted metadata cannot be decoded.
pub async fn get_artifact(
    pool: &SqlitePool,
    id: &ExecutionArtifactId,
) -> Result<Option<ExecutionArtifactRecord>, StoreError> {
    let row = sqlx::query(
        "SELECT id, kind, content_sha256, size_bytes, media_type, storage_ref, provenance_json, \
         execution_run_id, execution_task_id, runtime_session_id, runtime_attempt_id, \
         snapshot_authority_key, snapshot_resource_kind, snapshot_resource_id, \
         snapshot_resource_version, snapshot_content_sha256, target_repository_id, \
         target_base_commit, producer_worker_incarnation_id, created_at \
         FROM execution_artifacts WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(pool)
    .await?;
    row.as_ref().map(row_to_artifact).transpose()
}

/// List immutable artifact metadata for one run after a stable tuple cursor.
///
/// # Errors
/// Returns [`StoreError`] when rows cannot be queried or decoded.
pub async fn list_artifacts_by_run(
    pool: &SqlitePool,
    run_id: &RunId,
    cursor: Option<(i64, &ExecutionArtifactId)>,
    limit: u16,
) -> Result<Vec<ExecutionArtifactRecord>, StoreError> {
    let rows = if let Some((created_at, id)) = cursor {
        sqlx::query(
            "SELECT id, kind, content_sha256, size_bytes, media_type, storage_ref, \
                    provenance_json, execution_run_id, execution_task_id, runtime_session_id, \
                    runtime_attempt_id, snapshot_authority_key, snapshot_resource_kind, \
                    snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256, \
                    target_repository_id, target_base_commit, producer_worker_incarnation_id, \
                    created_at \
             FROM execution_artifacts \
             WHERE execution_run_id = ? \
               AND (created_at > ? OR (created_at = ? AND id > ?)) \
             ORDER BY created_at, id LIMIT ?",
        )
        .bind(run_id.as_str())
        .bind(created_at)
        .bind(created_at)
        .bind(id.as_str())
        .bind(i64::from(limit))
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query(
            "SELECT id, kind, content_sha256, size_bytes, media_type, storage_ref, \
                    provenance_json, execution_run_id, execution_task_id, runtime_session_id, \
                    runtime_attempt_id, snapshot_authority_key, snapshot_resource_kind, \
                    snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256, \
                    target_repository_id, target_base_commit, producer_worker_incarnation_id, \
                    created_at \
             FROM execution_artifacts WHERE execution_run_id = ? \
             ORDER BY created_at, id LIMIT ?",
        )
        .bind(run_id.as_str())
        .bind(i64::from(limit))
        .fetch_all(pool)
        .await?
    };
    rows.iter().map(row_to_artifact).collect()
}

/// Map one legacy content-addressed artifact to one enterprise artifact.
///
/// # Errors
/// Returns [`StoreError::NotFound`] for unknown records and
/// [`StoreError::Conflict`] for hash/size mismatch or remapping.
pub async fn map_legacy_artifact(
    pool: &SqlitePool,
    legacy_sha256: &str,
    artifact_id: &ExecutionArtifactId,
) -> Result<LegacyArtifactMappingRecord, StoreError> {
    validate_sha256(legacy_sha256, "legacy artifact sha256")?;
    validate_id(artifact_id.as_str(), "ar_", "ExecutionArtifactId")?;
    let mut tx = pool.begin().await?;
    let legacy: Option<(i64,)> = sqlx::query_as("SELECT bytes FROM artifacts WHERE sha256 = ?")
        .bind(legacy_sha256)
        .fetch_optional(&mut *tx)
        .await?;
    let legacy_bytes = legacy.ok_or(StoreError::NotFound)?.0;
    let enterprise: Option<(String, i64)> =
        sqlx::query_as("SELECT content_sha256, size_bytes FROM execution_artifacts WHERE id = ?")
            .bind(artifact_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    let (content_sha256, size_bytes) = enterprise.ok_or(StoreError::NotFound)?;
    if content_sha256 != legacy_sha256 || size_bytes != legacy_bytes {
        return Err(StoreError::Conflict(
            "legacy and enterprise artifact content metadata differ".to_string(),
        ));
    }
    if let Some(row) = sqlx::query(
        "SELECT execution_artifact_id, created_at FROM legacy_artifact_mappings \
         WHERE legacy_sha256 = ?",
    )
    .bind(legacy_sha256)
    .fetch_optional(&mut *tx)
    .await?
    {
        let existing_id: String = row.get("execution_artifact_id");
        if existing_id != artifact_id.as_str() {
            return Err(StoreError::Conflict(
                "legacy artifact is already mapped".to_string(),
            ));
        }
        return Ok(LegacyArtifactMappingRecord {
            legacy_sha256: legacy_sha256.to_string(),
            execution_artifact_id: artifact_id.clone(),
            created_at: row.get("created_at"),
        });
    }
    let mapped_legacy: Option<String> = sqlx::query_scalar(
        "SELECT legacy_sha256 FROM legacy_artifact_mappings WHERE execution_artifact_id = ?",
    )
    .bind(artifact_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    if mapped_legacy.is_some() {
        return Err(StoreError::Conflict(
            "enterprise artifact is already mapped".to_string(),
        ));
    }
    let now = now_unix();
    sqlx::query(
        "INSERT INTO legacy_artifact_mappings \
         (legacy_sha256, execution_artifact_id, created_at) VALUES (?, ?, ?)",
    )
    .bind(legacy_sha256)
    .bind(artifact_id.as_str())
    .bind(now)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(LegacyArtifactMappingRecord {
        legacy_sha256: legacy_sha256.to_string(),
        execution_artifact_id: artifact_id.clone(),
        created_at: now,
    })
}

/// Append one immutable OpenFab-owned certification reference.
///
/// # Errors
/// Returns [`StoreError::NotFound`] for an unknown artifact and
/// [`StoreError::Conflict`] if a kind or external ref is reused differently.
pub async fn append_certification_ref(
    pool: &SqlitePool,
    artifact_id: &ExecutionArtifactId,
    authority_key: &str,
    kind: CertificationRefKind,
    external_ref: &str,
) -> Result<CertificationRefRecord, StoreError> {
    validate_id(artifact_id.as_str(), "ar_", "ExecutionArtifactId")?;
    validate_text(authority_key, "certification authority key")?;
    validate_text(external_ref, "certification external ref")?;
    let mut tx = pool.begin().await?;
    let artifact_exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM execution_artifacts WHERE id = ?)")
            .bind(artifact_id.as_str())
            .fetch_one(&mut *tx)
            .await?;
    if !artifact_exists {
        return Err(StoreError::NotFound);
    }
    if let Some(row) = sqlx::query(
        "SELECT id, external_ref, recorded_at FROM artifact_certification_refs \
         WHERE execution_artifact_id = ? AND authority_key = ? AND ref_kind = ?",
    )
    .bind(artifact_id.as_str())
    .bind(authority_key)
    .bind(kind.as_str())
    .fetch_optional(&mut *tx)
    .await?
    {
        let existing_ref: String = row.get("external_ref");
        if existing_ref != external_ref {
            return Err(StoreError::Conflict(format!(
                "{} certification reference already recorded",
                kind.as_str()
            )));
        }
        return Ok(CertificationRefRecord {
            id: row.get("id"),
            execution_artifact_id: artifact_id.clone(),
            authority_key: authority_key.to_string(),
            kind,
            external_ref: existing_ref,
            recorded_at: row.get("recorded_at"),
        });
    }
    let reused_by: Option<String> = sqlx::query_scalar(
        "SELECT execution_artifact_id FROM artifact_certification_refs \
         WHERE authority_key = ? AND ref_kind = ? AND external_ref = ?",
    )
    .bind(authority_key)
    .bind(kind.as_str())
    .bind(external_ref)
    .fetch_optional(&mut *tx)
    .await?;
    if reused_by.is_some() {
        return Err(StoreError::Conflict(
            "certification external reference is already linked".to_string(),
        ));
    }
    let now = now_unix();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO artifact_certification_refs \
         (execution_artifact_id, authority_key, ref_kind, external_ref, recorded_at) \
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(artifact_id.as_str())
    .bind(authority_key)
    .bind(kind.as_str())
    .bind(external_ref)
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(CertificationRefRecord {
        id,
        execution_artifact_id: artifact_id.clone(),
        authority_key: authority_key.to_string(),
        kind,
        external_ref: external_ref.to_string(),
        recorded_at: now,
    })
}

/// List immutable certification references for one artifact in database order.
///
/// # Errors
/// Returns [`StoreError`] when rows cannot be queried or decoded.
pub async fn list_certification_refs(
    pool: &SqlitePool,
    artifact_id: &ExecutionArtifactId,
) -> Result<Vec<CertificationRefRecord>, StoreError> {
    validate_id(artifact_id.as_str(), "ar_", "ExecutionArtifactId")?;
    let rows = sqlx::query(
        "SELECT id, execution_artifact_id, authority_key, ref_kind, external_ref, recorded_at \
         FROM artifact_certification_refs WHERE execution_artifact_id = ? ORDER BY id",
    )
    .bind(artifact_id.as_str())
    .fetch_all(pool)
    .await?;
    rows.iter()
        .map(|row| {
            let kind_text: String = row.get("ref_kind");
            let kind = CertificationRefKind::try_from(kind_text.as_str()).map_err(|()| {
                StoreError::Invariant(format!("unknown certification reference kind: {kind_text}"))
            })?;
            Ok(CertificationRefRecord {
                id: row.get("id"),
                execution_artifact_id: ExecutionArtifactId::from_string(
                    row.get::<String, _>("execution_artifact_id"),
                ),
                authority_key: row.get("authority_key"),
                kind,
                external_ref: row.get("external_ref"),
                recorded_at: row.get("recorded_at"),
            })
        })
        .collect()
}

async fn validate_parent_graph(
    tx: &mut Transaction<'_, Sqlite>,
    request: &ExecutionArtifactCreate,
) -> Result<(), StoreError> {
    let run_exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?)")
        .bind(request.execution_run_id.as_str())
        .fetch_one(&mut **tx)
        .await?;
    if !run_exists {
        return Err(StoreError::NotFound);
    }
    if let Some(task_id) = &request.execution_task_id {
        let run_id: Option<String> =
            sqlx::query_scalar("SELECT run_id FROM task_runs WHERE id = ?")
                .bind(task_id.as_str())
                .fetch_optional(&mut **tx)
                .await?;
        let run_id = run_id.ok_or(StoreError::NotFound)?;
        if run_id != request.execution_run_id.as_str() {
            return Err(StoreError::Conflict(
                "execution task does not belong to artifact run".to_string(),
            ));
        }
    }
    if let Some(session_id) = &request.runtime_session_id {
        let row = sqlx::query(
            "SELECT execution_task_id, snapshot_authority_key, snapshot_resource_kind, \
             snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256 \
             FROM runtime_sessions WHERE id = ?",
        )
        .bind(session_id.as_str())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(StoreError::NotFound)?;
        if Some(row.get::<String, _>("execution_task_id").as_str())
            != request.execution_task_id.as_ref().map(TaskRunId::as_str)
            || row.get::<String, _>("snapshot_authority_key") != request.snapshot.authority_key
            || row.get::<String, _>("snapshot_resource_kind") != request.snapshot.resource_kind
            || row.get::<String, _>("snapshot_resource_id") != request.snapshot.resource_id
            || row.get::<String, _>("snapshot_resource_version")
                != request.snapshot.resource_version
            || row.get::<String, _>("snapshot_content_sha256") != request.snapshot.content_sha256
        {
            return Err(StoreError::Conflict(
                "runtime session does not match artifact task or snapshot".to_string(),
            ));
        }
    }
    let mut attempt_worker = None;
    if let Some(attempt_id) = &request.runtime_attempt_id {
        let row = sqlx::query(
            "SELECT runtime_session_id, worker_incarnation_id FROM runtime_attempts WHERE id = ?",
        )
        .bind(attempt_id.as_str())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(StoreError::NotFound)?;
        if Some(row.get::<String, _>("runtime_session_id").as_str())
            != request
                .runtime_session_id
                .as_ref()
                .map(RuntimeSessionId::as_str)
        {
            return Err(StoreError::Conflict(
                "runtime attempt does not belong to artifact session".to_string(),
            ));
        }
        attempt_worker = Some(row.get::<String, _>("worker_incarnation_id"));
    }
    if let Some(producer_id) = &request.producer_worker_incarnation_id {
        let worker_exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM worker_incarnations WHERE id = ?)")
                .bind(producer_id.as_str())
                .fetch_one(&mut **tx)
                .await?;
        if !worker_exists {
            return Err(StoreError::NotFound);
        }
        if let Some(attempt_worker) = attempt_worker
            && attempt_worker != producer_id.as_str()
        {
            return Err(StoreError::Conflict(
                "artifact producer does not match runtime attempt worker".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_artifact(request: &ExecutionArtifactCreate) -> Result<(), StoreError> {
    validate_id(request.id.as_str(), "ar_", "ExecutionArtifactId")?;
    validate_sha256(&request.content_sha256, "artifact content sha256")?;
    validate_text(&request.media_type, "artifact media type")?;
    validate_text(&request.storage_ref, "artifact storage ref")?;
    if !request.provenance.is_object() {
        return Err(StoreError::Invariant(
            "artifact provenance must be a JSON object".to_string(),
        ));
    }
    validate_snapshot(&request.snapshot)?;
    validate_text(&request.target_repository_id, "target repository id")?;
    validate_text(&request.target_base_commit, "target base commit")?;
    if request.runtime_session_id.is_some() && request.execution_task_id.is_none() {
        return Err(StoreError::Invariant(
            "runtime session requires execution task".to_string(),
        ));
    }
    if request.runtime_attempt_id.is_some() && request.runtime_session_id.is_none() {
        return Err(StoreError::Invariant(
            "runtime attempt requires runtime session".to_string(),
        ));
    }
    Ok(())
}

fn validate_snapshot(snapshot: &ExecutionSnapshotRef) -> Result<(), StoreError> {
    validate_text(&snapshot.authority_key, "snapshot authority key")?;
    if snapshot.resource_kind != "execution_snapshot" {
        return Err(StoreError::Invariant(
            "snapshot resource kind must be execution_snapshot".to_string(),
        ));
    }
    validate_text(&snapshot.resource_id, "snapshot resource id")?;
    validate_text(&snapshot.resource_version, "snapshot resource version")?;
    validate_sha256(&snapshot.content_sha256, "snapshot content sha256")
}

fn validate_id(value: &str, prefix: &str, name: &str) -> Result<(), StoreError> {
    let payload = value.strip_prefix(prefix).ok_or_else(|| {
        StoreError::Invariant(format!("{name} must use canonical {prefix} prefix"))
    })?;
    if payload.len() != 26
        || !payload.bytes().all(|byte| {
            matches!(byte, b'0'..=b'9' | b'A'..=b'Z') && !matches!(byte, b'I' | b'L' | b'O' | b'U')
        })
        || ulid::Ulid::from_string(payload).is_err()
    {
        return Err(StoreError::Invariant(format!(
            "{name} must contain a canonical uppercase ULID"
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str, field: &str) -> Result<(), StoreError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(StoreError::Invariant(format!(
            "{field} must be 64 lowercase hex characters"
        )));
    }
    Ok(())
}

fn validate_text(value: &str, field: &str) -> Result<(), StoreError> {
    if value.trim().is_empty() {
        return Err(StoreError::Invariant(format!("{field} must not be empty")));
    }
    Ok(())
}

fn row_to_artifact(row: &sqlx::sqlite::SqliteRow) -> Result<ExecutionArtifactRecord, StoreError> {
    let kind_text: String = row.get("kind");
    let kind = ExecutionArtifactKind::try_from(kind_text.as_str()).map_err(|()| {
        StoreError::Invariant(format!("unknown execution artifact kind: {kind_text}"))
    })?;
    let size: i64 = row.get("size_bytes");
    let size_bytes = u64::try_from(size)
        .map_err(|_| StoreError::Invariant("negative artifact size".to_string()))?;
    let provenance_json: String = row.get("provenance_json");
    Ok(ExecutionArtifactRecord {
        id: ExecutionArtifactId::from_string(row.get::<String, _>("id")),
        kind,
        content_sha256: row.get("content_sha256"),
        size_bytes,
        media_type: row.get("media_type"),
        storage_ref: row.get("storage_ref"),
        provenance: serde_json::from_str(&provenance_json)?,
        execution_run_id: RunId::from_string(row.get::<String, _>("execution_run_id")),
        execution_task_id: row
            .get::<Option<String>, _>("execution_task_id")
            .map(TaskRunId::from_string),
        runtime_session_id: row
            .get::<Option<String>, _>("runtime_session_id")
            .map(RuntimeSessionId::from_string),
        runtime_attempt_id: row
            .get::<Option<String>, _>("runtime_attempt_id")
            .map(RuntimeAttemptId::from_string),
        snapshot: ExecutionSnapshotRef {
            authority_key: row.get("snapshot_authority_key"),
            resource_kind: row.get("snapshot_resource_kind"),
            resource_id: row.get("snapshot_resource_id"),
            resource_version: row.get("snapshot_resource_version"),
            content_sha256: row.get("snapshot_content_sha256"),
        },
        target_repository_id: row.get("target_repository_id"),
        target_base_commit: row.get("target_base_commit"),
        producer_worker_incarnation_id: row
            .get::<Option<String>, _>("producer_worker_incarnation_id")
            .map(WorkerIncarnationId::from_string),
        created_at: row.get("created_at"),
    })
}
