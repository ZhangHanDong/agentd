//! The agent-spawning seam (design §4.1, §4.2). P0.1 needs only `spawn`; later
//! phases (P0.3) widen this trait with capture/status/shutdown. The tmux backend
//! implements it for real; `FakeBackend` implements it in memory for tests.

use crate::CoreError;
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
}
