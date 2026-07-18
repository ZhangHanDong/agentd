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
    pub ready: bool,
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
        let ready = runtime_resume_pending == 0 && recovery_pending == 0;
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
            ready,
        })
    }
}

async fn count(pool: &SqlitePool, query: &str) -> Result<i64, StoreError> {
    Ok(sqlx::query_scalar(query).fetch_one(pool).await?)
}
