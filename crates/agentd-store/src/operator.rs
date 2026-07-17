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

pub async fn run_doctor(
    pool: &SqlitePool,
    observed_at: i64,
) -> Result<DoctorReport, StoreError> {
    let schema_text = sqlx::query_scalar::<_, String>(
        "SELECT value FROM schema_meta WHERE key = 'version'",
    )
    .fetch_one(pool)
    .await?;
    let schema_version = schema_text.parse::<u32>().map_err(|error| {
        StoreError::Invariant(format!("schema version is invalid: {error}"))
    })?;
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
        status: if schema_version == 22 {
            DoctorStatus::Pass
        } else {
            DoctorStatus::Fail
        },
        code: if schema_version == 22 {
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

    let project_bindings = count(
        pool,
        "SELECT COUNT(*) FROM matrix_gateway_project_bindings",
    )
    .await?;
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
    u64::try_from(value)
        .map_err(|_| StoreError::Invariant("doctor count is negative".to_string()))
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
