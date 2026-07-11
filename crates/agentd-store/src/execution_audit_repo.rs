//! Ordered, idempotent, append-only enterprise execution audit events.

use agentd_core::types::{
    AuditEventId, ExecutionArtifactId, RunId, RuntimeAttemptId, RuntimeSessionId, TaskRunId,
    WorkerIncarnationId,
};
use serde_json::Value;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::error::StoreError;
use crate::runtime_session_repo::ExecutionSnapshotRef;
use crate::util::now_unix;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditActorKind {
    ControlPlane,
    Worker,
    AgentProfile,
    Operator,
    ProjectAuthority,
    CertificationAuthority,
    System,
    Import,
}

impl AuditActorKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ControlPlane => "control_plane",
            Self::Worker => "worker",
            Self::AgentProfile => "agent_profile",
            Self::Operator => "operator",
            Self::ProjectAuthority => "project_authority",
            Self::CertificationAuthority => "certification_authority",
            Self::System => "system",
            Self::Import => "import",
        }
    }
}

impl TryFrom<&str> for AuditActorKind {
    type Error = ();

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "control_plane" => Ok(Self::ControlPlane),
            "worker" => Ok(Self::Worker),
            "agent_profile" => Ok(Self::AgentProfile),
            "operator" => Ok(Self::Operator),
            "project_authority" => Ok(Self::ProjectAuthority),
            "certification_authority" => Ok(Self::CertificationAuthority),
            "system" => Ok(Self::System),
            "import" => Ok(Self::Import),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEventCreate {
    pub id: AuditEventId,
    pub idempotency_scope: String,
    pub idempotency_key: String,
    pub event_type: String,
    pub actor_kind: AuditActorKind,
    pub actor_ref: String,
    pub payload_sha256: String,
    pub payload: Value,
    pub execution_run_id: RunId,
    pub execution_task_id: Option<TaskRunId>,
    pub runtime_session_id: Option<RuntimeSessionId>,
    pub runtime_attempt_id: Option<RuntimeAttemptId>,
    pub execution_artifact_id: Option<ExecutionArtifactId>,
    pub worker_incarnation_id: Option<WorkerIncarnationId>,
    pub snapshot: ExecutionSnapshotRef,
    pub target_repository_id: String,
    pub target_base_commit: String,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEventRecord {
    pub sequence: i64,
    pub id: AuditEventId,
    pub idempotency_scope: String,
    pub idempotency_key: String,
    pub event_type: String,
    pub actor_kind: AuditActorKind,
    pub actor_ref: String,
    pub payload_sha256: String,
    pub payload: Value,
    pub execution_run_id: RunId,
    pub execution_task_id: Option<TaskRunId>,
    pub runtime_session_id: Option<RuntimeSessionId>,
    pub runtime_attempt_id: Option<RuntimeAttemptId>,
    pub execution_artifact_id: Option<ExecutionArtifactId>,
    pub worker_incarnation_id: Option<WorkerIncarnationId>,
    pub snapshot: ExecutionSnapshotRef,
    pub target_repository_id: String,
    pub target_base_commit: String,
    pub occurred_at: i64,
    pub recorded_at: i64,
}

/// Append one immutable audit event or return the original exact retry.
///
/// # Errors
/// Returns [`StoreError::Invariant`] for malformed envelopes,
/// [`StoreError::NotFound`] for unknown parents, and [`StoreError::Conflict`]
/// for changed retries, reused ids, or mismatched execution links.
pub async fn append_event(
    pool: &SqlitePool,
    request: AuditEventCreate,
) -> Result<AuditEventRecord, StoreError> {
    validate_event(&request)?;
    let payload_json = serde_json::to_string(&request.payload)?;
    let mut tx = pool.begin().await?;
    if let Some(existing) = load_by_idempotency(
        &mut tx,
        &request.idempotency_scope,
        &request.idempotency_key,
    )
    .await?
    {
        if !record_matches_request(&existing, &request) {
            return Err(StoreError::Conflict(
                "audit idempotency key was reused with a changed envelope".to_string(),
            ));
        }
        tx.commit().await?;
        return Ok(existing);
    }
    if load_by_id(&mut tx, &request.id).await?.is_some() {
        return Err(StoreError::Conflict(
            "audit event id was reused for another idempotency key".to_string(),
        ));
    }
    validate_parent_graph(&mut tx, &request).await?;
    let recorded_at = now_unix();
    let sequence: i64 = sqlx::query_scalar(
        "INSERT INTO execution_audit_events \
         (id, idempotency_scope, idempotency_key, event_type, actor_kind, actor_ref, \
          payload_sha256, payload_json, execution_run_id, execution_task_id, \
          runtime_session_id, runtime_attempt_id, execution_artifact_id, \
          worker_incarnation_id, snapshot_authority_key, snapshot_resource_kind, \
          snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256, \
          target_repository_id, target_base_commit, occurred_at, recorded_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING sequence",
    )
    .bind(request.id.as_str())
    .bind(&request.idempotency_scope)
    .bind(&request.idempotency_key)
    .bind(&request.event_type)
    .bind(request.actor_kind.as_str())
    .bind(&request.actor_ref)
    .bind(&request.payload_sha256)
    .bind(payload_json)
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
    .bind(
        request
            .execution_artifact_id
            .as_ref()
            .map(ExecutionArtifactId::as_str),
    )
    .bind(
        request
            .worker_incarnation_id
            .as_ref()
            .map(WorkerIncarnationId::as_str),
    )
    .bind(&request.snapshot.authority_key)
    .bind(&request.snapshot.resource_kind)
    .bind(&request.snapshot.resource_id)
    .bind(&request.snapshot.resource_version)
    .bind(&request.snapshot.content_sha256)
    .bind(&request.target_repository_id)
    .bind(&request.target_base_commit)
    .bind(request.occurred_at)
    .bind(recorded_at)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(record_from_request(sequence, recorded_at, request))
}

/// Replay one run's audit rows after a stable database sequence cursor.
///
/// # Errors
/// Returns [`StoreError`] when rows cannot be queried or decoded.
pub async fn read_from(
    pool: &SqlitePool,
    run_id: &RunId,
    after_sequence: i64,
) -> Result<Vec<AuditEventRecord>, StoreError> {
    if after_sequence < 0 {
        return Err(StoreError::Invariant(
            "audit sequence cursor must be non-negative".to_string(),
        ));
    }
    let rows = sqlx::query(
        "SELECT sequence, id, idempotency_scope, idempotency_key, event_type, actor_kind, \
         actor_ref, payload_sha256, payload_json, execution_run_id, execution_task_id, \
         runtime_session_id, runtime_attempt_id, execution_artifact_id, \
         worker_incarnation_id, snapshot_authority_key, snapshot_resource_kind, \
         snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256, \
         target_repository_id, target_base_commit, occurred_at, recorded_at \
         FROM execution_audit_events \
         WHERE execution_run_id = ? AND sequence > ? ORDER BY sequence ASC",
    )
    .bind(run_id.as_str())
    .bind(after_sequence)
    .fetch_all(pool)
    .await?;
    rows.iter().map(row_to_event).collect()
}


async fn load_by_idempotency(
    tx: &mut Transaction<'_, Sqlite>,
    scope: &str,
    key: &str,
) -> Result<Option<AuditEventRecord>, StoreError> {
    let row = sqlx::query(
        "SELECT sequence, id, idempotency_scope, idempotency_key, event_type, actor_kind, \
         actor_ref, payload_sha256, payload_json, execution_run_id, execution_task_id, \
         runtime_session_id, runtime_attempt_id, execution_artifact_id, \
         worker_incarnation_id, snapshot_authority_key, snapshot_resource_kind, \
         snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256, \
         target_repository_id, target_base_commit, occurred_at, recorded_at \
         FROM execution_audit_events WHERE idempotency_scope = ? AND idempotency_key = ?",
    )
    .bind(scope)
    .bind(key)
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(row_to_event).transpose()
}

async fn load_by_id(
    tx: &mut Transaction<'_, Sqlite>,
    id: &AuditEventId,
) -> Result<Option<AuditEventRecord>, StoreError> {
    let row = sqlx::query(
        "SELECT sequence, id, idempotency_scope, idempotency_key, event_type, actor_kind, \
         actor_ref, payload_sha256, payload_json, execution_run_id, execution_task_id, \
         runtime_session_id, runtime_attempt_id, execution_artifact_id, \
         worker_incarnation_id, snapshot_authority_key, snapshot_resource_kind, \
         snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256, \
         target_repository_id, target_base_commit, occurred_at, recorded_at \
         FROM execution_audit_events WHERE id = ?",
    )
    .bind(id.as_str())
    .fetch_optional(&mut **tx)
    .await?;
    row.as_ref().map(row_to_event).transpose()
}

async fn validate_parent_graph(
    tx: &mut Transaction<'_, Sqlite>,
    request: &AuditEventCreate,
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
        if run_id.as_deref().ok_or(StoreError::NotFound)? != request.execution_run_id.as_str() {
            return Err(StoreError::Conflict(
                "audit task does not belong to audit run".to_string(),
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
        if row.get::<String, _>("execution_task_id")
            != request
                .execution_task_id
                .as_ref()
                .map(TaskRunId::as_str)
                .unwrap_or_default()
            || !snapshot_columns_match(&row, &request.snapshot)
        {
            return Err(StoreError::Conflict(
                "audit session does not match task or snapshot".to_string(),
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
        if row.get::<String, _>("runtime_session_id")
            != request
                .runtime_session_id
                .as_ref()
                .map(RuntimeSessionId::as_str)
                .unwrap_or_default()
        {
            return Err(StoreError::Conflict(
                "audit attempt does not belong to audit session".to_string(),
            ));
        }
        attempt_worker = Some(row.get::<String, _>("worker_incarnation_id"));
    }
    if let Some(worker_id) = &request.worker_incarnation_id {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM worker_incarnations WHERE id = ?)")
                .bind(worker_id.as_str())
                .fetch_one(&mut **tx)
                .await?;
        if !exists {
            return Err(StoreError::NotFound);
        }
        if let Some(attempt_worker) = &attempt_worker
            && attempt_worker != worker_id.as_str()
        {
            return Err(StoreError::Conflict(
                "audit worker does not match attempt worker".to_string(),
            ));
        }
    }
    if let Some(artifact_id) = &request.execution_artifact_id {
        validate_artifact_link(tx, request, artifact_id).await?;
    }
    Ok(())
}

async fn validate_artifact_link(
    tx: &mut Transaction<'_, Sqlite>,
    request: &AuditEventCreate,
    artifact_id: &ExecutionArtifactId,
) -> Result<(), StoreError> {
    let row = sqlx::query(
        "SELECT execution_run_id, execution_task_id, runtime_session_id, runtime_attempt_id, \
         producer_worker_incarnation_id, snapshot_authority_key, snapshot_resource_kind, \
         snapshot_resource_id, snapshot_resource_version, snapshot_content_sha256, \
         target_repository_id, target_base_commit FROM execution_artifacts WHERE id = ?",
    )
    .bind(artifact_id.as_str())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(StoreError::NotFound)?;
    let matches = row.get::<String, _>("execution_run_id") == request.execution_run_id.as_str()
        && row.get::<Option<String>, _>("execution_task_id").as_deref()
            == request.execution_task_id.as_ref().map(TaskRunId::as_str)
        && row
            .get::<Option<String>, _>("runtime_session_id")
            .as_deref()
            == request
                .runtime_session_id
                .as_ref()
                .map(RuntimeSessionId::as_str)
        && row
            .get::<Option<String>, _>("runtime_attempt_id")
            .as_deref()
            == request
                .runtime_attempt_id
                .as_ref()
                .map(RuntimeAttemptId::as_str)
        && row
            .get::<Option<String>, _>("producer_worker_incarnation_id")
            .as_deref()
            == request
                .worker_incarnation_id
                .as_ref()
                .map(WorkerIncarnationId::as_str)
        && snapshot_columns_match(&row, &request.snapshot)
        && row.get::<String, _>("target_repository_id") == request.target_repository_id
        && row.get::<String, _>("target_base_commit") == request.target_base_commit;
    if !matches {
        return Err(StoreError::Conflict(
            "audit artifact does not match the execution envelope".to_string(),
        ));
    }
    Ok(())
}

fn snapshot_columns_match(row: &sqlx::sqlite::SqliteRow, snapshot: &ExecutionSnapshotRef) -> bool {
    row.get::<String, _>("snapshot_authority_key") == snapshot.authority_key
        && row.get::<String, _>("snapshot_resource_kind") == snapshot.resource_kind
        && row.get::<String, _>("snapshot_resource_id") == snapshot.resource_id
        && row.get::<String, _>("snapshot_resource_version") == snapshot.resource_version
        && row.get::<String, _>("snapshot_content_sha256") == snapshot.content_sha256
}

fn validate_event(request: &AuditEventCreate) -> Result<(), StoreError> {
    validate_id(request.id.as_str(), "ae_", "AuditEventId")?;
    validate_text(&request.idempotency_scope, "audit idempotency scope")?;
    validate_text(&request.idempotency_key, "audit idempotency key")?;
    validate_text(&request.event_type, "audit event type")?;
    validate_text(&request.actor_ref, "audit actor ref")?;
    validate_sha256(&request.payload_sha256, "audit payload sha256")?;
    if !request.payload.is_object() {
        return Err(StoreError::Invariant(
            "audit payload must be a JSON object".to_string(),
        ));
    }
    validate_snapshot(&request.snapshot)?;
    validate_text(&request.target_repository_id, "target repository id")?;
    validate_text(&request.target_base_commit, "target base commit")?;
    if request.runtime_session_id.is_some() && request.execution_task_id.is_none() {
        return Err(StoreError::Invariant(
            "audit runtime session requires execution task".to_string(),
        ));
    }
    if request.runtime_attempt_id.is_some() && request.runtime_session_id.is_none() {
        return Err(StoreError::Invariant(
            "audit runtime attempt requires runtime session".to_string(),
        ));
    }
    for (value, prefix, name) in [
        (
            request
                .runtime_session_id
                .as_ref()
                .map(RuntimeSessionId::as_str),
            "rs_",
            "RuntimeSessionId",
        ),
        (
            request
                .runtime_attempt_id
                .as_ref()
                .map(RuntimeAttemptId::as_str),
            "ra_",
            "RuntimeAttemptId",
        ),
        (
            request
                .execution_artifact_id
                .as_ref()
                .map(ExecutionArtifactId::as_str),
            "ar_",
            "ExecutionArtifactId",
        ),
        (
            request
                .worker_incarnation_id
                .as_ref()
                .map(WorkerIncarnationId::as_str),
            "wi_",
            "WorkerIncarnationId",
        ),
    ] {
        if let Some(value) = value {
            validate_id(value, prefix, name)?;
        }
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

fn record_matches_request(record: &AuditEventRecord, request: &AuditEventCreate) -> bool {
    record.id == request.id
        && record.idempotency_scope == request.idempotency_scope
        && record.idempotency_key == request.idempotency_key
        && record.event_type == request.event_type
        && record.actor_kind == request.actor_kind
        && record.actor_ref == request.actor_ref
        && record.payload_sha256 == request.payload_sha256
        && record.payload == request.payload
        && record.execution_run_id == request.execution_run_id
        && record.execution_task_id == request.execution_task_id
        && record.runtime_session_id == request.runtime_session_id
        && record.runtime_attempt_id == request.runtime_attempt_id
        && record.execution_artifact_id == request.execution_artifact_id
        && record.worker_incarnation_id == request.worker_incarnation_id
        && record.snapshot == request.snapshot
        && record.target_repository_id == request.target_repository_id
        && record.target_base_commit == request.target_base_commit
        && record.occurred_at == request.occurred_at
}

fn record_from_request(
    sequence: i64,
    recorded_at: i64,
    request: AuditEventCreate,
) -> AuditEventRecord {
    AuditEventRecord {
        sequence,
        id: request.id,
        idempotency_scope: request.idempotency_scope,
        idempotency_key: request.idempotency_key,
        event_type: request.event_type,
        actor_kind: request.actor_kind,
        actor_ref: request.actor_ref,
        payload_sha256: request.payload_sha256,
        payload: request.payload,
        execution_run_id: request.execution_run_id,
        execution_task_id: request.execution_task_id,
        runtime_session_id: request.runtime_session_id,
        runtime_attempt_id: request.runtime_attempt_id,
        execution_artifact_id: request.execution_artifact_id,
        worker_incarnation_id: request.worker_incarnation_id,
        snapshot: request.snapshot,
        target_repository_id: request.target_repository_id,
        target_base_commit: request.target_base_commit,
        occurred_at: request.occurred_at,
        recorded_at,
    }
}

fn row_to_event(row: &sqlx::sqlite::SqliteRow) -> Result<AuditEventRecord, StoreError> {
    let actor_text: String = row.get("actor_kind");
    let actor_kind = AuditActorKind::try_from(actor_text.as_str())
        .map_err(|()| StoreError::Invariant(format!("unknown audit actor kind: {actor_text}")))?;
    let payload_json: String = row.get("payload_json");
    Ok(AuditEventRecord {
        sequence: row.get("sequence"),
        id: AuditEventId::from_string(row.get::<String, _>("id")),
        idempotency_scope: row.get("idempotency_scope"),
        idempotency_key: row.get("idempotency_key"),
        event_type: row.get("event_type"),
        actor_kind,
        actor_ref: row.get("actor_ref"),
        payload_sha256: row.get("payload_sha256"),
        payload: serde_json::from_str(&payload_json)?,
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
        execution_artifact_id: row
            .get::<Option<String>, _>("execution_artifact_id")
            .map(ExecutionArtifactId::from_string),
        worker_incarnation_id: row
            .get::<Option<String>, _>("worker_incarnation_id")
            .map(WorkerIncarnationId::from_string),
        snapshot: ExecutionSnapshotRef {
            authority_key: row.get("snapshot_authority_key"),
            resource_kind: row.get("snapshot_resource_kind"),
            resource_id: row.get("snapshot_resource_id"),
            resource_version: row.get("snapshot_resource_version"),
            content_sha256: row.get("snapshot_content_sha256"),
        },
        target_repository_id: row.get("target_repository_id"),
        target_base_commit: row.get("target_base_commit"),
        occurred_at: row.get("occurred_at"),
        recorded_at: row.get("recorded_at"),
    })
}
