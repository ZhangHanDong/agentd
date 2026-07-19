//! Structured control-plane health and recovery diagnostics.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use crate::error::StoreError;
use crate::util::now_unix;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationalDoctorReport {
    pub checked_at: i64,
    pub workers_online: i64,
    pub workers_draining: i64,
    pub workers_offline: i64,
    pub active_leases: i64,
    pub runtime_running: i64,
    pub runtime_resume_pending: i64,
    pub recovery_pending: i64,
    pub artifacts: i64,
    pub audit_events: i64,
    pub matrix_rooms: i64,
    pub matrix_events: i64,
    pub projects: i64,
    pub queued_tasks: i64,
    pub in_flight_runs: i64,
    pub authority_snapshots: i64,
    pub authority_snapshots_expired: i64,
    pub active_leases_expired: i64,
    pub tasks_missing_execution_spec: i64,
    pub schema_version: i64,
    pub ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationalRemediationReport {
    pub observed_at: i64,
    pub workers_marked_offline: u64,
    pub leases_expired: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CutoverInventory {
    pub captured_at: i64,
    pub workers: WorkerInventory,
    pub active_leases: i64,
    pub runtime_running: i64,
    pub runtime_resume_pending: i64,
    pub matrix_rooms: i64,
    pub matrix_events: i64,
    pub artifacts: i64,
    pub queued_tasks: i64,
    pub in_flight_runs: i64,
    pub ready_for_cutover: bool,
    pub rollback_requires_new_lease_epoch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerInventory {
    pub online: i64,
    pub draining: i64,
    pub offline: i64,
}

#[derive(Debug, Clone)]
pub struct OperationalDoctor {
    pool: SqlitePool,
}

impl OperationalDoctor {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Produce a bounded, structured health report from durable control-plane
    /// tables. The report is intentionally independent of daemon log parsing.
    pub async fn check(&self) -> Result<OperationalDoctorReport, StoreError> {
        let workers_online = count(
            &self.pool,
            "SELECT COUNT(*) FROM workers WHERE status = 'online'",
        )
        .await?;
        let workers_draining = count(
            &self.pool,
            "SELECT COUNT(*) FROM workers WHERE status = 'draining'",
        )
        .await?;
        let workers_offline = count(
            &self.pool,
            "SELECT COUNT(*) FROM workers WHERE status = 'offline'",
        )
        .await?;
        let active_leases = count(
            &self.pool,
            "SELECT COUNT(*) FROM execution_task_leases WHERE status = 'active'",
        )
        .await?;
        let runtime_running = count(
            &self.pool,
            "SELECT COUNT(*) FROM runtime_sessions WHERE status IN ('starting', 'running')",
        )
        .await?;
        let runtime_resume_pending = count(
            &self.pool,
            "SELECT COUNT(*) FROM runtime_sessions WHERE status = 'resume_pending'",
        )
        .await?;
        let recovery_pending = count(
            &self.pool,
            "SELECT COUNT(*) FROM runtime_attempts WHERE status = 'gone'",
        )
        .await?;
        let artifacts = count(&self.pool, "SELECT COUNT(*) FROM execution_artifacts").await?;
        let audit_events = count(&self.pool, "SELECT COUNT(*) FROM execution_audit_events").await?;
        let matrix_rooms = count(&self.pool, "SELECT COUNT(*) FROM matrix_bridge_rooms").await?;
        let matrix_events = count(&self.pool, "SELECT COUNT(*) FROM matrix_bridge_events").await?;
        let projects = count(&self.pool, "SELECT COUNT(*) FROM projects").await?;
        let queued_tasks = count(
            &self.pool,
            "SELECT COUNT(*) FROM task_runs WHERE status = 'queued' AND finished_at IS NULL",
        )
        .await?;
        let in_flight_runs = count(
            &self.pool,
            "SELECT COUNT(*) FROM runs WHERE status NOT IN ('finished', 'failed')",
        )
        .await?;
        let authority_snapshots =
            crate::project_authority_repo::count_snapshots(&self.pool).await?;
        let authority_snapshots_expired =
            crate::project_authority_repo::count_expired(&self.pool, now_unix()).await?;
        let active_leases_expired = count(
            &self.pool,
            "SELECT COUNT(*) FROM execution_task_leases WHERE status = 'active' AND expires_at <= strftime('%s','now')",
        )
        .await?;
        let tasks_missing_execution_spec = count(
            &self.pool,
            "SELECT COUNT(*) FROM task_runs WHERE status IN ('queued', 'running') AND finished_at IS NULL AND execution_spec_json IS NULL",
        )
        .await?;
        let schema_version =
            sqlx::query_scalar::<_, String>("SELECT value FROM schema_meta WHERE key = 'version'")
                .fetch_one(&self.pool)
                .await?
                .parse::<i64>()
                .map_err(|error| {
                    StoreError::Invariant(format!("invalid schema version: {error}"))
                })?;
        let ready = runtime_resume_pending == 0
            && recovery_pending == 0
            && active_leases_expired == 0
            && tasks_missing_execution_spec == 0
            && authority_snapshots_expired == 0
            && in_flight_runs == 0;
        Ok(OperationalDoctorReport {
            checked_at: now_unix(),
            workers_online,
            workers_draining,
            workers_offline,
            active_leases,
            runtime_running,
            runtime_resume_pending,
            recovery_pending,
            artifacts,
            audit_events,
            matrix_rooms,
            matrix_events,
            projects,
            queued_tasks,
            in_flight_runs,
            authority_snapshots,
            authority_snapshots_expired,
            active_leases_expired,
            tasks_missing_execution_spec,
            schema_version,
            ready,
        })
    }

    /// Capture the durable cutover/rollback inventory without changing any
    /// lease, cursor, runtime, or artifact state.
    pub async fn cutover_inventory(&self) -> Result<CutoverInventory, StoreError> {
        let report = self.check().await?;
        Ok(CutoverInventory {
            captured_at: report.checked_at,
            workers: WorkerInventory {
                online: report.workers_online,
                draining: report.workers_draining,
                offline: report.workers_offline,
            },
            active_leases: report.active_leases,
            runtime_running: report.runtime_running,
            runtime_resume_pending: report.runtime_resume_pending,
            matrix_rooms: report.matrix_rooms,
            matrix_events: report.matrix_events,
            artifacts: report.artifacts,
            queued_tasks: report.queued_tasks,
            in_flight_runs: report.in_flight_runs,
            ready_for_cutover: report.ready
                && report.workers_draining == 0
                && report.active_leases == 0
                && report.runtime_running == 0,
            rollback_requires_new_lease_epoch: true,
        })
    }

    pub async fn remediate(
        &self,
        observed_at: i64,
        heartbeat_timeout_secs: i64,
    ) -> Result<OperationalRemediationReport, StoreError> {
        let workers_marked_offline = crate::worker_repo::mark_stale_workers_offline(
            &self.pool,
            observed_at.saturating_sub(heartbeat_timeout_secs),
        )
        .await?;
        let leases_expired = {
            use agentd_core::ports::TaskLeasePort;
            crate::task_lease_control_plane::SqliteTaskLeaseControlPlane::new(self.pool.clone())
                .expire_due(observed_at)
                .await
                .map_err(|error| StoreError::Invariant(error.to_string()))?
        };
        Ok(OperationalRemediationReport {
            observed_at,
            workers_marked_offline,
            leases_expired,
        })
    }
}

async fn count(pool: &SqlitePool, query: &str) -> Result<i64, StoreError> {
    Ok(sqlx::query_scalar(query).fetch_one(pool).await?)
}
