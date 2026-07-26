//! `check_inbox` (design §4.12.1): pull durable direct messages and group
//! mentions for an agent.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SurfaceError;
use crate::host::RunHost;

#[derive(Debug, Clone, Deserialize)]
pub struct CheckInboxInput {
    pub agent_id: String,
    /// Whether this read advances the agent's durable read cursor. Defaults to
    /// `true`, matching agent-chat's `GET /api/inbox/:agent`. Forced to `false`
    /// whenever `kinds` is set: a single cursor cannot advance over one kind
    /// without silently skipping unread messages of every other kind.
    #[serde(default = "default_drain")]
    pub drain: bool,
    /// Optional `schema.kind` filter. When non-empty the read is a preview.
    #[serde(default)]
    pub kinds: Vec<String>,
}

const fn default_drain() -> bool {
    true
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
    let kinds = input
        .kinds
        .iter()
        .map(|kind| kind.trim())
        .filter(|kind| !kind.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let advance = kinds.is_empty() && input.drain;

    let messages = host.check_inbox(&input.agent_id, advance).await?;
    let encoded = messages
        .into_iter()
        .filter(|message| matches_kinds(message.schema.as_ref(), &kinds))
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

/// agent-chat's `messageMatchesKinds`: an empty filter matches everything; a
/// non-empty filter matches only messages carrying a listed `schema.kind`.
fn matches_kinds(schema: Option<&Value>, kinds: &[String]) -> bool {
    if kinds.is_empty() {
        return true;
    }
    schema
        .and_then(|schema| schema.get("kind"))
        .and_then(Value::as_str)
        .is_some_and(|kind| kinds.iter().any(|wanted| wanted == kind))
}
