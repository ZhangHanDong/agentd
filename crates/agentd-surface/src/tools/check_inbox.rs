//! `check_inbox` (design §4.12.1): pull durable direct messages and group
//! mentions for an agent.

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
    pub dm: Vec<Value>,
    pub group: Vec<Value>,
}

/// Pull the agent's inbox.
///
/// # Errors
/// [`SurfaceError`] on host/store failures or JSON encoding failures.
pub async fn check_inbox(
    host: &dyn RunHost,
    input: CheckInboxInput,
) -> Result<CheckInboxOutput, SurfaceError> {
    let messages = host.check_inbox(&input.agent_id, input.drain).await?;
    let encoded = messages
        .into_iter()
        .map(|message| {
            serde_json::to_value(message)
                .map_err(|e| SurfaceError::Internal(format!("encode inbox message: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let dm = encoded
        .iter()
        .filter(|message| message.get("group").is_none_or(Value::is_null))
        .cloned()
        .collect::<Vec<_>>();
    let group = encoded
        .iter()
        .filter(|message| message.get("group").is_some_and(|value| !value.is_null()))
        .cloned()
        .collect::<Vec<_>>();
    Ok(CheckInboxOutput {
        messages: encoded,
        dm,
        group,
    })
}
