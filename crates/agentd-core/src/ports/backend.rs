//! The agent-spawning seam (design §4.1, §4.2). P0.1 needs only `spawn`; later
//! phases widen this trait with allocation-aware dispatch. The native runtime
//! backend implements it for real; `FakeBackend` implements it in memory.

use crate::CoreError;
use crate::ports::agent_allocator::AgentAllocation;
use crate::types::{AgentHandle, SpawnRequest};

/// Launch and address agent processes. Kept to a single P0.1 method so the
/// engine and `codergen`/`fan_out` handlers can request reviewer/worker agents.
#[async_trait::async_trait]
pub trait AgentBackend: Send + Sync {
    /// Spawn an agent for `req`, returning a handle the caller can address.
    ///
    /// # Errors
    /// Returns [`CoreError::Backend`] when the agent cannot be launched.
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError>;

    /// Dispatch workflow work to an already-selected allocation.
    ///
    /// Backends that can reuse a live scheduler-selected agent may override
    /// this. The default remains spawn-compatible so older callers and fakes do
    /// not need allocation-specific behavior.
    ///
    /// # Errors
    /// Returns [`CoreError::Backend`] when the work cannot be dispatched.
    async fn dispatch_allocated(
        &self,
        req: SpawnRequest,
        _allocation: &AgentAllocation,
    ) -> Result<AgentHandle, CoreError> {
        self.spawn(req).await
    }
}
