//! Durable AD-E7 enterprise scale reference control plane.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use agentd_core::ports::{
    ArtifactReplicaAcknowledgement, ArtifactReplicationPlan, AutoscalingRecommendation,
    CapacityObservation, ControlPlaneHeartbeatRequest, ControlPlaneLeadershipLease,
    ControlPlaneLeadershipRenewal, ControlPlaneLeadershipRequest, ControlPlaneMember,
    ControlPlaneMemberStatus, DisasterRecoveryCheckpoint, DisasterRecoveryDrill,
    DisasterRecoveryDrillStatus, EnterpriseMutationFence, EnterpriseOperationalSnapshot,
    EnterpriseScaleError, EnterpriseScalePort, EnterpriseZoneStatus, LegalHold,
    LoadModelRegistration, ReplicaStatus, RetentionDecision, RetentionDisposition, RetentionPolicy,
    ServiceLevelMeasurement, TenantKeyStatus, TenantKeyTransition, TenantKeyVersion,
    WorkerImageRollback, WorkerImageRollout, WorkerImageRolloutStatus, WorkerImageZoneObservation,
    ZonePoolPolicy,
};
use agentd_core::types::{
    ArtifactReplicationId, ControlPlaneInstanceId, DisasterRecoveryCheckpointId, LegalHoldId,
    LoadModelId, TenantKeyId, WorkerImageRolloutId, ZonePoolId,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqliteConnection, SqlitePool};

use crate::util::SqliteImmediateTransaction;

const REQUIRED_LOAD_DIMENSIONS: [&str; 11] = [
    "tenant",
    "project",
    "room",
    "matrix_event",
    "queue",
    "artifact_log",
    "certification_throughput",
    "failure_injection",
    "test_window",
    "noisy_neighbor",
    "budget",
];

#[derive(Clone)]
pub struct SqliteEnterpriseScaleControlPlane {
    pool: SqlitePool,
}

impl std::fmt::Debug for SqliteEnterpriseScaleControlPlane {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SqliteEnterpriseScaleControlPlane")
            .field("pool", &"[SQLITE]")
            .finish()
    }
}

impl SqliteEnterpriseScaleControlPlane {
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

async fn begin_immediate(
    pool: &SqlitePool,
) -> Result<SqliteImmediateTransaction, EnterpriseScaleError> {
    SqliteImmediateTransaction::begin(pool)
        .await
        .map_err(storage_error)
}

async fn begin_fenced_mutation(
    pool: &SqlitePool,
    fence: &EnterpriseMutationFence,
    operation: &str,
    resource_key: &str,
    mutation_sha256: &str,
) -> Result<SqliteImmediateTransaction, EnterpriseScaleError> {
    if !valid_typed_id(fence.instance_id.as_str(), "ci_")
        || fence.observed_at < 0
        || fence.term == 0
        || fence.fencing_token == 0
        || !nonempty(operation)
        || !nonempty(resource_key)
        || !valid_sha256(mutation_sha256)
    {
        return Err(EnterpriseScaleError::Invalid(
            "enterprise mutation fence is invalid".to_string(),
        ));
    }
    let mut connection = begin_immediate(pool).await?;
    let row = sqlx::query(
        "SELECT leadership.instance_id, leadership.term, leadership.fencing_token, \
                leadership.renewed_at, leadership.expires_at, member.status AS member_status \
         FROM enterprise_control_plane_leadership AS leadership \
         JOIN enterprise_control_plane_members AS member \
           ON member.instance_id = leadership.instance_id \
         WHERE leadership.singleton = 1",
    )
    .fetch_optional(&mut *connection)
    .await
    .map_err(storage_error)?;
    let current = row.is_some_and(|row| {
        row.get::<String, _>("instance_id") == fence.instance_id.as_str()
            && row.get::<i64, _>("term") == i64::try_from(fence.term).unwrap_or(-1)
            && row.get::<i64, _>("fencing_token")
                == i64::try_from(fence.fencing_token).unwrap_or(-1)
            && row.get::<String, _>("member_status") == "ready"
            && row.get::<i64, _>("renewed_at") <= fence.observed_at
            && row.get::<i64, _>("expires_at") > fence.observed_at
    });
    if !current {
        rollback(&mut connection).await;
        return Err(EnterpriseScaleError::Denied(
            "stale or expired enterprise leadership fence".to_string(),
        ));
    }
    sqlx::query(
        "INSERT INTO enterprise_mutation_fences \
         (operation, resource_key, mutation_sha256, instance_id, term, fencing_token, observed_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(operation)
    .bind(resource_key)
    .bind(mutation_sha256)
    .bind(fence.instance_id.as_str())
    .bind(u64_to_i64(fence.term, "leadership term")?)
    .bind(u64_to_i64(fence.fencing_token, "leadership fencing token")?)
    .bind(fence.observed_at)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    Ok(connection)
}

async fn commit(connection: &mut SqliteImmediateTransaction) -> Result<(), EnterpriseScaleError> {
    connection.commit().await.map_err(storage_error)
}

async fn rollback(connection: &mut SqliteImmediateTransaction) {
    let _ = connection.rollback().await;
}

#[allow(clippy::needless_pass_by_value)]
fn storage_error(error: sqlx::Error) -> EnterpriseScaleError {
    EnterpriseScaleError::Unavailable(error.to_string())
}

#[allow(clippy::needless_pass_by_value)]
fn serde_error(error: serde_json::Error) -> EnterpriseScaleError {
    EnterpriseScaleError::Invalid(error.to_string())
}

fn canonical_sha256<T: Serialize>(value: &T) -> Result<String, EnterpriseScaleError> {
    let bytes = serde_json::to_vec(value).map_err(serde_error)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn json<T: Serialize>(value: &T) -> Result<String, EnterpriseScaleError> {
    serde_json::to_string(value).map_err(serde_error)
}

fn parse_json<T: DeserializeOwned>(value: &str) -> Result<T, EnterpriseScaleError> {
    serde_json::from_str(value).map_err(serde_error)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_image_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(valid_sha256)
}

fn nonempty(value: &str) -> bool {
    value == value.trim()
        && !value.is_empty()
        && value.len() <= 512
        && !value.chars().any(char::is_control)
}

fn valid_typed_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|payload| {
        payload.len() == 26
            && payload
                .parse::<ulid::Ulid>()
                .is_ok_and(|parsed| parsed.to_string() == payload)
    })
}

fn valid_trust_domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

const fn replica_transition_allowed(previous: ReplicaStatus, target: ReplicaStatus) -> bool {
    matches!(
        (previous, target),
        (
            ReplicaStatus::Pending,
            ReplicaStatus::Available | ReplicaStatus::Failed
        ) | (
            ReplicaStatus::Failed,
            ReplicaStatus::Pending | ReplicaStatus::Available
        )
    )
}

fn u64_to_i64(value: u64, field: &str) -> Result<i64, EnterpriseScaleError> {
    i64::try_from(value)
        .map_err(|_| EnterpriseScaleError::Invalid(format!("{field} exceeds SQLite range")))
}

fn i64_to_u64(value: i64, field: &str) -> Result<u64, EnterpriseScaleError> {
    u64::try_from(value)
        .map_err(|_| EnterpriseScaleError::Unavailable(format!("stored {field} is negative")))
}

fn i64_to_u32(value: i64, field: &str) -> Result<u32, EnterpriseScaleError> {
    u32::try_from(value)
        .map_err(|_| EnterpriseScaleError::Unavailable(format!("stored {field} is invalid")))
}

async fn replay_receipt<T: DeserializeOwned>(
    connection: &mut SqliteImmediateTransaction,
    operation: &str,
    idempotency_key: &str,
    request_sha256: &str,
) -> Result<Option<T>, EnterpriseScaleError> {
    let row = sqlx::query(
        "SELECT request_sha256, response_json FROM enterprise_scale_receipts \
         WHERE operation = ? AND idempotency_key = ?",
    )
    .bind(operation)
    .bind(idempotency_key)
    .fetch_optional(&mut **connection)
    .await
    .map_err(storage_error)?;
    let Some(row) = row else {
        return Ok(None);
    };
    if row.get::<String, _>("request_sha256") != request_sha256 {
        return Err(EnterpriseScaleError::Conflict(format!(
            "{operation} idempotency key was reused with different input"
        )));
    }
    parse_json(&row.get::<String, _>("response_json")).map(Some)
}

async fn insert_receipt<T: Serialize>(
    connection: &mut SqliteImmediateTransaction,
    operation: &str,
    idempotency_key: &str,
    request_sha256: &str,
    response: &T,
    recorded_at: i64,
) -> Result<(), EnterpriseScaleError> {
    sqlx::query(
        "INSERT INTO enterprise_scale_receipts \
         (operation, idempotency_key, request_sha256, response_json, recorded_at) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(operation)
    .bind(idempotency_key)
    .bind(request_sha256)
    .bind(json(response)?)
    .bind(recorded_at)
    .execute(&mut **connection)
    .await
    .map_err(storage_error)?;
    Ok(())
}

fn member_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ControlPlaneMember, EnterpriseScaleError> {
    Ok(ControlPlaneMember {
        instance_id: ControlPlaneInstanceId::from_string(row.get::<String, _>("instance_id")),
        heartbeat_sequence: i64_to_u64(row.get("heartbeat_sequence"), "heartbeat sequence")?,
        region: row.get("region"),
        zone: row.get("zone"),
        daemon_version: row.get("daemon_version"),
        endpoint_sha256: row.get("endpoint_sha256"),
        status: ControlPlaneMemberStatus::try_from(row.get::<String, _>("status").as_str())
            .map_err(|error| EnterpriseScaleError::Unavailable(error.to_string()))?,
        started_at: row.get("started_at"),
        observed_at: row.get("observed_at"),
    })
}

fn leadership_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ControlPlaneLeadershipLease, EnterpriseScaleError> {
    Ok(ControlPlaneLeadershipLease {
        instance_id: ControlPlaneInstanceId::from_string(row.get::<String, _>("instance_id")),
        term: i64_to_u64(row.get("term"), "leadership term")?,
        fencing_token: i64_to_u64(row.get("fencing_token"), "leadership fencing token")?,
        acquired_at: row.get("acquired_at"),
        renewed_at: row.get("renewed_at"),
        expires_at: row.get("expires_at"),
    })
}

fn rollout_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<WorkerImageRollout, EnterpriseScaleError> {
    Ok(WorkerImageRollout {
        rollout_id: WorkerImageRolloutId::from_string(row.get::<String, _>("rollout_id")),
        image_digest: row.get("image_digest"),
        signature_bundle_sha256: row.get("signature_bundle_sha256"),
        policy_sha256: row.get("policy_sha256"),
        required_zones: parse_json(&row.get::<String, _>("required_zones_json"))?,
        status: WorkerImageRolloutStatus::try_from(row.get::<String, _>("status").as_str())
            .map_err(|error| EnterpriseScaleError::Unavailable(error.to_string()))?,
        declared_at: row.get("declared_at"),
        updated_at: row.get("updated_at"),
    })
}

async fn load_rollout(
    pool: &SqlitePool,
    rollout_id: &WorkerImageRolloutId,
) -> Result<WorkerImageRollout, EnterpriseScaleError> {
    let row = sqlx::query("SELECT * FROM enterprise_worker_image_rollouts WHERE rollout_id = ?")
        .bind(rollout_id.as_str())
        .fetch_optional(pool)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| EnterpriseScaleError::NotFound("worker image rollout".to_string()))?;
    rollout_from_row(&row)
}

async fn load_rollout_in_transaction(
    connection: &mut SqliteConnection,
    rollout_id: &WorkerImageRolloutId,
) -> Result<WorkerImageRollout, EnterpriseScaleError> {
    let row = sqlx::query("SELECT * FROM enterprise_worker_image_rollouts WHERE rollout_id = ?")
        .bind(rollout_id.as_str())
        .fetch_optional(connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| EnterpriseScaleError::NotFound("worker image rollout".to_string()))?;
    rollout_from_row(&row)
}

fn tenant_key_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<TenantKeyVersion, EnterpriseScaleError> {
    Ok(TenantKeyVersion {
        tenant_key_id: TenantKeyId::from_string(row.get::<String, _>("tenant_key_id")),
        tenant_scope_sha256: row.get("tenant_scope_sha256"),
        region: row.get("region"),
        kms_key_ref_sha256: row.get("kms_key_ref_sha256"),
        key_version_ref_sha256: row.get("key_version_ref_sha256"),
        status: TenantKeyStatus::try_from(row.get::<String, _>("status").as_str())
            .map_err(|error| EnterpriseScaleError::Unavailable(error.to_string()))?,
        activated_at: row.get("activated_at"),
        retired_at: row.get("retired_at"),
    })
}

async fn load_tenant_key(
    pool: &SqlitePool,
    tenant_key_id: &TenantKeyId,
) -> Result<TenantKeyVersion, EnterpriseScaleError> {
    let row = sqlx::query("SELECT * FROM enterprise_tenant_keys WHERE tenant_key_id = ?")
        .bind(tenant_key_id.as_str())
        .fetch_optional(pool)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| EnterpriseScaleError::NotFound("tenant key version".to_string()))?;
    tenant_key_from_row(&row)
}

fn artifact_replica_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ArtifactReplicaAcknowledgement, EnterpriseScaleError> {
    Ok(ArtifactReplicaAcknowledgement {
        replication_id: ArtifactReplicationId::from_string(row.get::<String, _>("replication_id")),
        region: row.get("region"),
        artifact_sha256: row.get("artifact_sha256"),
        object_ref_sha256: row.get("object_ref_sha256"),
        tenant_key_id: TenantKeyId::from_string(row.get::<String, _>("tenant_key_id")),
        status: ReplicaStatus::try_from(row.get::<String, _>("status").as_str())
            .map_err(|error| EnterpriseScaleError::Unavailable(error.to_string()))?,
        acknowledged_at: row.get("acknowledged_at"),
    })
}

fn zone_policy_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ZonePoolPolicy, EnterpriseScaleError> {
    Ok(ZonePoolPolicy {
        pool_id: ZonePoolId::from_string(row.get::<String, _>("pool_id")),
        region: row.get("region"),
        zone: row.get("zone"),
        resource_class: row.get("resource_class"),
        trust_domain: row.get("trust_domain"),
        rollout_id: WorkerImageRolloutId::from_string(row.get::<String, _>("rollout_id")),
        minimum_replicas: i64_to_u32(row.get("minimum_replicas"), "minimum replicas")?,
        maximum_replicas: i64_to_u32(row.get("maximum_replicas"), "maximum replicas")?,
        target_queue_per_slot: i64_to_u32(
            row.get("target_queue_per_slot"),
            "target queue per slot",
        )?,
        scale_down_cooldown_seconds: i64_to_u32(
            row.get("scale_down_cooldown_seconds"),
            "scale down cooldown",
        )?,
        enabled: row.get::<i64, _>("enabled") != 0,
        policy_sha256: row.get("policy_sha256"),
        updated_at: row.get("updated_at"),
    })
}

fn retention_policy_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<RetentionPolicy, EnterpriseScaleError> {
    Ok(RetentionPolicy {
        tenant_scope_sha256: row.get("tenant_scope_sha256"),
        policy_version_sha256: row.get("policy_version_sha256"),
        artifact_retention_seconds: i64_to_u64(
            row.get("artifact_retention_seconds"),
            "artifact retention",
        )?,
        transcript_retention_seconds: i64_to_u64(
            row.get("transcript_retention_seconds"),
            "transcript retention",
        )?,
        audit_retention_seconds: i64_to_u64(row.get("audit_retention_seconds"), "audit retention")?,
        minimum_replica_regions: i64_to_u32(
            row.get("minimum_replica_regions"),
            "minimum replica regions",
        )?,
        updated_at: row.get("updated_at"),
    })
}

async fn load_zone_policy_in_transaction(
    connection: &mut SqliteConnection,
    pool_id: &ZonePoolId,
) -> Result<ZonePoolPolicy, EnterpriseScaleError> {
    let row = sqlx::query("SELECT * FROM enterprise_zone_pool_policies WHERE pool_id = ?")
        .bind(pool_id.as_str())
        .fetch_optional(connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| EnterpriseScaleError::NotFound("zone pool policy".to_string()))?;
    zone_policy_from_row(&row)
}

fn legal_hold_from_row(row: &sqlx::sqlite::SqliteRow) -> LegalHold {
    LegalHold {
        legal_hold_id: LegalHoldId::from_string(row.get::<String, _>("legal_hold_id")),
        tenant_scope_sha256: row.get("tenant_scope_sha256"),
        subject_kind: row.get("subject_kind"),
        subject_sha256: row.get("subject_sha256"),
        reason_sha256: row.get("reason_sha256"),
        active: row.get::<i64, _>("active") != 0,
        placed_at: row.get("placed_at"),
        released_at: row.get("released_at"),
    }
}

fn checkpoint_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<DisasterRecoveryCheckpoint, EnterpriseScaleError> {
    Ok(DisasterRecoveryCheckpoint {
        checkpoint_id: DisasterRecoveryCheckpointId::from_string(
            row.get::<String, _>("checkpoint_id"),
        ),
        region: row.get("region"),
        database_sha256: row.get("database_sha256"),
        artifact_index_sha256: row.get("artifact_index_sha256"),
        audit_head_sha256: row.get("audit_head_sha256"),
        matrix_cursor_sha256: row.get("matrix_cursor_sha256"),
        certification_head_sha256: row.get("certification_head_sha256"),
        maximum_rpo_seconds: i64_to_u32(row.get("maximum_rpo_seconds"), "maximum RPO")?,
        maximum_rto_seconds: i64_to_u32(row.get("maximum_rto_seconds"), "maximum RTO")?,
        created_at: row.get("created_at"),
    })
}

fn load_model_from_row(
    row: &sqlx::sqlite::SqliteRow,
) -> Result<LoadModelRegistration, EnterpriseScaleError> {
    Ok(LoadModelRegistration {
        load_model_id: LoadModelId::from_string(row.get::<String, _>("load_model_id")),
        version: row.get("version"),
        content_sha256: row.get("content_sha256"),
        dimensions: parse_json(&row.get::<String, _>("dimensions_json"))?,
        test_window_seconds: i64_to_u32(row.get("test_window_seconds"), "test window")?,
        tenant_count: i64_to_u32(row.get("tenant_count"), "tenant count")?,
        project_count: i64_to_u32(row.get("project_count"), "project count")?,
        room_count: i64_to_u32(row.get("room_count"), "room count")?,
        matrix_events_per_second: i64_to_u32(
            row.get("matrix_events_per_second"),
            "Matrix events per second",
        )?,
        maximum_queue_depth: i64_to_u64(row.get("maximum_queue_depth"), "maximum queue depth")?,
        noisy_neighbor_ratio_basis_points: i64_to_u32(
            row.get("noisy_neighbor_ratio_basis_points"),
            "noisy-neighbor ratio",
        )?,
        registered_at: row.get("registered_at"),
    })
}

fn validate_heartbeat(request: &ControlPlaneHeartbeatRequest) -> Result<(), EnterpriseScaleError> {
    let member = &request.member;
    if !nonempty(&request.idempotency_key)
        || !valid_typed_id(member.instance_id.as_str(), "ci_")
        || member.heartbeat_sequence == 0
        || member.heartbeat_sequence > i64::MAX.unsigned_abs()
        || !nonempty(&member.region)
        || !nonempty(&member.zone)
        || !nonempty(&member.daemon_version)
        || !valid_sha256(&member.endpoint_sha256)
        || member.started_at < 0
        || member.observed_at < member.started_at
    {
        return Err(EnterpriseScaleError::Invalid(
            "control-plane heartbeat fields are invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_leadership_window(
    idempotency_key: &str,
    observed_at: i64,
    expires_at: i64,
) -> Result<(), EnterpriseScaleError> {
    if !nonempty(idempotency_key)
        || observed_at < 0
        || expires_at <= observed_at
        || expires_at.saturating_sub(observed_at) > 300
    {
        return Err(EnterpriseScaleError::Invalid(
            "leadership lease must be positive and at most 300 seconds".to_string(),
        ));
    }
    Ok(())
}

fn validate_rollout(rollout: &WorkerImageRollout) -> Result<(), EnterpriseScaleError> {
    if !valid_typed_id(rollout.rollout_id.as_str(), "ir_")
        || !valid_image_digest(&rollout.image_digest)
        || !valid_sha256(&rollout.signature_bundle_sha256)
        || !valid_sha256(&rollout.policy_sha256)
        || rollout.required_zones.is_empty()
        || rollout.required_zones.len() > 128
        || rollout.required_zones.iter().any(|zone| !nonempty(zone))
        || rollout.status != WorkerImageRolloutStatus::Declared
        || rollout.declared_at < 0
        || rollout.updated_at != rollout.declared_at
    {
        return Err(EnterpriseScaleError::Invalid(
            "worker image rollout is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_zone_policy(policy: &ZonePoolPolicy) -> Result<(), EnterpriseScaleError> {
    if !valid_typed_id(policy.pool_id.as_str(), "zp_")
        || !valid_typed_id(policy.rollout_id.as_str(), "ir_")
        || !nonempty(&policy.region)
        || !nonempty(&policy.zone)
        || !nonempty(&policy.resource_class)
        || !valid_trust_domain(&policy.trust_domain)
        || policy.minimum_replicas == 0
        || policy.maximum_replicas < policy.minimum_replicas
        || policy.maximum_replicas > 100_000
        || policy.target_queue_per_slot == 0
        || policy.scale_down_cooldown_seconds == 0
        || policy.scale_down_cooldown_seconds > 86_400
        || !valid_sha256(&policy.policy_sha256)
        || policy.updated_at < 0
    {
        return Err(EnterpriseScaleError::Invalid(
            "zone pool policy is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_replication_plan(plan: &ArtifactReplicationPlan) -> Result<(), EnterpriseScaleError> {
    if !valid_typed_id(plan.replication_id.as_str(), "rp_")
        || !valid_typed_id(plan.execution_artifact_id.as_str(), "ar_")
        || !valid_sha256(&plan.tenant_scope_sha256)
        || !valid_sha256(&plan.artifact_sha256)
        || !nonempty(&plan.source_region)
        || plan.required_regions.is_empty()
        || plan.required_regions.len() > 32
        || !plan.required_regions.contains(&plan.source_region)
        || plan.required_regions.iter().any(|region| !nonempty(region))
        || plan.created_at < 0
    {
        return Err(EnterpriseScaleError::Invalid(
            "artifact replication plan is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_tenant_key(key: &TenantKeyVersion) -> Result<(), EnterpriseScaleError> {
    if !valid_typed_id(key.tenant_key_id.as_str(), "tk_")
        || !valid_sha256(&key.tenant_scope_sha256)
        || !nonempty(&key.region)
        || !valid_sha256(&key.kms_key_ref_sha256)
        || !valid_sha256(&key.key_version_ref_sha256)
        || key.status != TenantKeyStatus::Active
        || key.activated_at < 0
        || key.retired_at.is_some()
    {
        return Err(EnterpriseScaleError::Invalid(
            "new tenant key reference must start active without retirement state".to_string(),
        ));
    }
    Ok(())
}

fn validate_retention_policy(policy: &RetentionPolicy) -> Result<(), EnterpriseScaleError> {
    if !valid_sha256(&policy.tenant_scope_sha256)
        || !valid_sha256(&policy.policy_version_sha256)
        || policy.artifact_retention_seconds == 0
        || policy.transcript_retention_seconds == 0
        || policy.audit_retention_seconds < policy.artifact_retention_seconds
        || policy.audit_retention_seconds < policy.transcript_retention_seconds
        || policy.minimum_replica_regions == 0
        || policy.minimum_replica_regions > 32
        || policy.updated_at < 0
    {
        return Err(EnterpriseScaleError::Invalid(
            "retention policy is invalid".to_string(),
        ));
    }
    Ok(())
}

fn validate_legal_hold(hold: &LegalHold) -> Result<(), EnterpriseScaleError> {
    if !valid_typed_id(hold.legal_hold_id.as_str(), "lh_")
        || !valid_sha256(&hold.tenant_scope_sha256)
        || !nonempty(&hold.subject_kind)
        || !valid_sha256(&hold.subject_sha256)
        || !valid_sha256(&hold.reason_sha256)
        || !hold.active
        || hold.released_at.is_some()
        || hold.placed_at < 0
    {
        return Err(EnterpriseScaleError::Invalid(
            "new legal hold must be active and digest-only".to_string(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
#[async_trait::async_trait]
impl EnterpriseScalePort for SqliteEnterpriseScaleControlPlane {
    async fn heartbeat_control_plane(
        &self,
        request: &ControlPlaneHeartbeatRequest,
    ) -> Result<ControlPlaneMember, EnterpriseScaleError> {
        validate_heartbeat(request)?;
        let request_sha = canonical_sha256(request)?;
        let mut connection = begin_immediate(&self.pool).await?;
        if let Some(replayed) = replay_receipt(
            &mut connection,
            "control_plane.heartbeat",
            request.idempotency_key.trim(),
            &request_sha,
        )
        .await?
        {
            commit(&mut connection).await?;
            return Ok(replayed);
        }
        let current = sqlx::query(
            "SELECT heartbeat_sequence, region, zone, endpoint_sha256, started_at, observed_at \
             FROM enterprise_control_plane_members WHERE instance_id = ?",
        )
        .bind(request.member.instance_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = current {
            let member = &request.member;
            if row.get::<i64, _>("heartbeat_sequence")
                >= i64::try_from(member.heartbeat_sequence).unwrap_or(i64::MAX)
                || row.get::<i64, _>("observed_at") > member.observed_at
                || row.get::<i64, _>("started_at") > member.started_at
            {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "control-plane heartbeat sequence or time did not advance".to_string(),
                ));
            }
            if row.get::<String, _>("region") != member.region
                || row.get::<String, _>("zone") != member.zone
                || row.get::<String, _>("endpoint_sha256") != member.endpoint_sha256
            {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "control-plane instance identity changed region, zone, or endpoint".to_string(),
                ));
            }
        }
        let member = &request.member;
        sqlx::query(
            "INSERT INTO enterprise_control_plane_members \
             (instance_id, heartbeat_sequence, region, zone, daemon_version, endpoint_sha256, \
              status, started_at, observed_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(instance_id) DO UPDATE SET \
              heartbeat_sequence = excluded.heartbeat_sequence, region = excluded.region, \
              zone = excluded.zone, daemon_version = excluded.daemon_version, \
              endpoint_sha256 = excluded.endpoint_sha256, status = excluded.status, \
              started_at = excluded.started_at, observed_at = excluded.observed_at, \
              updated_at = excluded.updated_at",
        )
        .bind(member.instance_id.as_str())
        .bind(u64_to_i64(member.heartbeat_sequence, "heartbeat sequence")?)
        .bind(member.region.trim())
        .bind(member.zone.trim())
        .bind(member.daemon_version.trim())
        .bind(&member.endpoint_sha256)
        .bind(member.status.as_str())
        .bind(member.started_at)
        .bind(member.observed_at)
        .bind(member.observed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        insert_receipt(
            &mut connection,
            "control_plane.heartbeat",
            request.idempotency_key.trim(),
            &request_sha,
            member,
            member.observed_at,
        )
        .await?;
        commit(&mut connection).await?;
        Ok(member.clone())
    }

    async fn acquire_leadership(
        &self,
        request: &ControlPlaneLeadershipRequest,
    ) -> Result<ControlPlaneLeadershipLease, EnterpriseScaleError> {
        if !valid_typed_id(request.instance_id.as_str(), "ci_") {
            return Err(EnterpriseScaleError::Invalid(
                "control-plane instance id is invalid".to_string(),
            ));
        }
        validate_leadership_window(
            &request.idempotency_key,
            request.observed_at,
            request.expires_at,
        )?;
        let request_sha = canonical_sha256(request)?;
        let mut connection = begin_immediate(&self.pool).await?;
        if let Some(replayed) = replay_receipt(
            &mut connection,
            "leadership.acquire",
            request.idempotency_key.trim(),
            &request_sha,
        )
        .await?
        {
            commit(&mut connection).await?;
            return Ok(replayed);
        }
        let member = sqlx::query(
            "SELECT status, observed_at FROM enterprise_control_plane_members WHERE instance_id = ?",
        )
        .bind(request.instance_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| EnterpriseScaleError::Denied("control-plane member is unknown".to_string()))?;
        let member_observed = member.get::<i64, _>("observed_at");
        if member.get::<String, _>("status") != "ready"
            || member_observed > request.observed_at
            || request.observed_at.saturating_sub(member_observed) > 30
        {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Denied(
                "control-plane member is not recently ready".to_string(),
            ));
        }
        let current =
            sqlx::query("SELECT * FROM enterprise_control_plane_leadership WHERE singleton = 1")
                .fetch_optional(&mut *connection)
                .await
                .map_err(storage_error)?;
        let lease = match current {
            Some(row)
                if row.get::<i64, _>("expires_at") > request.observed_at
                    && row.get::<String, _>("instance_id") == request.instance_id.as_str() =>
            {
                leadership_from_row(&row)?
            }
            Some(row) if row.get::<i64, _>("expires_at") > request.observed_at => {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Denied(
                    "another control-plane instance holds the leadership lease".to_string(),
                ));
            }
            Some(row) => ControlPlaneLeadershipLease {
                instance_id: request.instance_id.clone(),
                term: i64_to_u64(row.get("term"), "leadership term")?.saturating_add(1),
                fencing_token: i64_to_u64(row.get("fencing_token"), "leadership fence")?
                    .saturating_add(1),
                acquired_at: request.observed_at,
                renewed_at: request.observed_at,
                expires_at: request.expires_at,
            },
            None => ControlPlaneLeadershipLease {
                instance_id: request.instance_id.clone(),
                term: 1,
                fencing_token: 1,
                acquired_at: request.observed_at,
                renewed_at: request.observed_at,
                expires_at: request.expires_at,
            },
        };
        sqlx::query(
            "INSERT INTO enterprise_control_plane_leadership \
             (singleton, instance_id, term, fencing_token, acquired_at, renewed_at, expires_at) \
             VALUES (1, ?, ?, ?, ?, ?, ?) ON CONFLICT(singleton) DO UPDATE SET \
             instance_id = excluded.instance_id, term = excluded.term, \
             fencing_token = excluded.fencing_token, acquired_at = excluded.acquired_at, \
             renewed_at = excluded.renewed_at, expires_at = excluded.expires_at",
        )
        .bind(lease.instance_id.as_str())
        .bind(u64_to_i64(lease.term, "leadership term")?)
        .bind(u64_to_i64(lease.fencing_token, "leadership fencing token")?)
        .bind(lease.acquired_at)
        .bind(lease.renewed_at)
        .bind(lease.expires_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        insert_receipt(
            &mut connection,
            "leadership.acquire",
            request.idempotency_key.trim(),
            &request_sha,
            &lease,
            request.observed_at,
        )
        .await?;
        commit(&mut connection).await?;
        Ok(lease)
    }

    async fn renew_leadership(
        &self,
        request: &ControlPlaneLeadershipRenewal,
    ) -> Result<ControlPlaneLeadershipLease, EnterpriseScaleError> {
        if !valid_typed_id(request.instance_id.as_str(), "ci_") {
            return Err(EnterpriseScaleError::Invalid(
                "control-plane instance id is invalid".to_string(),
            ));
        }
        validate_leadership_window(
            &request.idempotency_key,
            request.observed_at,
            request.expires_at,
        )?;
        if request.term == 0 || request.fencing_token == 0 {
            return Err(EnterpriseScaleError::Invalid(
                "leadership term and fencing token must be positive".to_string(),
            ));
        }
        let request_sha = canonical_sha256(request)?;
        let mut connection = begin_immediate(&self.pool).await?;
        if let Some(replayed) = replay_receipt(
            &mut connection,
            "leadership.renew",
            request.idempotency_key.trim(),
            &request_sha,
        )
        .await?
        {
            commit(&mut connection).await?;
            return Ok(replayed);
        }
        let row =
            sqlx::query("SELECT * FROM enterprise_control_plane_leadership WHERE singleton = 1")
                .fetch_optional(&mut *connection)
                .await
                .map_err(storage_error)?
                .ok_or_else(|| {
                    EnterpriseScaleError::Denied("leadership lease does not exist".to_string())
                })?;
        let current = leadership_from_row(&row)?;
        if current.instance_id != request.instance_id
            || current.term != request.term
            || current.fencing_token != request.fencing_token
            || current.expires_at <= request.observed_at
            || request.observed_at < current.renewed_at
        {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Denied(
                "stale leadership term, fence, owner, or expiry".to_string(),
            ));
        }
        let lease = ControlPlaneLeadershipLease {
            renewed_at: request.observed_at,
            expires_at: request.expires_at,
            ..current
        };
        sqlx::query(
            "UPDATE enterprise_control_plane_leadership SET renewed_at = ?, expires_at = ? \
             WHERE singleton = 1 AND instance_id = ? AND term = ? AND fencing_token = ?",
        )
        .bind(lease.renewed_at)
        .bind(lease.expires_at)
        .bind(lease.instance_id.as_str())
        .bind(u64_to_i64(lease.term, "leadership term")?)
        .bind(u64_to_i64(lease.fencing_token, "leadership fencing token")?)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        insert_receipt(
            &mut connection,
            "leadership.renew",
            request.idempotency_key.trim(),
            &request_sha,
            &lease,
            request.observed_at,
        )
        .await?;
        commit(&mut connection).await?;
        Ok(lease)
    }

    async fn declare_worker_image_rollout(
        &self,
        fence: &EnterpriseMutationFence,
        rollout: &WorkerImageRollout,
    ) -> Result<WorkerImageRollout, EnterpriseScaleError> {
        validate_rollout(rollout)?;
        let declaration_sha = canonical_sha256(rollout)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "rollout.declare",
            rollout.rollout_id.as_str(),
            &declaration_sha,
        )
        .await?;
        let existing = sqlx::query(
            "SELECT declaration_sha256 FROM enterprise_worker_image_rollouts WHERE rollout_id = ?",
        )
        .bind(rollout.rollout_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = existing {
            if row.get::<String, _>("declaration_sha256") != declaration_sha {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "rollout id was reused with a different declaration".to_string(),
                ));
            }
            commit(&mut connection).await?;
            return load_rollout(&self.pool, &rollout.rollout_id).await;
        }
        sqlx::query(
            "INSERT INTO enterprise_worker_image_rollouts \
             (rollout_id, image_digest, signature_bundle_sha256, policy_sha256, \
              required_zones_json, declaration_sha256, status, declared_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(rollout.rollout_id.as_str())
        .bind(&rollout.image_digest)
        .bind(&rollout.signature_bundle_sha256)
        .bind(&rollout.policy_sha256)
        .bind(json(&rollout.required_zones)?)
        .bind(&declaration_sha)
        .bind(rollout.status.as_str())
        .bind(rollout.declared_at)
        .bind(rollout.updated_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(rollout.clone())
    }

    async fn observe_worker_image_zone(
        &self,
        fence: &EnterpriseMutationFence,
        observation: &WorkerImageZoneObservation,
    ) -> Result<WorkerImageRollout, EnterpriseScaleError> {
        if !valid_typed_id(observation.rollout_id.as_str(), "ir_")
            || !nonempty(&observation.zone)
            || !valid_image_digest(&observation.observed_image_digest)
            || observation.desired_workers == 0
            || observation.ready_workers > observation.desired_workers
            || observation.observed_at < 0
        {
            return Err(EnterpriseScaleError::Invalid(
                "worker image zone observation is invalid".to_string(),
            ));
        }
        let observation_sha = canonical_sha256(observation)?;
        let resource_key = format!("{}:{}", observation.rollout_id, observation.zone.trim());
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "rollout.observe_zone",
            &resource_key,
            &observation_sha,
        )
        .await?;
        let rollout = load_rollout_in_transaction(&mut connection, &observation.rollout_id).await?;
        if !rollout.required_zones.contains(observation.zone.trim()) {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Denied(
                "zone is not required by the rollout".to_string(),
            ));
        }
        let historical_replay = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM enterprise_worker_image_zone_observation_history \
             WHERE rollout_id = ? AND zone = ? AND observation_sha256 = ?",
        )
        .bind(observation.rollout_id.as_str())
        .bind(observation.zone.trim())
        .bind(&observation_sha)
        .fetch_one(&mut *connection)
        .await
        .map_err(storage_error)?
            == 1;
        if historical_replay {
            commit(&mut connection).await?;
            return Ok(rollout);
        }
        if rollout.status == WorkerImageRolloutStatus::RolledBack {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Denied(
                "rolled-back rollout cannot accept new zone observations".to_string(),
            ));
        }
        let existing = sqlx::query(
            "SELECT observed_at, observation_sha256 FROM enterprise_worker_image_zone_observations \
             WHERE rollout_id = ? AND zone = ?",
        )
        .bind(observation.rollout_id.as_str())
        .bind(observation.zone.trim())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = existing {
            let previous_at = row.get::<i64, _>("observed_at");
            let previous_sha = row.get::<String, _>("observation_sha256");
            if previous_sha == observation_sha {
                commit(&mut connection).await?;
                return load_rollout(&self.pool, &observation.rollout_id).await;
            }
            if previous_at >= observation.observed_at {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "zone rollout observation did not advance".to_string(),
                ));
            }
        }
        if observation.observed_at < rollout.updated_at {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Conflict(
                "zone rollout observation predates current rollout state".to_string(),
            ));
        }
        sqlx::query(
            "INSERT INTO enterprise_worker_image_zone_observations \
             (rollout_id, zone, observed_image_digest, signature_verified, ready_workers, \
              desired_workers, observed_at, observation_sha256) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(rollout_id, zone) DO UPDATE SET \
              observed_image_digest = excluded.observed_image_digest, \
              signature_verified = excluded.signature_verified, ready_workers = excluded.ready_workers, \
              desired_workers = excluded.desired_workers, observed_at = excluded.observed_at, \
              observation_sha256 = excluded.observation_sha256",
        )
        .bind(observation.rollout_id.as_str())
        .bind(observation.zone.trim())
        .bind(&observation.observed_image_digest)
        .bind(i64::from(observation.signature_verified))
        .bind(i64::from(observation.ready_workers))
        .bind(i64::from(observation.desired_workers))
        .bind(observation.observed_at)
        .bind(&observation_sha)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "INSERT INTO enterprise_worker_image_zone_observation_history \
             (rollout_id, zone, observed_image_digest, signature_verified, ready_workers, \
              desired_workers, observed_at, observation_sha256) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(observation.rollout_id.as_str())
        .bind(observation.zone.trim())
        .bind(&observation.observed_image_digest)
        .bind(i64::from(observation.signature_verified))
        .bind(i64::from(observation.ready_workers))
        .bind(i64::from(observation.desired_workers))
        .bind(observation.observed_at)
        .bind(&observation_sha)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        let rows = sqlx::query(
            "SELECT zone, observed_image_digest, signature_verified, ready_workers, desired_workers \
             FROM enterprise_worker_image_zone_observations WHERE rollout_id = ?",
        )
        .bind(observation.rollout_id.as_str())
        .fetch_all(&mut *connection)
        .await
        .map_err(storage_error)?;
        let by_zone = rows
            .iter()
            .map(|row| (row.get::<String, _>("zone"), row))
            .collect::<BTreeMap<_, _>>();
        let any_degraded = by_zone.values().any(|row| {
            row.get::<String, _>("observed_image_digest") != rollout.image_digest
                || row.get::<i64, _>("signature_verified") == 0
        });
        let healthy = rollout.required_zones.iter().all(|zone| {
            by_zone.get(zone).is_some_and(|row| {
                row.get::<String, _>("observed_image_digest") == rollout.image_digest
                    && row.get::<i64, _>("signature_verified") != 0
                    && row.get::<i64, _>("ready_workers") >= row.get::<i64, _>("desired_workers")
            })
        });
        let status = if healthy {
            WorkerImageRolloutStatus::Healthy
        } else if any_degraded {
            WorkerImageRolloutStatus::Degraded
        } else {
            WorkerImageRolloutStatus::Progressing
        };
        sqlx::query(
            "UPDATE enterprise_worker_image_rollouts SET status = ?, updated_at = ? WHERE rollout_id = ?",
        )
        .bind(status.as_str())
        .bind(observation.observed_at)
        .bind(observation.rollout_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        load_rollout(&self.pool, &observation.rollout_id).await
    }

    async fn rollback_worker_image_rollout(
        &self,
        fence: &EnterpriseMutationFence,
        rollback_request: &WorkerImageRollback,
    ) -> Result<WorkerImageRollout, EnterpriseScaleError> {
        if !valid_typed_id(rollback_request.rollout_id.as_str(), "ir_")
            || !valid_sha256(&rollback_request.reason_sha256)
            || rollback_request.rolled_back_at < 0
        {
            return Err(EnterpriseScaleError::Invalid(
                "worker image rollback is invalid".to_string(),
            ));
        }
        let rollback_sha = canonical_sha256(rollback_request)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "rollout.rollback",
            rollback_request.rollout_id.as_str(),
            &rollback_sha,
        )
        .await?;
        let current =
            load_rollout_in_transaction(&mut connection, &rollback_request.rollout_id).await?;
        let previous = sqlx::query(
            "SELECT rollback_sha256 FROM enterprise_worker_image_rollout_rollbacks \
             WHERE rollout_id = ?",
        )
        .bind(rollback_request.rollout_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = previous {
            if row.get::<String, _>("rollback_sha256") == rollback_sha {
                commit(&mut connection).await?;
                return Ok(current);
            }
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Conflict(
                "worker image rollout was already rolled back with different evidence".to_string(),
            ));
        }
        if current.status == WorkerImageRolloutStatus::RolledBack
            || rollback_request.rolled_back_at < current.updated_at
        {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Conflict(
                "worker image rollback is stale or conflicts with durable state".to_string(),
            ));
        }
        let updated = sqlx::query(
            "UPDATE enterprise_worker_image_rollouts SET status = 'rolled_back', updated_at = ? \
             WHERE rollout_id = ? AND status <> 'rolled_back'",
        )
        .bind(rollback_request.rolled_back_at)
        .bind(rollback_request.rollout_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        if updated.rows_affected() != 1 {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Conflict(
                "worker image rollout changed during rollback".to_string(),
            ));
        }
        sqlx::query(
            "INSERT INTO enterprise_worker_image_rollout_rollbacks \
             (rollout_id, previous_status, reason_sha256, rollback_sha256, rolled_back_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(rollback_request.rollout_id.as_str())
        .bind(current.status.as_str())
        .bind(&rollback_request.reason_sha256)
        .bind(&rollback_sha)
        .bind(rollback_request.rolled_back_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "UPDATE enterprise_worker_availability \
             SET worker_status = 'offline', available_slots = 0, observed_at = ?, updated_at = ? \
             WHERE worker_id IN ( \
               SELECT id FROM workers \
               WHERE CASE WHEN json_valid(labels_json) \
                 THEN json_extract(labels_json, '$.agentd_attestation.rollout_id') END = ? \
             )",
        )
        .bind(rollback_request.rolled_back_at)
        .bind(rollback_request.rolled_back_at)
        .bind(rollback_request.rollout_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "UPDATE workers SET status = 'offline', record_version = record_version + 1, \
                    updated_at = ? \
             WHERE status <> 'retired' \
               AND CASE WHEN json_valid(labels_json) \
                 THEN json_extract(labels_json, '$.agentd_attestation.rollout_id') END = ?",
        )
        .bind(rollback_request.rolled_back_at)
        .bind(rollback_request.rollout_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        load_rollout(&self.pool, &rollback_request.rollout_id).await
    }

    async fn upsert_zone_pool(
        &self,
        fence: &EnterpriseMutationFence,
        policy: &ZonePoolPolicy,
    ) -> Result<ZonePoolPolicy, EnterpriseScaleError> {
        validate_zone_policy(policy)?;
        let mutation_sha = canonical_sha256(policy)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "zone_pool.upsert",
            policy.pool_id.as_str(),
            &mutation_sha,
        )
        .await?;
        let rollout = load_rollout_in_transaction(&mut connection, &policy.rollout_id).await?;
        let existing = sqlx::query("SELECT * FROM enterprise_zone_pool_policies WHERE pool_id = ?")
            .bind(policy.pool_id.as_str())
            .fetch_optional(&mut *connection)
            .await
            .map_err(storage_error)?;
        if let Some(row) = existing {
            let current = zone_policy_from_row(&row)?;
            if current.region != policy.region
                || current.zone != policy.zone
                || current.resource_class != policy.resource_class
                || current.trust_domain != policy.trust_domain
            {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "zone pool identity or version conflicts with durable state".to_string(),
                ));
            }
            if current == *policy {
                commit(&mut connection).await?;
                return Ok(current);
            }
            let historical = sqlx::query(
                "SELECT * FROM enterprise_zone_pool_policy_versions \
                 WHERE pool_id = ? AND policy_sha256 = ?",
            )
            .bind(policy.pool_id.as_str())
            .bind(&policy.policy_sha256)
            .fetch_optional(&mut *connection)
            .await
            .map_err(storage_error)?;
            if let Some(row) = historical {
                if zone_policy_from_row(&row)? == *policy {
                    commit(&mut connection).await?;
                    return Ok(current);
                }
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "zone pool policy version was reused with different content".to_string(),
                ));
            }
            if current.updated_at >= policy.updated_at {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "zone pool policy version did not advance".to_string(),
                ));
            }
        }
        if !rollout.required_zones.contains(policy.zone.trim())
            || rollout.status == WorkerImageRolloutStatus::RolledBack
        {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Denied(
                "zone pool must reference an active rollout that includes its zone".to_string(),
            ));
        }
        if policy.updated_at < rollout.declared_at {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Invalid(
                "zone pool policy predates its rollout".to_string(),
            ));
        }
        sqlx::query(
            "INSERT INTO enterprise_zone_pool_policies \
             (pool_id, region, zone, resource_class, trust_domain, rollout_id, minimum_replicas, \
              maximum_replicas, target_queue_per_slot, scale_down_cooldown_seconds, enabled, \
              policy_sha256, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(pool_id) DO UPDATE SET rollout_id = excluded.rollout_id, \
              minimum_replicas = excluded.minimum_replicas, maximum_replicas = excluded.maximum_replicas, \
              target_queue_per_slot = excluded.target_queue_per_slot, \
              scale_down_cooldown_seconds = excluded.scale_down_cooldown_seconds, \
              enabled = excluded.enabled, policy_sha256 = excluded.policy_sha256, \
              updated_at = excluded.updated_at",
        )
        .bind(policy.pool_id.as_str())
        .bind(policy.region.trim())
        .bind(policy.zone.trim())
        .bind(policy.resource_class.trim())
        .bind(policy.trust_domain.trim())
        .bind(policy.rollout_id.as_str())
        .bind(i64::from(policy.minimum_replicas))
        .bind(i64::from(policy.maximum_replicas))
        .bind(i64::from(policy.target_queue_per_slot))
        .bind(i64::from(policy.scale_down_cooldown_seconds))
        .bind(i64::from(policy.enabled))
        .bind(&policy.policy_sha256)
        .bind(policy.updated_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "INSERT INTO enterprise_zone_pool_policy_versions \
             (pool_id, region, zone, resource_class, trust_domain, rollout_id, \
              minimum_replicas, maximum_replicas, target_queue_per_slot, \
              scale_down_cooldown_seconds, enabled, policy_sha256, \
              policy_record_sha256, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(policy.pool_id.as_str())
        .bind(policy.region.trim())
        .bind(policy.zone.trim())
        .bind(policy.resource_class.trim())
        .bind(policy.trust_domain.trim())
        .bind(policy.rollout_id.as_str())
        .bind(i64::from(policy.minimum_replicas))
        .bind(i64::from(policy.maximum_replicas))
        .bind(i64::from(policy.target_queue_per_slot))
        .bind(i64::from(policy.scale_down_cooldown_seconds))
        .bind(i64::from(policy.enabled))
        .bind(&policy.policy_sha256)
        .bind(&mutation_sha)
        .bind(policy.updated_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(policy.clone())
    }

    async fn recommend_capacity(
        &self,
        fence: &EnterpriseMutationFence,
        observation: &CapacityObservation,
    ) -> Result<AutoscalingRecommendation, EnterpriseScaleError> {
        if !valid_typed_id(observation.pool_id.as_str(), "zp_")
            || observation.observed_at < 0
            || observation.available_slots > observation.total_slots
            || observation
                .last_scale_at
                .is_some_and(|time| time > observation.observed_at)
        {
            return Err(EnterpriseScaleError::Invalid(
                "capacity observation is invalid".to_string(),
            ));
        }
        let observation_sha = canonical_sha256(observation)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "capacity.recommend",
            observation.pool_id.as_str(),
            &observation_sha,
        )
        .await?;
        let policy = load_zone_policy_in_transaction(&mut connection, &observation.pool_id).await?;
        let rollout = load_rollout_in_transaction(&mut connection, &policy.rollout_id).await?;
        if rollout.status == WorkerImageRolloutStatus::RolledBack {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Denied(
                "capacity cannot be recommended for a rolled-back image".to_string(),
            ));
        }
        if observation.observed_at < policy.updated_at
            || observation.observed_at < rollout.updated_at
        {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Conflict(
                "capacity observation predates current rollout or zone policy".to_string(),
            ));
        }
        let (mut desired, mut reason) = if policy.enabled {
            let slots_per_replica = if observation.ready_replicas == 0 {
                1
            } else {
                observation
                    .total_slots
                    .checked_div(observation.ready_replicas)
                    .unwrap_or(1)
                    .max(1)
            };
            let target = u64::from(policy.target_queue_per_slot);
            let queued_slots = observation.queue_depth.saturating_add(target - 1) / target;
            let demanded_slots = queued_slots.saturating_add(observation.running_tasks);
            let replicas = demanded_slots.saturating_add(u64::from(slots_per_replica) - 1)
                / u64::from(slots_per_replica);
            let replicas = u32::try_from(replicas)
                .unwrap_or(u32::MAX)
                .clamp(policy.minimum_replicas, policy.maximum_replicas);
            let reason = match replicas.cmp(&observation.ready_replicas) {
                Ordering::Greater => "queue_pressure",
                Ordering::Less => "excess_capacity",
                Ordering::Equal => "capacity_balanced",
            };
            (replicas, reason)
        } else {
            (0, "pool_disabled")
        };
        if desired < observation.ready_replicas
            && observation.last_scale_at.is_some_and(|last| {
                observation.observed_at.saturating_sub(last)
                    < i64::from(policy.scale_down_cooldown_seconds)
            })
        {
            desired = observation.ready_replicas;
            reason = "scale_down_cooldown";
        }
        let digest_input = (observation, &policy.policy_sha256, desired, reason);
        let recommendation = AutoscalingRecommendation {
            pool_id: observation.pool_id.clone(),
            current_replicas: observation.ready_replicas,
            desired_replicas: desired,
            queue_depth: observation.queue_depth,
            reason_code: reason.to_string(),
            recommendation_sha256: canonical_sha256(&digest_input)?,
            observed_at: observation.observed_at,
        };
        sqlx::query(
            "INSERT OR IGNORE INTO enterprise_autoscaling_recommendations \
             (pool_id, current_replicas, desired_replicas, queue_depth, running_tasks, total_slots, \
              available_slots, reason_code, recommendation_sha256, observed_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(recommendation.pool_id.as_str())
        .bind(i64::from(recommendation.current_replicas))
        .bind(i64::from(recommendation.desired_replicas))
        .bind(u64_to_i64(recommendation.queue_depth, "queue depth")?)
        .bind(u64_to_i64(observation.running_tasks, "running tasks")?)
        .bind(i64::from(observation.total_slots))
        .bind(i64::from(observation.available_slots))
        .bind(&recommendation.reason_code)
        .bind(&recommendation.recommendation_sha256)
        .bind(recommendation.observed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(recommendation)
    }

    async fn create_replication_plan(
        &self,
        fence: &EnterpriseMutationFence,
        plan: &ArtifactReplicationPlan,
    ) -> Result<ArtifactReplicationPlan, EnterpriseScaleError> {
        validate_replication_plan(plan)?;
        let plan_sha = canonical_sha256(plan)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "replication.create_plan",
            plan.replication_id.as_str(),
            &plan_sha,
        )
        .await?;
        let artifact_sha256: Option<String> =
            sqlx::query_scalar("SELECT content_sha256 FROM execution_artifacts WHERE id = ?")
                .bind(plan.execution_artifact_id.as_str())
                .fetch_optional(&mut *connection)
                .await
                .map_err(storage_error)?;
        match artifact_sha256 {
            Some(digest) if digest == plan.artifact_sha256 => {}
            Some(_) => {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Denied(
                    "replication plan digest does not match the execution artifact".to_string(),
                ));
            }
            None => {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::NotFound(
                    "execution artifact for replication plan".to_string(),
                ));
            }
        }
        let existing = sqlx::query(
            "SELECT plan_sha256 FROM enterprise_artifact_replication_plans WHERE replication_id = ?",
        )
        .bind(plan.replication_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = existing {
            if row.get::<String, _>("plan_sha256") != plan_sha {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "replication id was reused with a different plan".to_string(),
                ));
            }
            commit(&mut connection).await?;
            return Ok(plan.clone());
        }
        sqlx::query(
            "INSERT INTO enterprise_artifact_replication_plans \
             (replication_id, execution_artifact_id, tenant_scope_sha256, artifact_sha256, \
              source_region, required_regions_json, plan_sha256, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(plan.replication_id.as_str())
        .bind(plan.execution_artifact_id.as_str())
        .bind(&plan.tenant_scope_sha256)
        .bind(&plan.artifact_sha256)
        .bind(plan.source_region.trim())
        .bind(json(&plan.required_regions)?)
        .bind(&plan_sha)
        .bind(plan.created_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(plan.clone())
    }

    async fn acknowledge_artifact_replica(
        &self,
        fence: &EnterpriseMutationFence,
        acknowledgement: &ArtifactReplicaAcknowledgement,
    ) -> Result<ArtifactReplicaAcknowledgement, EnterpriseScaleError> {
        if !valid_typed_id(acknowledgement.replication_id.as_str(), "rp_")
            || !valid_typed_id(acknowledgement.tenant_key_id.as_str(), "tk_")
            || !nonempty(&acknowledgement.region)
            || !valid_sha256(&acknowledgement.artifact_sha256)
            || !valid_sha256(&acknowledgement.object_ref_sha256)
            || acknowledgement.acknowledged_at < 0
        {
            return Err(EnterpriseScaleError::Invalid(
                "artifact replica acknowledgement is invalid".to_string(),
            ));
        }
        let acknowledgement_sha = canonical_sha256(acknowledgement)?;
        let resource_key = format!(
            "{}:{}",
            acknowledgement.replication_id,
            acknowledgement.region.trim()
        );
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "replication.acknowledge",
            &resource_key,
            &acknowledgement_sha,
        )
        .await?;
        let plan_row = sqlx::query(
            "SELECT tenant_scope_sha256, artifact_sha256, required_regions_json \
             FROM enterprise_artifact_replication_plans WHERE replication_id = ?",
        )
        .bind(acknowledgement.replication_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| EnterpriseScaleError::NotFound("artifact replication plan".to_string()))?;
        let required_regions: BTreeSet<String> =
            parse_json(&plan_row.get::<String, _>("required_regions_json"))?;
        if plan_row.get::<String, _>("artifact_sha256") != acknowledgement.artifact_sha256
            || !required_regions.contains(acknowledgement.region.trim())
        {
            return Err(EnterpriseScaleError::Denied(
                "replica digest or region does not match its immutable plan".to_string(),
            ));
        }
        let existing = sqlx::query(
            "SELECT * \
             FROM enterprise_artifact_replica_acknowledgements \
             WHERE replication_id = ? AND region = ?",
        )
        .bind(acknowledgement.replication_id.as_str())
        .bind(acknowledgement.region.trim())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = existing.as_ref() {
            let current = artifact_replica_from_row(row)?;
            if row.get::<String, _>("acknowledgement_sha256") == acknowledgement_sha {
                commit(&mut connection).await?;
                return Ok(current);
            }
            let historical_replay = sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM enterprise_artifact_replica_transitions \
                 WHERE replication_id = ? AND region = ? AND acknowledgement_sha256 = ?",
            )
            .bind(acknowledgement.replication_id.as_str())
            .bind(acknowledgement.region.trim())
            .bind(&acknowledgement_sha)
            .fetch_one(&mut *connection)
            .await
            .map_err(storage_error)?
                == 1;
            if historical_replay {
                commit(&mut connection).await?;
                return Ok(current);
            }
        }
        let key_row = sqlx::query(
            "SELECT tenant_scope_sha256, region, status FROM enterprise_tenant_keys WHERE tenant_key_id = ?",
        )
        .bind(acknowledgement.tenant_key_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| EnterpriseScaleError::NotFound("tenant key version".to_string()))?;
        if key_row.get::<String, _>("tenant_scope_sha256")
            != plan_row.get::<String, _>("tenant_scope_sha256")
            || key_row.get::<String, _>("region") != acknowledgement.region
            || key_row.get::<String, _>("status") != "active"
        {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Denied(
                "replica is not encrypted with the active tenant key for its region".to_string(),
            ));
        }
        if let Some(row) = existing {
            let previous_status = ReplicaStatus::try_from(row.get::<String, _>("status").as_str())
                .map_err(|error| EnterpriseScaleError::Unavailable(error.to_string()))?;
            if !replica_transition_allowed(previous_status, acknowledgement.status)
                || row.get::<i64, _>("acknowledged_at") >= acknowledgement.acknowledged_at
            {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "replica acknowledgement transition is stale or invalid".to_string(),
                ));
            }
            sqlx::query(
                "UPDATE enterprise_artifact_replica_acknowledgements SET \
                 object_ref_sha256 = ?, tenant_key_id = ?, status = ?, \
                 acknowledgement_sha256 = ?, acknowledged_at = ? \
                 WHERE replication_id = ? AND region = ?",
            )
            .bind(&acknowledgement.object_ref_sha256)
            .bind(acknowledgement.tenant_key_id.as_str())
            .bind(acknowledgement.status.as_str())
            .bind(&acknowledgement_sha)
            .bind(acknowledgement.acknowledged_at)
            .bind(acknowledgement.replication_id.as_str())
            .bind(acknowledgement.region.trim())
            .execute(&mut *connection)
            .await
            .map_err(storage_error)?;
            sqlx::query(
                "INSERT INTO enterprise_artifact_replica_transitions \
                 (replication_id, region, previous_status, target_status, \
                  acknowledgement_sha256, acknowledged_at) VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(acknowledgement.replication_id.as_str())
            .bind(acknowledgement.region.trim())
            .bind(previous_status.as_str())
            .bind(acknowledgement.status.as_str())
            .bind(&acknowledgement_sha)
            .bind(acknowledgement.acknowledged_at)
            .execute(&mut *connection)
            .await
            .map_err(storage_error)?;
            commit(&mut connection).await?;
            return Ok(acknowledgement.clone());
        }
        sqlx::query(
            "INSERT INTO enterprise_artifact_replica_acknowledgements \
             (replication_id, region, artifact_sha256, object_ref_sha256, tenant_key_id, \
              status, acknowledgement_sha256, acknowledged_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(acknowledgement.replication_id.as_str())
        .bind(acknowledgement.region.trim())
        .bind(&acknowledgement.artifact_sha256)
        .bind(&acknowledgement.object_ref_sha256)
        .bind(acknowledgement.tenant_key_id.as_str())
        .bind(acknowledgement.status.as_str())
        .bind(&acknowledgement_sha)
        .bind(acknowledgement.acknowledged_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "INSERT INTO enterprise_artifact_replica_transitions \
             (replication_id, region, previous_status, target_status, \
              acknowledgement_sha256, acknowledged_at) VALUES (?, ?, NULL, ?, ?, ?)",
        )
        .bind(acknowledgement.replication_id.as_str())
        .bind(acknowledgement.region.trim())
        .bind(acknowledgement.status.as_str())
        .bind(&acknowledgement_sha)
        .bind(acknowledgement.acknowledged_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(acknowledgement.clone())
    }

    async fn register_tenant_key(
        &self,
        fence: &EnterpriseMutationFence,
        key: &TenantKeyVersion,
    ) -> Result<TenantKeyVersion, EnterpriseScaleError> {
        validate_tenant_key(key)?;
        let registration_sha = canonical_sha256(key)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "tenant_key.register",
            key.tenant_key_id.as_str(),
            &registration_sha,
        )
        .await?;
        let existing = sqlx::query(
            "SELECT registration_sha256 FROM enterprise_tenant_keys WHERE tenant_key_id = ?",
        )
        .bind(key.tenant_key_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = existing {
            if row.get::<String, _>("registration_sha256") != registration_sha {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "tenant key id was reused with different metadata".to_string(),
                ));
            }
            commit(&mut connection).await?;
            return load_tenant_key(&self.pool, &key.tenant_key_id).await;
        }
        if key.status == TenantKeyStatus::Active {
            let active = sqlx::query_scalar::<_, String>(
                "SELECT tenant_key_id FROM enterprise_tenant_keys \
                 WHERE tenant_scope_sha256 = ? AND region = ? AND status = 'active'",
            )
            .bind(&key.tenant_scope_sha256)
            .bind(key.region.trim())
            .fetch_optional(&mut *connection)
            .await
            .map_err(storage_error)?;
            if let Some(active) = active {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(format!(
                    "tenant region already has active key {active}"
                )));
            }
        }
        sqlx::query(
            "INSERT INTO enterprise_tenant_keys \
             (tenant_key_id, tenant_scope_sha256, region, kms_key_ref_sha256, \
              key_version_ref_sha256, status, registration_sha256, activated_at, retired_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(key.tenant_key_id.as_str())
        .bind(&key.tenant_scope_sha256)
        .bind(key.region.trim())
        .bind(&key.kms_key_ref_sha256)
        .bind(&key.key_version_ref_sha256)
        .bind(key.status.as_str())
        .bind(&registration_sha)
        .bind(key.activated_at)
        .bind(key.retired_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(key.clone())
    }

    async fn transition_tenant_key(
        &self,
        fence: &EnterpriseMutationFence,
        transition: &TenantKeyTransition,
    ) -> Result<TenantKeyVersion, EnterpriseScaleError> {
        if !valid_typed_id(transition.tenant_key_id.as_str(), "tk_")
            || transition.transitioned_at < 0
            || transition.target_status == TenantKeyStatus::Active
        {
            return Err(EnterpriseScaleError::Invalid(
                "tenant key transition is invalid".to_string(),
            ));
        }
        let transition_sha = canonical_sha256(transition)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "tenant_key.transition",
            transition.tenant_key_id.as_str(),
            &transition_sha,
        )
        .await?;
        let row = sqlx::query("SELECT * FROM enterprise_tenant_keys WHERE tenant_key_id = ?")
            .bind(transition.tenant_key_id.as_str())
            .fetch_optional(&mut *connection)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| EnterpriseScaleError::NotFound("tenant key version".to_string()))?;
        let current = tenant_key_from_row(&row)?;
        let historical_replay = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM enterprise_tenant_key_transitions \
             WHERE tenant_key_id = ? AND target_status = ? AND transition_sha256 = ?",
        )
        .bind(transition.tenant_key_id.as_str())
        .bind(transition.target_status.as_str())
        .bind(&transition_sha)
        .fetch_one(&mut *connection)
        .await
        .map_err(storage_error)?
            == 1;
        if historical_replay {
            commit(&mut connection).await?;
            return Ok(current);
        }
        if current.status == transition.target_status {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Conflict(
                "tenant key transition replay changed input".to_string(),
            ));
        }
        let allowed = matches!(
            (current.status, transition.target_status),
            (TenantKeyStatus::Active, TenantKeyStatus::Retiring)
                | (TenantKeyStatus::Retiring, TenantKeyStatus::Retired)
        );
        if !allowed
            || transition.transitioned_at < current.activated_at
            || current.retired_at.is_some()
        {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Conflict(
                "tenant key status transition is not allowed".to_string(),
            ));
        }
        sqlx::query(
            "UPDATE enterprise_tenant_keys SET status = ?, retired_at = ? \
             WHERE tenant_key_id = ? AND status = ?",
        )
        .bind(transition.target_status.as_str())
        .bind(
            (transition.target_status == TenantKeyStatus::Retired)
                .then_some(transition.transitioned_at),
        )
        .bind(transition.tenant_key_id.as_str())
        .bind(current.status.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "INSERT INTO enterprise_tenant_key_transitions \
             (tenant_key_id, previous_status, target_status, transition_sha256, transitioned_at) \
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(transition.tenant_key_id.as_str())
        .bind(current.status.as_str())
        .bind(transition.target_status.as_str())
        .bind(&transition_sha)
        .bind(transition.transitioned_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        load_tenant_key(&self.pool, &transition.tenant_key_id).await
    }

    async fn set_retention_policy(
        &self,
        fence: &EnterpriseMutationFence,
        policy: &RetentionPolicy,
    ) -> Result<RetentionPolicy, EnterpriseScaleError> {
        validate_retention_policy(policy)?;
        let mutation_sha = canonical_sha256(policy)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "retention.set_policy",
            &policy.tenant_scope_sha256,
            &mutation_sha,
        )
        .await?;
        let existing = sqlx::query(
            "SELECT * FROM enterprise_retention_policies WHERE tenant_scope_sha256 = ?",
        )
        .bind(&policy.tenant_scope_sha256)
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = existing {
            let current = retention_policy_from_row(&row)?;
            if current == *policy {
                commit(&mut connection).await?;
                return Ok(current);
            }
            let historical = sqlx::query(
                "SELECT * FROM enterprise_retention_policy_versions \
                 WHERE tenant_scope_sha256 = ? AND policy_version_sha256 = ?",
            )
            .bind(&policy.tenant_scope_sha256)
            .bind(&policy.policy_version_sha256)
            .fetch_optional(&mut *connection)
            .await
            .map_err(storage_error)?;
            if let Some(row) = historical {
                if retention_policy_from_row(&row)? == *policy {
                    commit(&mut connection).await?;
                    return Ok(current);
                }
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "retention policy version was reused with different content".to_string(),
                ));
            }
            if current.updated_at >= policy.updated_at {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "retention policy version did not advance".to_string(),
                ));
            }
        }
        sqlx::query(
            "INSERT INTO enterprise_retention_policies \
             (tenant_scope_sha256, policy_version_sha256, artifact_retention_seconds, \
              transcript_retention_seconds, audit_retention_seconds, minimum_replica_regions, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT(tenant_scope_sha256) DO UPDATE SET \
              policy_version_sha256 = excluded.policy_version_sha256, \
              artifact_retention_seconds = excluded.artifact_retention_seconds, \
              transcript_retention_seconds = excluded.transcript_retention_seconds, \
              audit_retention_seconds = excluded.audit_retention_seconds, \
              minimum_replica_regions = excluded.minimum_replica_regions, updated_at = excluded.updated_at",
        )
        .bind(&policy.tenant_scope_sha256)
        .bind(&policy.policy_version_sha256)
        .bind(u64_to_i64(policy.artifact_retention_seconds, "artifact retention")?)
        .bind(u64_to_i64(policy.transcript_retention_seconds, "transcript retention")?)
        .bind(u64_to_i64(policy.audit_retention_seconds, "audit retention")?)
        .bind(i64::from(policy.minimum_replica_regions))
        .bind(policy.updated_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "INSERT INTO enterprise_retention_policy_versions \
             (tenant_scope_sha256, policy_version_sha256, policy_record_sha256, \
              artifact_retention_seconds, transcript_retention_seconds, \
              audit_retention_seconds, minimum_replica_regions, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&policy.tenant_scope_sha256)
        .bind(&policy.policy_version_sha256)
        .bind(&mutation_sha)
        .bind(u64_to_i64(
            policy.artifact_retention_seconds,
            "artifact retention",
        )?)
        .bind(u64_to_i64(
            policy.transcript_retention_seconds,
            "transcript retention",
        )?)
        .bind(u64_to_i64(
            policy.audit_retention_seconds,
            "audit retention",
        )?)
        .bind(i64::from(policy.minimum_replica_regions))
        .bind(policy.updated_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(policy.clone())
    }

    async fn place_legal_hold(
        &self,
        fence: &EnterpriseMutationFence,
        hold: &LegalHold,
    ) -> Result<LegalHold, EnterpriseScaleError> {
        validate_legal_hold(hold)?;
        let hold_sha = canonical_sha256(hold)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "legal_hold.place",
            hold.legal_hold_id.as_str(),
            &hold_sha,
        )
        .await?;
        let existing = sqlx::query("SELECT * FROM enterprise_legal_holds WHERE legal_hold_id = ?")
            .bind(hold.legal_hold_id.as_str())
            .fetch_optional(&mut *connection)
            .await
            .map_err(storage_error)?;
        if let Some(row) = existing {
            if row.get::<String, _>("hold_sha256") != hold_sha {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "legal hold id was reused with different scope".to_string(),
                ));
            }
            let current = legal_hold_from_row(&row);
            commit(&mut connection).await?;
            return Ok(current);
        }
        sqlx::query(
            "INSERT INTO enterprise_legal_holds \
             (legal_hold_id, tenant_scope_sha256, subject_kind, subject_sha256, reason_sha256, \
              hold_sha256, active, placed_at, released_at) VALUES (?, ?, ?, ?, ?, ?, 1, ?, NULL)",
        )
        .bind(hold.legal_hold_id.as_str())
        .bind(&hold.tenant_scope_sha256)
        .bind(hold.subject_kind.trim())
        .bind(&hold.subject_sha256)
        .bind(&hold.reason_sha256)
        .bind(&hold_sha)
        .bind(hold.placed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(hold.clone())
    }

    async fn release_legal_hold(
        &self,
        fence: &EnterpriseMutationFence,
        legal_hold_id: &LegalHoldId,
        released_at: i64,
    ) -> Result<LegalHold, EnterpriseScaleError> {
        if !valid_typed_id(legal_hold_id.as_str(), "lh_") || released_at < 0 {
            return Err(EnterpriseScaleError::Invalid(
                "legal hold release is invalid".to_string(),
            ));
        }
        let mutation_sha = canonical_sha256(&(legal_hold_id, released_at))?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "legal_hold.release",
            legal_hold_id.as_str(),
            &mutation_sha,
        )
        .await?;
        let row = sqlx::query("SELECT * FROM enterprise_legal_holds WHERE legal_hold_id = ?")
            .bind(legal_hold_id.as_str())
            .fetch_optional(&mut *connection)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| EnterpriseScaleError::NotFound("legal hold".to_string()))?;
        let current = legal_hold_from_row(&row);
        if !current.active {
            if current.released_at == Some(released_at) {
                commit(&mut connection).await?;
                return Ok(current);
            }
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Conflict(
                "legal hold is already released at a different time".to_string(),
            ));
        }
        if released_at < current.placed_at {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Invalid(
                "legal hold release predates placement".to_string(),
            ));
        }
        sqlx::query(
            "UPDATE enterprise_legal_holds SET active = 0, released_at = ? \
             WHERE legal_hold_id = ? AND active = 1",
        )
        .bind(released_at)
        .bind(legal_hold_id.as_str())
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(LegalHold {
            active: false,
            released_at: Some(released_at),
            ..current
        })
    }

    async fn decide_retention(
        &self,
        tenant_scope_sha256: &str,
        subject_kind: &str,
        subject_sha256: &str,
        created_at: i64,
        observed_at: i64,
    ) -> Result<RetentionDecision, EnterpriseScaleError> {
        if !valid_sha256(tenant_scope_sha256)
            || !nonempty(subject_kind)
            || !valid_sha256(subject_sha256)
            || created_at < 0
            || observed_at < created_at
        {
            return Err(EnterpriseScaleError::Invalid(
                "retention decision input is invalid".to_string(),
            ));
        }
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let policy_row = sqlx::query(
            "SELECT * FROM enterprise_retention_policies WHERE tenant_scope_sha256 = ?",
        )
        .bind(tenant_scope_sha256)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| {
            EnterpriseScaleError::Denied("retention policy is unavailable".to_string())
        })?;
        let retention_seconds = match subject_kind.trim() {
            "artifact" => policy_row.get::<i64, _>("artifact_retention_seconds"),
            "transcript" => policy_row.get::<i64, _>("transcript_retention_seconds"),
            "audit" => policy_row.get::<i64, _>("audit_retention_seconds"),
            _ => {
                return Err(EnterpriseScaleError::Invalid(
                    "retention subject kind must be artifact, transcript, or audit".to_string(),
                ));
            }
        };
        let delete_after = created_at.checked_add(retention_seconds).ok_or_else(|| {
            EnterpriseScaleError::Invalid("retention deadline overflow".to_string())
        })?;
        let hold_count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM enterprise_legal_holds WHERE tenant_scope_sha256 = ? \
             AND subject_kind = ? AND subject_sha256 = ? AND active = 1",
        )
        .bind(tenant_scope_sha256)
        .bind(subject_kind.trim())
        .bind(subject_sha256)
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let replica_count = if subject_kind.trim() == "artifact" {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(DISTINCT ack.region) FROM enterprise_artifact_replica_acknowledgements AS ack \
                 JOIN enterprise_artifact_replication_plans AS plan ON plan.replication_id = ack.replication_id \
                 WHERE plan.tenant_scope_sha256 = ? AND plan.artifact_sha256 = ? AND ack.status = 'available'",
            )
            .bind(tenant_scope_sha256)
            .bind(subject_sha256)
            .fetch_one(&mut *transaction)
            .await
            .map_err(storage_error)?
        } else {
            policy_row.get::<i64, _>("minimum_replica_regions")
        };
        let required = policy_row.get::<i64, _>("minimum_replica_regions");
        let disposition = if hold_count > 0 {
            RetentionDisposition::LegalHold
        } else if observed_at < delete_after {
            RetentionDisposition::Retain
        } else if replica_count < required {
            RetentionDisposition::ReplicationPending
        } else {
            RetentionDisposition::DeleteEligible
        };
        let decision = RetentionDecision {
            tenant_scope_sha256: tenant_scope_sha256.to_string(),
            subject_kind: subject_kind.trim().to_string(),
            subject_sha256: subject_sha256.to_string(),
            disposition,
            policy_version_sha256: policy_row.get("policy_version_sha256"),
            delete_after,
            active_legal_holds: i64_to_u32(hold_count, "active legal holds")?,
            available_replica_regions: i64_to_u32(replica_count, "replica regions")?,
            required_replica_regions: i64_to_u32(required, "required replica regions")?,
            observed_at,
        };
        transaction.commit().await.map_err(storage_error)?;
        Ok(decision)
    }

    async fn record_dr_checkpoint(
        &self,
        fence: &EnterpriseMutationFence,
        checkpoint: &DisasterRecoveryCheckpoint,
    ) -> Result<DisasterRecoveryCheckpoint, EnterpriseScaleError> {
        let digests = [
            &checkpoint.database_sha256,
            &checkpoint.artifact_index_sha256,
            &checkpoint.audit_head_sha256,
            &checkpoint.matrix_cursor_sha256,
            &checkpoint.certification_head_sha256,
        ];
        if !valid_typed_id(checkpoint.checkpoint_id.as_str(), "dr_")
            || !nonempty(&checkpoint.region)
            || digests.into_iter().any(|digest| !valid_sha256(digest))
            || checkpoint.maximum_rpo_seconds == 0
            || checkpoint.maximum_rto_seconds == 0
            || checkpoint.created_at < 0
        {
            return Err(EnterpriseScaleError::Invalid(
                "disaster recovery checkpoint is invalid".to_string(),
            ));
        }
        let checkpoint_sha = canonical_sha256(checkpoint)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "dr.record_checkpoint",
            checkpoint.checkpoint_id.as_str(),
            &checkpoint_sha,
        )
        .await?;
        let existing = sqlx::query(
            "SELECT checkpoint_sha256 FROM enterprise_dr_checkpoints WHERE checkpoint_id = ?",
        )
        .bind(checkpoint.checkpoint_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = existing {
            if row.get::<String, _>("checkpoint_sha256") != checkpoint_sha {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "DR checkpoint id was reused with different state".to_string(),
                ));
            }
            commit(&mut connection).await?;
            return Ok(checkpoint.clone());
        }
        sqlx::query(
            "INSERT INTO enterprise_dr_checkpoints \
             (checkpoint_id, region, database_sha256, artifact_index_sha256, audit_head_sha256, \
              matrix_cursor_sha256, certification_head_sha256, checkpoint_sha256, \
              maximum_rpo_seconds, maximum_rto_seconds, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(checkpoint.checkpoint_id.as_str())
        .bind(checkpoint.region.trim())
        .bind(&checkpoint.database_sha256)
        .bind(&checkpoint.artifact_index_sha256)
        .bind(&checkpoint.audit_head_sha256)
        .bind(&checkpoint.matrix_cursor_sha256)
        .bind(&checkpoint.certification_head_sha256)
        .bind(&checkpoint_sha)
        .bind(i64::from(checkpoint.maximum_rpo_seconds))
        .bind(i64::from(checkpoint.maximum_rto_seconds))
        .bind(checkpoint.created_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(checkpoint.clone())
    }

    async fn record_dr_drill(
        &self,
        fence: &EnterpriseMutationFence,
        drill: &DisasterRecoveryDrill,
    ) -> Result<DisasterRecoveryDrill, EnterpriseScaleError> {
        if !valid_typed_id(drill.drill_id.as_str(), "dd_")
            || !valid_typed_id(drill.checkpoint_id.as_str(), "dr_")
            || !nonempty(&drill.recovery_region)
            || !valid_sha256(&drill.evidence_sha256)
            || drill.completed_at < 0
        {
            return Err(EnterpriseScaleError::Invalid(
                "disaster recovery drill is invalid".to_string(),
            ));
        }
        let drill_sha = canonical_sha256(drill)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "dr.record_drill",
            drill.drill_id.as_str(),
            &drill_sha,
        )
        .await?;
        let checkpoint = sqlx::query(
            "SELECT maximum_rpo_seconds, maximum_rto_seconds, created_at FROM enterprise_dr_checkpoints \
             WHERE checkpoint_id = ?",
        )
        .bind(drill.checkpoint_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?
        .ok_or_else(|| EnterpriseScaleError::NotFound("DR checkpoint".to_string()))?;
        let within_objective = i64::from(drill.measured_rpo_seconds)
            <= checkpoint.get::<i64, _>("maximum_rpo_seconds")
            && i64::from(drill.measured_rto_seconds)
                <= checkpoint.get::<i64, _>("maximum_rto_seconds");
        if drill.completed_at < checkpoint.get::<i64, _>("created_at") {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Invalid(
                "DR drill predates its checkpoint".to_string(),
            ));
        }
        let passed =
            within_objective && drill.lease_fencing_verified && drill.accepted_state_verified;
        if (drill.status == DisasterRecoveryDrillStatus::Passed) != passed {
            rollback(&mut connection).await;
            return Err(EnterpriseScaleError::Invalid(
                "DR drill status does not match its objectives and integrity evidence".to_string(),
            ));
        }
        let existing =
            sqlx::query("SELECT drill_sha256 FROM enterprise_dr_drills WHERE drill_id = ?")
                .bind(drill.drill_id.as_str())
                .fetch_optional(&mut *connection)
                .await
                .map_err(storage_error)?;
        if let Some(row) = existing {
            if row.get::<String, _>("drill_sha256") != drill_sha {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "DR drill id was reused with different evidence".to_string(),
                ));
            }
            commit(&mut connection).await?;
            return Ok(drill.clone());
        }
        sqlx::query(
            "INSERT INTO enterprise_dr_drills \
             (drill_id, checkpoint_id, recovery_region, measured_rpo_seconds, measured_rto_seconds, \
              lease_fencing_verified, accepted_state_verified, status, evidence_sha256, \
              drill_sha256, completed_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(drill.drill_id.as_str())
        .bind(drill.checkpoint_id.as_str())
        .bind(drill.recovery_region.trim())
        .bind(i64::from(drill.measured_rpo_seconds))
        .bind(i64::from(drill.measured_rto_seconds))
        .bind(i64::from(drill.lease_fencing_verified))
        .bind(i64::from(drill.accepted_state_verified))
        .bind(drill.status.as_str())
        .bind(&drill.evidence_sha256)
        .bind(&drill_sha)
        .bind(drill.completed_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(drill.clone())
    }

    async fn register_load_model(
        &self,
        fence: &EnterpriseMutationFence,
        model: &LoadModelRegistration,
    ) -> Result<LoadModelRegistration, EnterpriseScaleError> {
        let has_dimensions = REQUIRED_LOAD_DIMENSIONS
            .iter()
            .all(|required| model.dimensions.contains(*required));
        if !valid_typed_id(model.load_model_id.as_str(), "lm_")
            || !nonempty(&model.version)
            || !valid_sha256(&model.content_sha256)
            || !has_dimensions
            || model.dimensions.len() > 64
            || model.test_window_seconds == 0
            || model.tenant_count == 0
            || model.project_count == 0
            || model.room_count == 0
            || model.matrix_events_per_second == 0
            || model.maximum_queue_depth == 0
            || model.noisy_neighbor_ratio_basis_points > 10_000
            || model.registered_at < 0
        {
            return Err(EnterpriseScaleError::Invalid(
                "factory load model is incomplete or invalid".to_string(),
            ));
        }
        let registration_sha = canonical_sha256(model)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "load_model.register",
            model.load_model_id.as_str(),
            &registration_sha,
        )
        .await?;
        let existing = sqlx::query(
            "SELECT registration_sha256 FROM enterprise_load_models \
             WHERE load_model_id = ? OR version = ?",
        )
        .bind(model.load_model_id.as_str())
        .bind(model.version.trim())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = existing {
            if row.get::<String, _>("registration_sha256") != registration_sha {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "load model id or version was reused with different content".to_string(),
                ));
            }
            commit(&mut connection).await?;
            return Ok(model.clone());
        }
        sqlx::query(
            "INSERT INTO enterprise_load_models \
             (load_model_id, version, content_sha256, dimensions_json, test_window_seconds, \
              tenant_count, project_count, room_count, matrix_events_per_second, \
              maximum_queue_depth, noisy_neighbor_ratio_basis_points, registration_sha256, registered_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(model.load_model_id.as_str())
        .bind(model.version.trim())
        .bind(&model.content_sha256)
        .bind(json(&model.dimensions)?)
        .bind(i64::from(model.test_window_seconds))
        .bind(i64::from(model.tenant_count))
        .bind(i64::from(model.project_count))
        .bind(i64::from(model.room_count))
        .bind(i64::from(model.matrix_events_per_second))
        .bind(u64_to_i64(model.maximum_queue_depth, "maximum queue depth")?)
        .bind(i64::from(model.noisy_neighbor_ratio_basis_points))
        .bind(&registration_sha)
        .bind(model.registered_at)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(model.clone())
    }

    async fn record_service_level(
        &self,
        fence: &EnterpriseMutationFence,
        measurement: &ServiceLevelMeasurement,
    ) -> Result<ServiceLevelMeasurement, EnterpriseScaleError> {
        if !nonempty(&measurement.idempotency_key)
            || !valid_sha256(&measurement.tenant_scope_sha256)
            || !nonempty(&measurement.metric)
            || measurement.target_units == 0
            || measurement.window_started_at < 0
            || measurement.window_ends_at <= measurement.window_started_at
            || measurement.measured_at < measurement.window_started_at
            || measurement.measured_at > measurement.window_ends_at
        {
            return Err(EnterpriseScaleError::Invalid(
                "service-level measurement is invalid".to_string(),
            ));
        }
        let measurement_sha = canonical_sha256(measurement)?;
        let mut connection = begin_fenced_mutation(
            &self.pool,
            fence,
            "service_level.record",
            measurement.idempotency_key.trim(),
            &measurement_sha,
        )
        .await?;
        let existing = sqlx::query(
            "SELECT measurement_sha256 FROM enterprise_service_level_measurements \
             WHERE idempotency_key = ?",
        )
        .bind(measurement.idempotency_key.trim())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = existing {
            if row.get::<String, _>("measurement_sha256") != measurement_sha {
                rollback(&mut connection).await;
                return Err(EnterpriseScaleError::Conflict(
                    "service-level idempotency key was reused".to_string(),
                ));
            }
            commit(&mut connection).await?;
            return Ok(measurement.clone());
        }
        sqlx::query(
            "INSERT INTO enterprise_service_level_measurements \
             (idempotency_key, tenant_scope_sha256, metric, target_units, observed_units, \
              error_budget_units, consumed_budget_units, window_started_at, window_ends_at, \
              measured_at, status, measurement_sha256) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(measurement.idempotency_key.trim())
        .bind(&measurement.tenant_scope_sha256)
        .bind(measurement.metric.trim())
        .bind(u64_to_i64(measurement.target_units, "SLO target")?)
        .bind(u64_to_i64(measurement.observed_units, "SLO observation")?)
        .bind(u64_to_i64(measurement.error_budget_units, "error budget")?)
        .bind(u64_to_i64(
            measurement.consumed_budget_units,
            "consumed budget",
        )?)
        .bind(measurement.window_started_at)
        .bind(measurement.window_ends_at)
        .bind(measurement.measured_at)
        .bind(measurement.status().as_str())
        .bind(&measurement_sha)
        .execute(&mut *connection)
        .await
        .map_err(storage_error)?;
        commit(&mut connection).await?;
        Ok(measurement.clone())
    }

    async fn operational_snapshot(
        &self,
        observed_at: i64,
    ) -> Result<EnterpriseOperationalSnapshot, EnterpriseScaleError> {
        if observed_at < 0 {
            return Err(EnterpriseScaleError::Invalid(
                "snapshot observation time is invalid".to_string(),
            ));
        }
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let leadership =
            sqlx::query("SELECT * FROM enterprise_control_plane_leadership WHERE singleton = 1")
                .fetch_optional(&mut *transaction)
                .await
                .map_err(storage_error)?
                .map(|row| leadership_from_row(&row))
                .transpose()?;
        let member_rows = sqlx::query(
            "SELECT * FROM enterprise_control_plane_members ORDER BY region, zone, instance_id",
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let members = member_rows
            .iter()
            .map(member_from_row)
            .collect::<Result<Vec<_>, _>>()?;
        let pool_rows = sqlx::query(
            "SELECT policy.*, \
              COALESCE((SELECT COUNT(*) FROM enterprise_worker_availability AS worker \
                WHERE worker.region = policy.region AND worker.zone = policy.zone \
                  AND worker.resource_class = policy.resource_class AND worker.worker_status = 'online'), 0) AS ready_workers, \
              COALESCE((SELECT SUM(available_slots) FROM enterprise_worker_availability AS worker \
                WHERE worker.region = policy.region AND worker.zone = policy.zone \
                  AND worker.resource_class = policy.resource_class AND worker.worker_status = 'online'), 0) AS available_slots, \
              COALESCE((SELECT COUNT(*) FROM enterprise_fleet_queue AS queue \
                WHERE queue.resource_class = policy.resource_class AND queue.status IN ('queued','retry_wait')), 0) AS queued_tasks, \
              (SELECT desired_replicas FROM enterprise_autoscaling_recommendations AS scale \
                WHERE scale.pool_id = policy.pool_id ORDER BY scale.observed_at DESC, scale.sequence DESC LIMIT 1) AS desired_replicas \
             FROM enterprise_zone_pool_policies AS policy WHERE policy.enabled = 1 \
             ORDER BY policy.region, policy.zone, policy.pool_id",
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let zones = pool_rows
            .iter()
            .map(|row| {
                Ok(EnterpriseZoneStatus {
                    pool_id: ZonePoolId::from_string(row.get::<String, _>("pool_id")),
                    region: row.get("region"),
                    zone: row.get("zone"),
                    ready_workers: i64_to_u32(row.get("ready_workers"), "ready workers")?,
                    available_slots: i64_to_u32(row.get("available_slots"), "available slots")?,
                    queued_tasks: i64_to_u64(row.get("queued_tasks"), "queued tasks")?,
                    last_desired_replicas: row
                        .get::<Option<i64>, _>("desired_replicas")
                        .map(|value| i64_to_u32(value, "desired replicas"))
                        .transpose()?,
                })
            })
            .collect::<Result<Vec<_>, EnterpriseScaleError>>()?;
        let queue_counts = sqlx::query(
            "SELECT \
              SUM(CASE WHEN status IN ('queued','retry_wait') THEN 1 ELSE 0 END) AS queued, \
              SUM(CASE WHEN status = 'acquired' THEN 1 ELSE 0 END) AS acquired, \
              SUM(CASE WHEN status = 'dead_letter' THEN 1 ELSE 0 END) AS dead_letter \
             FROM enterprise_fleet_queue",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let rollout_counts = sqlx::query(
            "SELECT \
              SUM(CASE WHEN status IN ('declared','progressing','healthy','degraded') THEN 1 ELSE 0 END) AS active, \
              SUM(CASE WHEN status = 'degraded' THEN 1 ELSE 0 END) AS degraded \
             FROM enterprise_worker_image_rollouts",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let pending_replica_regions = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE(SUM(json_array_length(plan.required_regions_json) - \
              (SELECT COUNT(*) FROM enterprise_artifact_replica_acknowledgements AS ack \
               WHERE ack.replication_id = plan.replication_id AND ack.status = 'available')), 0) \
             FROM enterprise_artifact_replication_plans AS plan",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let active_legal_holds = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM enterprise_legal_holds WHERE active = 1",
        )
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let slo_rows = sqlx::query(
            "SELECT \
              SUM(CASE WHEN status = 'budget_warning' THEN 1 ELSE 0 END) AS warnings, \
              SUM(CASE WHEN status = 'breached' THEN 1 ELSE 0 END) AS breaches \
             FROM enterprise_service_level_measurements WHERE window_ends_at >= ?",
        )
        .bind(observed_at)
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let latest_dr_checkpoint = sqlx::query(
            "SELECT * FROM enterprise_dr_checkpoints ORDER BY created_at DESC, checkpoint_id DESC LIMIT 1",
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?
        .map(|row| checkpoint_from_row(&row))
        .transpose()?;
        let load_model = sqlx::query(
            "SELECT * FROM enterprise_load_models ORDER BY registered_at DESC, load_model_id DESC LIMIT 1",
        )
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?
        .map(|row| load_model_from_row(&row))
        .transpose()?;
        let count =
            |row: &sqlx::sqlite::SqliteRow, name: &str| -> Result<u64, EnterpriseScaleError> {
                i64_to_u64(row.get::<Option<i64>, _>(name).unwrap_or(0), name)
            };
        let snapshot = EnterpriseOperationalSnapshot {
            observed_at,
            leadership,
            members,
            zones,
            queued_tasks: count(&queue_counts, "queued")?,
            acquired_tasks: count(&queue_counts, "acquired")?,
            dead_letter_tasks: count(&queue_counts, "dead_letter")?,
            active_rollouts: count(&rollout_counts, "active")?,
            degraded_rollouts: count(&rollout_counts, "degraded")?,
            pending_replica_regions: i64_to_u64(
                pending_replica_regions.max(0),
                "pending replica regions",
            )?,
            active_legal_holds: i64_to_u64(active_legal_holds, "active legal holds")?,
            service_level_warnings: count(&slo_rows, "warnings")?,
            service_level_breaches: count(&slo_rows, "breaches")?,
            latest_dr_checkpoint,
            load_model,
        };
        transaction.commit().await.map_err(storage_error)?;
        Ok(snapshot)
    }
}
