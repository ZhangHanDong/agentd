//! Authenticated worker-fleet protocol boundary.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::ports::task_lease::TaskLeasePort;
use crate::types::{TaskLeaseGrant, WorkerId, WorkerIncarnationId};

/// Wire protocol version this daemon build speaks to the fleet. Bumped when a
/// registration/pull/heartbeat contract changes in a way workers must match.
pub const WORKER_PROTOCOL_VERSION: u32 = 1;

/// Lowest protocol version the daemon will accept a registration from. A
/// worker below this floor is rejected at registration (version negotiation).
pub const MIN_WORKER_PROTOCOL_VERSION: u32 = 1;

fn default_worker_capacity() -> u32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerFleetRegisterRequest {
    pub auth_proof: String,
    pub worker_id: WorkerId,
    pub trust_domain: String,
    pub labels: Value,
    pub incarnation_id: WorkerIncarnationId,
    pub daemon_version: String,
    pub host_name: String,
    pub network_zone: Option<String>,
    pub capabilities: Value,
    /// Maximum concurrent leases the daemon may grant this incarnation.
    #[serde(default = "default_worker_capacity")]
    pub capacity: u32,
    /// Worker's wire protocol version. An older peer that omits it deserializes
    /// as 0 and is rejected against `MIN_WORKER_PROTOCOL_VERSION`.
    #[serde(default)]
    pub protocol_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerFleetRegistration {
    pub worker_id: WorkerId,
    pub incarnation_id: WorkerIncarnationId,
    pub accepted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerFleetHeartbeat {
    pub auth_proof: String,
    pub worker_id: WorkerId,
    pub incarnation_id: WorkerIncarnationId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerFleetDrainRequest {
    pub auth_proof: String,
    pub worker_id: WorkerId,
    pub incarnation_id: WorkerIncarnationId,
    pub drain: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkerFleetPullRequest {
    pub auth_proof: String,
    pub worker_incarnation_id: WorkerIncarnationId,
    pub observed_at: i64,
    pub expires_at: i64,
    /// Optional idempotency key: replaying the same id returns the original
    /// grant. `None` derives a per-call key (no replay protection).
    #[serde(default)]
    pub request_id: Option<String>,
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

    async fn recover_offline(&self, heartbeat_cutoff: i64) -> Result<u64, WorkerFleetError>;

    async fn pull(
        &self,
        request: &WorkerFleetPullRequest,
    ) -> Result<Option<TaskLeaseGrant>, WorkerFleetError>;
}
