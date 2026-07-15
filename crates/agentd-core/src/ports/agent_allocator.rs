//! Agent allocation seam for workflow handlers.
//!
//! The scheduler-backed implementation lives outside `agentd-core`; this trait
//! lets `codergen` and `parallel.fan_out` ask for a role/capability allocation
//! without depending on `SQLite` or daemon wiring.

use serde_json::Value;

use crate::CoreError;
use crate::types::{AgentId, NodeId, RunId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentAllocationRequest {
    pub run_id: RunId,
    pub node_id: NodeId,
    pub role: String,
    pub capability: Option<String>,
    pub task: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentAllocationStatus {
    Direct,
    Routed,
    Queued,
    Provision,
    Drained,
}

impl AgentAllocationStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Routed => "routed",
            Self::Queued => "queued",
            Self::Provision => "provision",
            Self::Drained => "drained",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentAllocation {
    pub requested_role: String,
    pub agent_id: AgentId,
    pub status: AgentAllocationStatus,
    pub tier: Option<String>,
    pub reservation_id: Option<String>,
    pub ticket: Option<String>,
    pub provisioned_name: Option<String>,
    pub runtime: Value,
}

impl AgentAllocation {
    #[must_use]
    pub fn direct(role: String) -> Self {
        Self {
            agent_id: AgentId::parsed(&role),
            requested_role: role,
            status: AgentAllocationStatus::Direct,
            tier: None,
            reservation_id: None,
            ticket: None,
            provisioned_name: None,
            runtime: Value::Object(serde_json::Map::new()),
        }
    }
}

#[async_trait::async_trait]
pub trait AgentAllocator: Send + Sync {
    async fn allocate(&self, req: AgentAllocationRequest) -> Result<AgentAllocation, CoreError>;

    async fn release(&self, _agent_id: &AgentId) -> Result<Option<AgentAllocation>, CoreError> {
        Ok(None)
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DirectAgentAllocator;

#[async_trait::async_trait]
impl AgentAllocator for DirectAgentAllocator {
    async fn allocate(&self, req: AgentAllocationRequest) -> Result<AgentAllocation, CoreError> {
        Ok(AgentAllocation::direct(req.role))
    }
}
