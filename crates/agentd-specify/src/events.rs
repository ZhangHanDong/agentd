//! Pure mapping from local agentd run events to Specify semantic events.

use serde_json::{Value, json};

use crate::{SemanticEvent, SpecifyError};

/// Specify event kind for a local run park that needs external progress.
pub const SPECIFY_AGENT_BLOCKED: &str = "agent.blocked";
/// Specify event kind for a completed local workflow run.
pub const SPECIFY_WORKFLOW_FINISHED: &str = "workflow.finished";
/// Specify event kind for a failed local workflow run.
pub const SPECIFY_WORKFLOW_FAILED: &str = "workflow.failed";

/// Borrowed reference to an agentd durable event row.
///
/// The mapper keeps this crate decoupled from surface/runtime crates by taking
/// only the stable fields it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentdEventRef<'a> {
    /// Local run id.
    pub run_id: &'a str,
    /// Durable event sequence number.
    pub seq: i64,
    /// Local event kind such as `run_parked`.
    pub kind: &'a str,
    /// Original compact JSON payload.
    pub payload: &'a str,
}

/// Map a local durable event row into the Specify semantic-event vocabulary.
///
/// Unknown local event kinds return `Ok(None)` so optional Specify reporting can
/// ignore dashboard-only or future events without breaking standalone mode.
pub fn map_agentd_event(
    workflow_id: &str,
    event: AgentdEventRef<'_>,
) -> Result<Option<SemanticEvent>, SpecifyError> {
    let specify_kind = match event.kind {
        "run_parked" => SPECIFY_AGENT_BLOCKED,
        "run_finished" => SPECIFY_WORKFLOW_FINISHED,
        "run_failed" => SPECIFY_WORKFLOW_FAILED,
        _ => return Ok(None),
    };

    let payload = decode_payload(&event)?;
    Ok(Some(SemanticEvent {
        workflow_id: workflow_id.to_owned(),
        kind: specify_kind.to_owned(),
        payload: semantic_payload(&event, &payload),
    }))
}

fn decode_payload(event: &AgentdEventRef<'_>) -> Result<Value, SpecifyError> {
    serde_json::from_str(event.payload).map_err(|err| {
        SpecifyError::Decode(format!(
            "agentd event payload for kind {} seq {}: {err}",
            event.kind, event.seq
        ))
    })
}

fn semantic_payload(event: &AgentdEventRef<'_>, payload: &Value) -> Value {
    json!({
        "run_id": event.run_id,
        "seq": event.seq,
        "agentd_event_kind": event.kind,
        "payload": payload,
    })
}
