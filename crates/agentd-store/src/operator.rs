//! Bounded structured operator diagnostics for the canonical control plane.

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::StoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: DoctorStatus,
    pub code: String,
    pub count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub schema_version: u32,
    pub observed_at: i64,
    pub checks: Vec<DoctorCheck>,
}

pub async fn run_doctor(pool: &SqlitePool, observed_at: i64) -> Result<DoctorReport, StoreError> {
    let schema_text =
        sqlx::query_scalar::<_, String>("SELECT value FROM schema_meta WHERE key = 'version'")
            .fetch_one(pool)
            .await?;
    let schema_version = schema_text
        .parse::<u32>()
        .map_err(|error| StoreError::Invariant(format!("schema version is invalid: {error}")))?;
    let integrity = sqlx::query_scalar::<_, String>("PRAGMA quick_check")
        .fetch_one(pool)
        .await?;
    let mut checks = vec![DoctorCheck {
        name: "database".to_string(),
        status: if integrity == "ok" {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Fail
        },
        code: if integrity == "ok" {
            "integrity_ok".to_string()
        } else {
            "integrity_failed".to_string()
        },
        count: None,
    }];
    checks.push(DoctorCheck {
        name: "schema".to_string(),
        status: if schema_version == 27 {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Fail
        },
        code: if schema_version == 27 {
            "schema_current".to_string()
        } else {
            "schema_mismatch".to_string()
        },
        count: Some(u64::from(schema_version)),
    });

    let authority = sqlx::query(
        "SELECT state, authority_owner FROM cutover_runs ORDER BY updated_at DESC, id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;
    checks.push(match authority {
        Some(row) => {
            let state = row.get::<String, _>("state");
            let owner = row.get::<String, _>("authority_owner");
            let valid = matches!(state.as_str(), "active" | "retired") && owner == "agentd";
            DoctorCheck {
                name: "cutover_authority".to_string(),
                status: if valid {
                    DoctorStatus::Pass
                } else {
                    DoctorStatus::Warn
                },
                code: if valid {
                    "agentd_authoritative".to_string()
                } else {
                    format!("cutover_{state}_{owner}")
                },
                count: Some(1),
            }
        }
        None => DoctorCheck {
            name: "cutover_authority".to_string(),
            status: DoctorStatus::Warn,
            code: "cutover_not_started".to_string(),
            count: Some(0),
        },
    });

    let project_bindings =
        count(pool, "SELECT COUNT(*) FROM matrix_gateway_project_bindings").await?;
    checks.push(count_check(
        "project_authority",
        project_bindings,
        project_bindings > 0,
        "project_bindings_available",
        "project_bindings_absent",
        DoctorStatus::Warn,
    ));
    let online_workers = count(
        pool,
        "SELECT COUNT(*) FROM enterprise_worker_availability WHERE worker_status = 'online'",
    )
    .await?;
    checks.push(count_check(
        "workers",
        online_workers,
        online_workers > 0,
        "workers_online",
        "workers_unavailable",
        DoctorStatus::Warn,
    ));
    let active_leases = count(
        pool,
        "SELECT COUNT(*) FROM execution_task_leases WHERE status = 'active'",
    )
    .await?;
    checks.push(DoctorCheck {
        name: "leases".to_string(),
        status: DoctorStatus::Pass,
        code: "lease_heads_readable".to_string(),
        count: Some(active_leases),
    });
    let queued = count(
        pool,
        "SELECT COUNT(*) FROM enterprise_fleet_queue WHERE status IN ('queued','retry_wait')",
    )
    .await?;
    checks.push(DoctorCheck {
        name: "queue".to_string(),
        status: DoctorStatus::Pass,
        code: if queued == 0 {
            "queue_empty".to_string()
        } else {
            "queue_backlog_present".to_string()
        },
        count: Some(queued),
    });
    let recoverable_runtimes = count(
        pool,
        "SELECT COUNT(*) FROM runtime_sessions WHERE status IN ('starting','running','resume_pending')",
    )
    .await?;
    checks.push(DoctorCheck {
        name: "runtime".to_string(),
        status: DoctorStatus::Pass,
        code: "runtime_ledger_readable".to_string(),
        count: Some(recoverable_runtimes),
    });
    let pending_matrix = count(
        pool,
        "SELECT COUNT(*) FROM matrix_gateway_outbox WHERE delivered_at IS NULL",
    )
    .await?;
    checks.push(DoctorCheck {
        name: "matrix".to_string(),
        status: if pending_matrix == 0 {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Warn
        },
        code: if pending_matrix == 0 {
            "matrix_outbox_clear".to_string()
        } else {
            "matrix_delivery_pending".to_string()
        },
        count: Some(pending_matrix),
    });
    let pending_certifications = count(
        pool,
        "SELECT COUNT(*) FROM openfab_certification_requests AS request \
         LEFT JOIN openfab_certification_results AS result ON result.request_id = request.request_id \
         WHERE result.result_id IS NULL",
    )
    .await?;
    checks.push(DoctorCheck {
        name: "openfab".to_string(),
        status: if pending_certifications == 0 {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Warn
        },
        code: if pending_certifications == 0 {
            "certification_clear".to_string()
        } else {
            "certification_pending".to_string()
        },
        count: Some(pending_certifications),
    });
    let artifacts = count(pool, "SELECT COUNT(*) FROM execution_artifacts").await?;
    checks.push(DoctorCheck {
        name: "artifacts".to_string(),
        status: DoctorStatus::Pass,
        code: "artifact_index_readable".to_string(),
        count: Some(artifacts),
    });
    let backups = count(pool, "SELECT COUNT(*) FROM cutover_backup_manifests").await?;
    checks.push(count_check(
        "backup",
        backups,
        backups > 0,
        "backup_manifest_available",
        "backup_manifest_absent",
        DoctorStatus::Warn,
    ));
    let enterprise_members = count(
        pool,
        "SELECT COUNT(*) FROM enterprise_control_plane_members",
    )
    .await?;
    let ready_control_plane = count_with_i64(
        pool,
        "SELECT COUNT(*) FROM enterprise_control_plane_members \
         WHERE status = 'ready' AND observed_at >= ?",
        observed_at.saturating_sub(60),
    )
    .await?;
    checks.push(DoctorCheck {
        name: "enterprise_control_plane".to_string(),
        status: if ready_control_plane >= 2 {
            DoctorStatus::Pass
        } else if enterprise_members == 0 {
            DoctorStatus::Warn
        } else {
            DoctorStatus::Fail
        },
        code: if ready_control_plane >= 2 {
            "control_plane_quorum_ready".to_string()
        } else if enterprise_members == 0 {
            "enterprise_not_configured".to_string()
        } else {
            "control_plane_quorum_unavailable".to_string()
        },
        count: Some(ready_control_plane),
    });
    let live_leader = count_with_i64(
        pool,
        "SELECT COUNT(*) FROM enterprise_control_plane_leadership WHERE expires_at > ?",
        observed_at,
    )
    .await?;
    checks.push(DoctorCheck {
        name: "enterprise_leadership".to_string(),
        status: if live_leader == 1 {
            DoctorStatus::Pass
        } else if enterprise_members == 0 {
            DoctorStatus::Warn
        } else {
            DoctorStatus::Fail
        },
        code: if live_leader == 1 {
            "leadership_live".to_string()
        } else if enterprise_members == 0 {
            "enterprise_not_configured".to_string()
        } else {
            "leadership_unavailable".to_string()
        },
        count: Some(live_leader),
    });
    let degraded_rollouts = count(
        pool,
        "SELECT COUNT(*) FROM enterprise_worker_image_rollouts \
         WHERE status IN ('degraded', 'rolled_back')",
    )
    .await?;
    checks.push(DoctorCheck {
        name: "enterprise_rollout".to_string(),
        status: if degraded_rollouts == 0 {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Warn
        },
        code: if degraded_rollouts == 0 {
            "rollouts_healthy".to_string()
        } else {
            "rollouts_degraded".to_string()
        },
        count: Some(degraded_rollouts),
    });
    let unaudited_enterprise_versions = count(
        pool,
        "SELECT \
           (SELECT COUNT(*) FROM enterprise_worker_image_zone_observations AS current \
            WHERE NOT EXISTS (SELECT 1 FROM enterprise_worker_image_zone_observation_history AS history \
              WHERE history.observation_sha256 = current.observation_sha256 \
                AND history.rollout_id = current.rollout_id \
                AND history.zone = current.zone \
                AND history.observed_image_digest = current.observed_image_digest \
                AND history.signature_verified = current.signature_verified \
                AND history.ready_workers = current.ready_workers \
                AND history.desired_workers = current.desired_workers \
                AND history.observed_at = current.observed_at)) + \
           (SELECT COUNT(*) FROM enterprise_zone_pool_policies AS current \
            WHERE NOT EXISTS (SELECT 1 FROM enterprise_zone_pool_policy_versions AS history \
              WHERE history.pool_id = current.pool_id \
                AND history.region = current.region \
                AND history.zone = current.zone \
                AND history.resource_class = current.resource_class \
                AND history.trust_domain = current.trust_domain \
                AND history.rollout_id = current.rollout_id \
                AND history.minimum_replicas = current.minimum_replicas \
                AND history.maximum_replicas = current.maximum_replicas \
                AND history.target_queue_per_slot = current.target_queue_per_slot \
                AND history.scale_down_cooldown_seconds = current.scale_down_cooldown_seconds \
                AND history.enabled = current.enabled \
                AND history.policy_sha256 = current.policy_sha256 \
                AND history.updated_at = current.updated_at)) + \
           (SELECT COUNT(*) FROM enterprise_retention_policies AS current \
            WHERE NOT EXISTS (SELECT 1 FROM enterprise_retention_policy_versions AS history \
              WHERE history.tenant_scope_sha256 = current.tenant_scope_sha256 \
                AND history.policy_version_sha256 = current.policy_version_sha256 \
                AND history.artifact_retention_seconds = current.artifact_retention_seconds \
                AND history.transcript_retention_seconds = current.transcript_retention_seconds \
                AND history.audit_retention_seconds = current.audit_retention_seconds \
                AND history.minimum_replica_regions = current.minimum_replica_regions \
                AND history.updated_at = current.updated_at)) + \
           (SELECT COUNT(*) FROM enterprise_retention_policies \
            WHERE audit_retention_seconds < transcript_retention_seconds)",
    )
    .await?;
    checks.push(DoctorCheck {
        name: "enterprise_audit_history".to_string(),
        status: if unaudited_enterprise_versions == 0 {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Fail
        },
        code: if unaudited_enterprise_versions == 0 {
            "enterprise_history_complete".to_string()
        } else {
            "enterprise_history_missing".to_string()
        },
        count: Some(unaudited_enterprise_versions),
    });
    let rolled_back_workers_online = count(
        pool,
        "SELECT COUNT(*) FROM enterprise_worker_availability AS availability \
         JOIN workers AS worker ON worker.id = availability.worker_id \
         JOIN enterprise_worker_image_rollouts AS rollout ON rollout.rollout_id = \
           CASE WHEN json_valid(worker.labels_json) \
             THEN json_extract(worker.labels_json, '$.agentd_attestation.rollout_id') END \
         WHERE rollout.status = 'rolled_back' AND availability.worker_status = 'online'",
    )
    .await?;
    checks.push(DoctorCheck {
        name: "enterprise_rollout_fencing".to_string(),
        status: if rolled_back_workers_online == 0 {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Fail
        },
        code: if rolled_back_workers_online == 0 {
            "rolled_back_workers_offline".to_string()
        } else {
            "rolled_back_workers_online".to_string()
        },
        count: Some(rolled_back_workers_online),
    });
    let invalid_replication_plans = count(
        pool,
        "SELECT COUNT(*) FROM enterprise_artifact_replication_plans AS plan \
         WHERE NOT EXISTS (SELECT 1 FROM execution_artifacts AS artifact \
           WHERE artifact.id = plan.execution_artifact_id \
             AND artifact.content_sha256 = plan.artifact_sha256)",
    )
    .await?;
    checks.push(DoctorCheck {
        name: "enterprise_replication_integrity".to_string(),
        status: if invalid_replication_plans == 0 {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Fail
        },
        code: if invalid_replication_plans == 0 {
            "replication_artifacts_resolved".to_string()
        } else {
            "replication_artifacts_missing_or_mismatched".to_string()
        },
        count: Some(invalid_replication_plans),
    });
    let missing_replica_regions = count(
        pool,
        "SELECT COUNT(*) FROM enterprise_artifact_replication_plans AS plan, \
         json_each(plan.required_regions_json) AS required \
         WHERE NOT EXISTS (SELECT 1 FROM enterprise_artifact_replica_acknowledgements AS ack \
           WHERE ack.replication_id = plan.replication_id AND ack.region = required.value \
             AND ack.status = 'available')",
    )
    .await?;
    checks.push(DoctorCheck {
        name: "enterprise_replication".to_string(),
        status: if missing_replica_regions == 0 {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Warn
        },
        code: if missing_replica_regions == 0 {
            "replicas_complete".to_string()
        } else {
            "replicas_pending".to_string()
        },
        count: Some(missing_replica_regions),
    });
    let active_slo_breaches = count_with_i64(
        pool,
        "SELECT COUNT(*) FROM enterprise_service_level_measurements \
         WHERE status = 'breached' AND window_ends_at >= ?",
        observed_at,
    )
    .await?;
    checks.push(DoctorCheck {
        name: "enterprise_slo".to_string(),
        status: if active_slo_breaches == 0 {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Fail
        },
        code: if active_slo_breaches == 0 {
            "slo_within_objective".to_string()
        } else {
            "slo_breached".to_string()
        },
        count: Some(active_slo_breaches),
    });
    let dr_checkpoints = count(pool, "SELECT COUNT(*) FROM enterprise_dr_checkpoints").await?;
    checks.push(count_check(
        "enterprise_dr",
        dr_checkpoints,
        dr_checkpoints > 0,
        "dr_checkpoint_available",
        "dr_checkpoint_absent",
        DoctorStatus::Warn,
    ));
    let load_models = count(pool, "SELECT COUNT(*) FROM enterprise_load_models").await?;
    checks.push(count_check(
        "enterprise_load_model",
        load_models,
        load_models > 0,
        "load_model_pinned",
        "load_model_absent",
        DoctorStatus::Warn,
    ));
    let ok = checks
        .iter()
        .all(|check| check.status != DoctorStatus::Fail);
    Ok(DoctorReport {
        ok,
        schema_version,
        observed_at,
        checks,
    })
}

async fn count(pool: &SqlitePool, query: &str) -> Result<u64, StoreError> {
    let value = sqlx::query_scalar::<_, i64>(query).fetch_one(pool).await?;
    u64::try_from(value).map_err(|_| StoreError::Invariant("doctor count is negative".to_string()))
}

async fn count_with_i64(pool: &SqlitePool, query: &str, value: i64) -> Result<u64, StoreError> {
    let count = sqlx::query_scalar::<_, i64>(query)
        .bind(value)
        .fetch_one(pool)
        .await?;
    u64::try_from(count).map_err(|_| StoreError::Invariant("doctor count is negative".to_string()))
}

fn count_check(
    name: &str,
    count: u64,
    pass: bool,
    pass_code: &str,
    absent_code: &str,
    absent_status: DoctorStatus,
) -> DoctorCheck {
    DoctorCheck {
        name: name.to_string(),
        status: if pass {
            DoctorStatus::Pass
        } else {
            absent_status
        },
        code: if pass { pass_code } else { absent_code }.to_string(),
        count: Some(count),
    }
}
