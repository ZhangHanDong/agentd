//! An in-memory [`AgentBackend`]. Records every `SpawnRequest` and returns a
//! synthetic, deterministic [`AgentHandle`].

use std::sync::Mutex;
use std::time::SystemTime;

use crate::CoreError;
use crate::ports::AgentBackend;
use crate::types::{AgentHandle, BackendKind, SpawnRequest};

/// Recording fake backend for tests.
#[derive(Debug, Default)]
pub struct FakeBackend {
    spawned: Mutex<Vec<SpawnRequest>>,
}

impl FakeBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every `SpawnRequest` seen so far, in order.
    #[must_use]
    pub fn spawned(&self) -> Vec<SpawnRequest> {
        self.spawned.lock().expect("spawned lock").clone()
    }
}

#[async_trait::async_trait]
impl AgentBackend for FakeBackend {
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        let agent_id = req.agent_id.clone();
        self.spawned.lock().expect("spawned lock").push(req);
        Ok(AgentHandle {
            address: format!("fake://{}", agent_id.as_str()),
            session_name: format!("agentd-{}", agent_id.as_str()),
            agent_id,
            backend: BackendKind::NativeRuntime,
            pane_id: Some("%0".to_string()),
            pid: Some(4242),
            spawned_at: SystemTime::UNIX_EPOCH,
        })
    }
}
