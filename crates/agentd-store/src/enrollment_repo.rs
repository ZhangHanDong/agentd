//! Atomic worker, incarnation, and verified workload-certificate enrollment.

use agentd_core::ports::SecurityError;
use std::collections::BTreeSet;

use agentd_core::types::{SecurityDenialReason, WorkerStatus, WorkloadRole};
use serde_json::Value;
use sqlx::{Row, SqlitePool};

use crate::security_repo::{
    WorkloadIdentityBindingCreate, WorkloadIdentityBindingRecord, get_workload_identity_binding,
};
use crate::util::{SqliteImmediateTransaction, now_unix};
use crate::worker_repo::{
    self, WorkerCreate, WorkerIncarnationRecord, WorkerRecord, WorkerRegistration,
};

const MAX_ENROLLMENT_METADATA_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerWorkloadEnrollment {
    pub worker: WorkerCreate,
    pub incarnation: WorkerRegistration,
    pub binding: WorkloadIdentityBindingCreate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerWorkloadEnrollmentRecord {
    pub worker: WorkerRecord,
    pub incarnation: WorkerIncarnationRecord,
    pub identity: WorkloadIdentityBindingRecord,
}

/// Atomically create or exactly replay one verified worker enrollment.
pub async fn enroll_worker_workload_identity(
    pool: &SqlitePool,
    request: WorkerWorkloadEnrollment,
) -> Result<WorkerWorkloadEnrollmentRecord, SecurityError> {
    validate_enrollment(&request)?;
    let labels_json = serde_json::to_string(&request.worker.labels)
        .map_err(|error| invalid(format!("invalid worker labels: {error}")))?;
    let capabilities_json = serde_json::to_string(&request.incarnation.capabilities)
        .map_err(|error| invalid(format!("invalid worker capabilities: {error}")))?;
    let now = now_unix();
    let mut tx = SqliteImmediateTransaction::begin(pool)
        .await
        .map_err(storage)?;
    validate_attestation_rollout(&mut tx, &request).await?;

    let existing_binding = sqlx::query(
        "SELECT spiffe_uri, role, trust_domain, worker_id, worker_incarnation_id, \
         not_before, not_after, revoked_at \
         FROM workload_identity_bindings WHERE certificate_sha256 = ?",
    )
    .bind(&request.binding.certificate_sha256)
    .fetch_optional(&mut *tx)
    .await
    .map_err(storage)?;
    if let Some(row) = existing_binding.as_ref() {
        let exact = row.get::<String, _>("spiffe_uri") == request.binding.spiffe_uri
            && row.get::<String, _>("role") == "worker"
            && row.get::<String, _>("trust_domain") == request.binding.trust_domain
            && row.get::<Option<String>, _>("worker_id").as_deref()
                == request.binding.worker_id.as_ref().map(|id| id.as_str())
            && row
                .get::<Option<String>, _>("worker_incarnation_id")
                .as_deref()
                == request
                    .binding
                    .worker_incarnation_id
                    .as_ref()
                    .map(|id| id.as_str())
            && row.get::<i64, _>("not_before") == request.binding.not_before
            && row.get::<i64, _>("not_after") == request.binding.not_after
            && row.get::<Option<i64>, _>("revoked_at").is_none();
        if !exact {
            return Err(invalid(
                "certificate fingerprint already has a changed or revoked binding",
            ));
        }
    }

    let existing_worker =
        sqlx::query("SELECT status, trust_domain, labels_json FROM workers WHERE id = ?")
            .bind(request.worker.id.as_str())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
    match existing_worker.as_ref() {
        Some(row) => {
            let status = row.get::<String, _>("status");
            if status == WorkerStatus::Retired.as_str()
                || row.get::<String, _>("trust_domain") != request.worker.trust_domain
            {
                return Err(invalid(
                    "worker id already has a changed trust domain or retired enrollment",
                ));
            }
        }
        None => {
            sqlx::query(
                "INSERT INTO workers \
                 (id, status, trust_domain, labels_json, record_version, created_at, updated_at) \
                 VALUES (?, 'offline', ?, ?, 1, ?, ?)",
            )
            .bind(request.worker.id.as_str())
            .bind(&request.worker.trust_domain)
            .bind(&labels_json)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        }
    }

    let existing_incarnation = sqlx::query(
        "SELECT worker_id, daemon_version, host_name, network_zone, capabilities_json, \
         is_current FROM worker_incarnations WHERE id = ?",
    )
    .bind(request.incarnation.id.as_str())
    .fetch_optional(&mut *tx)
    .await
    .map_err(storage)?;
    match existing_incarnation.as_ref() {
        Some(row) => {
            let stored_capabilities: Value =
                serde_json::from_str(&row.get::<String, _>("capabilities_json"))
                    .map_err(|error| storage(format!("corrupt capabilities: {error}")))?;
            let stored_labels: Value = existing_worker
                .as_ref()
                .map(|worker| worker.get::<String, _>("labels_json"))
                .map(|labels| serde_json::from_str(&labels))
                .transpose()
                .map_err(|error| storage(format!("corrupt worker labels: {error}")))?
                .unwrap_or_else(|| request.worker.labels.clone());
            let exact = row.get::<String, _>("worker_id") == request.worker.id.as_str()
                && row.get::<String, _>("daemon_version") == request.incarnation.daemon_version
                && row.get::<String, _>("host_name") == request.incarnation.host_name
                && row.get::<Option<String>, _>("network_zone") == request.incarnation.network_zone
                && stored_capabilities == request.incarnation.capabilities
                && stored_labels == request.worker.labels
                && row.get::<i64, _>("is_current") == 1;
            if !exact {
                return Err(invalid(
                    "worker incarnation id already has a changed or stale registration",
                ));
            }
        }
        None => {
            sqlx::query(
                "UPDATE worker_incarnations SET is_current = 0, superseded_at = ? \
                 WHERE worker_id = ? AND is_current = 1",
            )
            .bind(now)
            .bind(request.worker.id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "INSERT INTO worker_incarnations \
                 (id, worker_id, daemon_version, host_name, network_zone, capabilities_json, \
                  is_current, registered_at, last_seen_at, superseded_at) \
                 VALUES (?, ?, ?, ?, ?, ?, 1, ?, ?, NULL)",
            )
            .bind(request.incarnation.id.as_str())
            .bind(request.worker.id.as_str())
            .bind(&request.incarnation.daemon_version)
            .bind(&request.incarnation.host_name)
            .bind(&request.incarnation.network_zone)
            .bind(&capabilities_json)
            .bind(now)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query(
                "UPDATE workers SET status = 'online', labels_json = ?, \
                 record_version = record_version + 1, updated_at = ?, retired_at = NULL \
                 WHERE id = ?",
            )
            .bind(&labels_json)
            .bind(now)
            .bind(request.worker.id.as_str())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        }
    }

    if existing_binding.is_none() {
        sqlx::query(
            "INSERT INTO workload_identity_bindings \
             (certificate_sha256, spiffe_uri, role, trust_domain, worker_id, \
              worker_incarnation_id, not_before, not_after, revoked_at, \
              revocation_reason, created_at) \
             VALUES (?, ?, 'worker', ?, ?, ?, ?, ?, NULL, NULL, ?)",
        )
        .bind(&request.binding.certificate_sha256)
        .bind(&request.binding.spiffe_uri)
        .bind(&request.binding.trust_domain)
        .bind(request.worker.id.as_str())
        .bind(request.incarnation.id.as_str())
        .bind(request.binding.not_before)
        .bind(request.binding.not_after)
        .bind(request.binding.created_at)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
    }
    tx.commit().await.map_err(storage)?;

    let worker = worker_repo::get_worker(pool, &request.worker.id)
        .await
        .map_err(storage)?
        .ok_or_else(|| storage("worker disappeared after enrollment"))?;
    let incarnation = worker_repo::get_incarnation(pool, &request.incarnation.id)
        .await
        .map_err(storage)?
        .ok_or_else(|| storage("incarnation disappeared after enrollment"))?;
    let identity = get_workload_identity_binding(pool, &request.binding.certificate_sha256)
        .await?
        .ok_or_else(|| storage("identity disappeared after enrollment"))?;
    Ok(WorkerWorkloadEnrollmentRecord {
        worker,
        incarnation,
        identity,
    })
}

fn validate_enrollment(request: &WorkerWorkloadEnrollment) -> Result<(), SecurityError> {
    let expected_spiffe = format!(
        "spiffe://{}/worker/{}",
        request.worker.trust_domain, request.incarnation.id
    );
    if request.binding.role != WorkloadRole::Worker
        || !valid_typed_id(request.worker.id.as_str(), "wk_")
        || !valid_typed_id(request.incarnation.id.as_str(), "wi_")
        || request.binding.worker_id.as_ref() != Some(&request.worker.id)
        || request.binding.worker_incarnation_id.as_ref() != Some(&request.incarnation.id)
        || request.binding.trust_domain != request.worker.trust_domain
        || request.binding.spiffe_uri != expected_spiffe
        || request.binding.not_before < 0
        || request.binding.not_after <= request.binding.not_before
        || request.binding.created_at < request.binding.not_before
        || request.binding.created_at >= request.binding.not_after
        || !valid_trust_domain(&request.worker.trust_domain)
        || !bounded_text(&request.incarnation.daemon_version, 256)
        || !bounded_text(&request.incarnation.host_name, 256)
        || request
            .incarnation
            .network_zone
            .as_deref()
            .is_some_and(|value| !bounded_text(value, 256))
        || !request.worker.labels.is_object()
        || !valid_worker_attestation(&request.worker.labels)
        || !request.incarnation.capabilities.is_object()
        || serde_json::to_vec(&request.worker.labels)
            .ok()
            .is_none_or(|value| value.len() > MAX_ENROLLMENT_METADATA_BYTES)
        || serde_json::to_vec(&request.incarnation.capabilities)
            .ok()
            .is_none_or(|value| value.len() > MAX_ENROLLMENT_METADATA_BYTES)
        || request.binding.certificate_sha256.len() != 64
        || !request
            .binding
            .certificate_sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid("worker workload enrollment fields do not agree"));
    }
    Ok(())
}

async fn validate_attestation_rollout(
    connection: &mut SqliteImmediateTransaction,
    request: &WorkerWorkloadEnrollment,
) -> Result<(), SecurityError> {
    let attestation = &request.worker.labels["agentd_attestation"];
    let rollout_id = attestation["rollout_id"]
        .as_str()
        .ok_or_else(|| invalid("worker attestation rollout id is missing"))?;
    let rollout = sqlx::query(
        "SELECT image_digest, signature_bundle_sha256, policy_sha256, \
                required_zones_json, status \
         FROM enterprise_worker_image_rollouts WHERE rollout_id = ?",
    )
    .bind(rollout_id)
    .fetch_optional(&mut **connection)
    .await
    .map_err(storage)?
    .ok_or(SecurityError::Denied(
        SecurityDenialReason::IdentityUntrusted,
    ))?;
    let required_zones: BTreeSet<String> =
        serde_json::from_str(&rollout.get::<String, _>("required_zones_json"))
            .map_err(|error| storage(format!("corrupt rollout zones: {error}")))?;
    let field = |name: &str| attestation.get(name).and_then(Value::as_str);
    let rollout_status = rollout.get::<String, _>("status");
    let rollout_image = rollout.get::<String, _>("image_digest");
    let rollout_signature = rollout.get::<String, _>("signature_bundle_sha256");
    let rollout_policy = rollout.get::<String, _>("policy_sha256");
    let matching_rollout = rollout_status != "rolled_back"
        && field("image_digest") == Some(rollout_image.as_str())
        && field("signature_bundle_sha256") == Some(rollout_signature.as_str())
        && field("signature_policy_sha256") == Some(rollout_policy.as_str())
        && field("zone").is_some_and(|zone| required_zones.contains(zone));
    let policy_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM enterprise_zone_pool_policies \
         WHERE rollout_id = ? AND region = ? AND zone = ? AND resource_class = ? \
           AND trust_domain = ? AND enabled = 1",
    )
    .bind(rollout_id)
    .bind(field("region"))
    .bind(field("zone"))
    .bind(field("resource_class"))
    .bind(&request.worker.trust_domain)
    .fetch_one(&mut **connection)
    .await
    .map_err(storage)?;
    if !matching_rollout || policy_count != 1 {
        return Err(SecurityError::Denied(
            SecurityDenialReason::IdentityUntrusted,
        ));
    }
    Ok(())
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

fn bounded_text(value: &str, maximum_bytes: usize) -> bool {
    value == value.trim()
        && !value.is_empty()
        && value.len() <= maximum_bytes
        && !value.chars().any(char::is_control)
}

fn valid_worker_attestation(labels: &Value) -> bool {
    let Some(attestation) = labels.get("agentd_attestation") else {
        return false;
    };
    let text = |field: &str| {
        attestation
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| bounded_text(value, 512))
    };
    let sha256 = |field: &str| {
        attestation
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
    };
    attestation.is_object()
        && attestation
            .get("rollout_id")
            .and_then(Value::as_str)
            .is_some_and(|value| valid_typed_id(value, "ir_"))
        && attestation
            .get("image_digest")
            .and_then(Value::as_str)
            .and_then(|value| value.strip_prefix("sha256:"))
            .is_some_and(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        && sha256("signature_bundle_sha256")
        && sha256("signature_policy_sha256")
        && text("region")
        && text("zone")
        && text("resource_class")
}

fn invalid(message: impl Into<String>) -> SecurityError {
    SecurityError::Invalid(message.into())
}

fn storage(error: impl std::fmt::Display) -> SecurityError {
    SecurityError::Unavailable(format!("worker enrollment storage unavailable: {error}"))
}
