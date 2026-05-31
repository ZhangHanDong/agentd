//! `submit_outcome` (design §4.12.1): the strictly append-once tool. Resolve the
//! open task for `(run, node)` (the input carries `(run, node, attempt)` but the
//! engine routes by `task_run_id`), build an `Outcome`, and deliver it. A park
//! that already moved comes back as `RunProgress::Ignored` → `stale_attempt`.

use agentd_core::types::{NodeId, Outcome, RunId, Status};
use agentd_core::{EngineEvent, RunProgress};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::SurfaceError;
use crate::host::RunHost;

#[derive(Debug, Clone, Deserialize)]
pub struct SubmitOutcomeInput {
    pub run_id: String,
    pub node_id: String,
    pub attempt: u32,
    pub status: String,
    #[serde(default)]
    pub context_updates: Map<String, Value>,
    #[serde(default)]
    pub preferred_label: Option<String>,
    #[serde(default)]
    pub suggested_next: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubmitOutcomeOutput {
    pub recorded: bool,
    pub next_node: Option<String>,
}

/// Submit a node outcome through the host.
///
/// # Errors
/// [`SurfaceError::NotAssigned`] when there is no open task for `(run, node)`;
/// [`SurfaceError::StaleAttempt`] when the park already moved;
/// [`SurfaceError::Internal`] on an invalid status or a host failure.
pub async fn submit_outcome(
    host: &dyn RunHost,
    input: SubmitOutcomeInput,
) -> Result<SubmitOutcomeOutput, SurfaceError> {
    let run_id = RunId::from_string(input.run_id.as_str());
    let node_id = NodeId::parsed(input.node_id.as_str());

    let task = host
        .open_task(&run_id, &node_id)
        .await?
        .ok_or(SurfaceError::NotAssigned)?;

    let outcome = build_outcome(input)?;
    let progress = host
        .deliver(EngineEvent::AgentOutcomeSubmitted {
            task_run_id: task.task_run_id,
            outcome,
        })
        .await?;

    match progress {
        // The park is gone — a stale or replayed submission. Never re-deliver.
        RunProgress::Ignored { .. } => Err(SurfaceError::StaleAttempt),
        RunProgress::Parked { node_id, .. } => Ok(SubmitOutcomeOutput {
            recorded: true,
            next_node: Some(node_id.to_string()),
        }),
        RunProgress::Finished { .. } | RunProgress::Failed { .. } => Ok(SubmitOutcomeOutput {
            recorded: true,
            next_node: None,
        }),
    }
}

/// Build the engine `Outcome` from the tool input.
fn build_outcome(input: SubmitOutcomeInput) -> Result<Outcome, SurfaceError> {
    let status = match input.status.as_str() {
        "success" => Status::Success,
        "fail" => Status::Fail,
        "retry" => Status::Retry,
        "partial_success" => Status::PartialSuccess,
        other => return Err(SurfaceError::Internal(format!("invalid status {other:?}"))),
    };
    let mut outcome = Outcome::success();
    outcome.status = status;
    outcome.preferred_label = input.preferred_label;
    outcome.suggested_next_ids = input
        .suggested_next
        .into_iter()
        .map(NodeId::parsed)
        .collect();
    outcome.context_updates = input.context_updates;
    Ok(outcome)
}
