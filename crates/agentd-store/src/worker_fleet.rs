//! Local durable implementation of the worker-fleet registration boundary.

use agentd_core::ports::{
    TaskLeaseCloseRequest, TaskLeaseDispatchRequest, TaskLeaseError, TaskLeasePort,
    TaskLeaseRenewRequest, WorkerFleetDrainRequest, WorkerFleetError, WorkerFleetHeartbeat,
    WorkerFleetHeartbeatResult, WorkerFleetPort, WorkerFleetPullRequest,
    WorkerFleetRegisterRequest, WorkerFleetRegistration,
};
use agentd_core::types::{TaskLeaseGrant, WorkerStatus};
use sqlx::SqlitePool;

use crate::task_lease_control_plane::SqliteTaskLeaseControlPlane;
use crate::util::now_unix;
use crate::worker_repo::{self, WorkerCreate, WorkerHeartbeatOutcome, WorkerRegistration};

#[derive(Debug, Clone)]
pub struct SqliteWorkerFleet {
    pool: SqlitePool,
    expected_auth_proof: Option<String>,
}

impl SqliteWorkerFleet {
    #[must_use]
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            pool,
            expected_auth_proof: None,
        }
    }

    #[must_use]
    pub fn with_auth_proof(mut self, auth_proof: impl Into<String>) -> Self {
        self.expected_auth_proof = Some(auth_proof.into());
        self
    }

    fn authorize(&self, proof: &str) -> Result<(), WorkerFleetError> {
        if let Some(expected) = &self.expected_auth_proof {
            if proof != expected {
                return Err(WorkerFleetError::Unavailable(
                    "worker authentication failed".to_string(),
                ));
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl WorkerFleetPort for SqliteWorkerFleet {
    async fn register(
        &self,
        request: &WorkerFleetRegisterRequest,
    ) -> Result<WorkerFleetRegistration, WorkerFleetError> {
        self.authorize(&request.auth_proof)?;
        if request.trust_domain.trim().is_empty()
            || request.daemon_version.trim().is_empty()
            || request.host_name.trim().is_empty()
        {
            return Err(WorkerFleetError::Invalid(
                "trust_domain, daemon_version, and host_name are required".to_string(),
            ));
        }
        if worker_repo::get_worker(&self.pool, &request.worker_id)
            .await
            .map_err(storage_error)?
            .is_none()
        {
            worker_repo::create_worker(
                &self.pool,
                WorkerCreate {
                    id: request.worker_id.clone(),
                    trust_domain: request.trust_domain.clone(),
                    labels: request.labels.clone(),
                },
            )
            .await
            .map_err(storage_error)?;
        }
        worker_repo::register_incarnation(
            &self.pool,
            &request.worker_id,
            WorkerRegistration {
                id: request.incarnation_id.clone(),
                daemon_version: request.daemon_version.clone(),
                host_name: request.host_name.clone(),
                network_zone: request.network_zone.clone(),
                capabilities: request.capabilities.clone(),
            },
        )
        .await
        .map_err(storage_error)?;
        Ok(WorkerFleetRegistration {
            worker_id: request.worker_id.clone(),
            incarnation_id: request.incarnation_id.clone(),
            accepted_at: now_unix(),
        })
    }

    async fn heartbeat(
        &self,
        request: &WorkerFleetHeartbeat,
    ) -> Result<WorkerFleetHeartbeatResult, WorkerFleetError> {
        self.authorize(&request.auth_proof)?;
        match worker_repo::heartbeat_incarnation(
            &self.pool,
            &request.worker_id,
            &request.incarnation_id,
        )
        .await
        .map_err(storage_error)?
        {
            WorkerHeartbeatOutcome::Accepted(record) => Ok(WorkerFleetHeartbeatResult::Accepted {
                last_seen_at: record.last_seen_at,
            }),
            WorkerHeartbeatOutcome::Stale => Ok(WorkerFleetHeartbeatResult::Stale),
        }
    }

    async fn set_drain(&self, request: &WorkerFleetDrainRequest) -> Result<(), WorkerFleetError> {
        self.authorize(&request.auth_proof)?;
        let incarnation = worker_repo::get_incarnation(&self.pool, &request.incarnation_id)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| WorkerFleetError::NotFound(request.incarnation_id.to_string()))?;
        if incarnation.worker_id != request.worker_id || !incarnation.is_current {
            return Err(WorkerFleetError::Conflict(
                "stale worker incarnation".to_string(),
            ));
        }
        let target = if request.drain {
            WorkerStatus::Draining
        } else {
            WorkerStatus::Online
        };
        worker_repo::transition_worker_status(&self.pool, &request.worker_id, target)
            .await
            .map(|_| ())
            .map_err(storage_error)
    }

    async fn recover_offline(&self, heartbeat_cutoff: i64) -> Result<u64, WorkerFleetError> {
        worker_repo::mark_stale_workers_offline(&self.pool, heartbeat_cutoff)
            .await
            .map_err(storage_error)
    }

    async fn pull(
        &self,
        request: &WorkerFleetPullRequest,
    ) -> Result<Option<TaskLeaseGrant>, WorkerFleetError> {
        self.authorize(&request.auth_proof)?;
        let worker = worker_repo::get_incarnation(&self.pool, &request.worker_incarnation_id)
            .await
            .map_err(storage_error)?
            .ok_or_else(|| WorkerFleetError::NotFound(request.worker_incarnation_id.to_string()))?;
        if !worker.is_current {
            return Err(WorkerFleetError::Conflict(
                "stale worker incarnation".to_string(),
            ));
        }
        let task_id = sqlx::query_scalar::<_, String>(
            "SELECT t.id FROM task_runs t \
             WHERE t.finished_at IS NULL AND t.status = 'running' \
             AND NOT EXISTS (SELECT 1 FROM execution_task_leases l \
                 WHERE l.execution_task_id = t.id AND l.status = 'active') \
             ORDER BY t.started_at ASC, t.id ASC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| WorkerFleetError::Unavailable(error.to_string()))?;
        let Some(task_id) = task_id else {
            return Ok(None);
        };
        let grant = self
            .dispatch(&TaskLeaseDispatchRequest {
                execution_task_id: agentd_core::types::TaskRunId::from_string(task_id),
                worker_incarnation_id: request.worker_incarnation_id.clone(),
                observed_at: request.observed_at,
                expires_at: request.expires_at,
            })
            .await
            .map_err(|error| WorkerFleetError::Conflict(error.to_string()))?;
        Ok(Some(grant))
    }
}

#[async_trait::async_trait]
impl TaskLeasePort for SqliteWorkerFleet {
    async fn dispatch(
        &self,
        request: &TaskLeaseDispatchRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        SqliteTaskLeaseControlPlane::new(self.pool.clone())
            .dispatch(request)
            .await
    }

    async fn renew(
        &self,
        request: &TaskLeaseRenewRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        SqliteTaskLeaseControlPlane::new(self.pool.clone())
            .renew(request)
            .await
    }

    async fn release(
        &self,
        request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        SqliteTaskLeaseControlPlane::new(self.pool.clone())
            .release(request)
            .await
    }

    async fn cancel(
        &self,
        request: &TaskLeaseCloseRequest,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        SqliteTaskLeaseControlPlane::new(self.pool.clone())
            .cancel(request)
            .await
    }

    async fn validate_claim(
        &self,
        claim: &agentd_core::types::TaskLeaseClaim,
        observed_at: i64,
    ) -> Result<TaskLeaseGrant, TaskLeaseError> {
        SqliteTaskLeaseControlPlane::new(self.pool.clone())
            .validate_claim(claim, observed_at)
            .await
    }

    async fn expire_due(&self, observed_at: i64) -> Result<u64, TaskLeaseError> {
        SqliteTaskLeaseControlPlane::new(self.pool.clone())
            .expire_due(observed_at)
            .await
    }
}

fn storage_error(error: crate::StoreError) -> WorkerFleetError {
    WorkerFleetError::Unavailable(error.to_string())
}
