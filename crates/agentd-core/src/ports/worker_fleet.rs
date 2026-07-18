//! Authenticated worker-fleet protocol boundary.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::ports::task_lease::TaskLeasePort;
use crate::types::{WorkerId, WorkerIncarnationId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerFleetRegisterRequest {
    pub worker_id: WorkerId,
    pub trust_domain: String,
    pub labels: Value,
    pub incarnation_id: WorkerIncarnationId,
    pub daemon_version: String,
    pub host_name: String,
    pub network_zone: Option<String>,
    pub capabilities: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerFleetRegistration {
    pub worker_id: WorkerId,
    pub incarnation_id: WorkerIncarnationId,
    pub accepted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerFleetHeartbeat {
    pub worker_id: WorkerId,
    pub incarnation_id: WorkerIncarnationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerFleetDrainRequest {
    pub worker_id: WorkerId,
    pub incarnation_id: WorkerIncarnationId,
    pub drain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerFleetHeartbeatResult {
    Accepted { last_seen_at: i64 },
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WorkerFleetError {
    #[error("worker fleet input is invalid: {0}")]
    Invalid(String),
    #[error("worker fleet resource not found: {0}")]
    NotFound(String),
    #[error("worker fleet conflict: {0}")]
    Conflict(String),
    #[error("worker fleet is unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait WorkerFleetPort: TaskLeasePort + Send + Sync {
    async fn register(
        &self,
        request: &WorkerFleetRegisterRequest,
    ) -> Result<WorkerFleetRegistration, WorkerFleetError>;

    async fn heartbeat(
        &self,
        request: &WorkerFleetHeartbeat,
    ) -> Result<WorkerFleetHeartbeatResult, WorkerFleetError>;

    async fn set_drain(&self, request: &WorkerFleetDrainRequest) -> Result<(), WorkerFleetError>;
}
