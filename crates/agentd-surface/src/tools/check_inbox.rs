//! `check_inbox` (design §4.12.1): pull cowork-bus messages for an agent. v0
//! returns an empty inbox — the cowork-bus pull is not on the frozen
//! `MempalClient` port (D5) and the standalone MVP tolerates no peer messages;
//! the real pull lands with the daemon.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SurfaceError;
use crate::host::RunHost;

#[derive(Debug, Clone, Deserialize)]
pub struct CheckInboxInput {
    pub agent_id: String,
    #[serde(default)]
    pub drain: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckInboxOutput {
    pub messages: Vec<Value>,
}

/// Pull the agent's inbox.
///
/// # Errors
/// Never in v0 — kept fallible/async for a uniform tool signature in the dispatcher.
#[allow(clippy::unused_async)] // uniform async tool signature; the real pull awaits mempal
pub async fn check_inbox(
    _host: &dyn RunHost,
    _input: CheckInboxInput,
) -> Result<CheckInboxOutput, SurfaceError> {
    Ok(CheckInboxOutput {
        messages: Vec::new(),
    })
}
