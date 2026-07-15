//! `submit_human_answer`: local operator answer for a parked `wait.human` node.
//! It exposes the existing `HumanAnswered` engine event over the MCP seam.

use agentd_core::EngineEvent;
use agentd_core::RunProgress;
use serde::{Deserialize, Serialize};

use crate::error::SurfaceError;
use crate::host::RunHost;

#[derive(Debug, Clone, Deserialize)]
pub struct SubmitHumanAnswerInput {
    pub wait_id: String,
    pub answer: String,
    #[serde(default)]
    pub feedback: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubmitHumanAnswerOutput {
    pub accepted: bool,
    pub next_node: Option<String>,
}

/// Submit a human answer through the host.
///
/// # Errors
/// [`SurfaceError::AlreadySubmitted`] when the wait was already answered or the
/// park moved; [`SurfaceError::Internal`] on host failure.
pub async fn submit_human_answer(
    host: &dyn RunHost,
    input: SubmitHumanAnswerInput,
) -> Result<SubmitHumanAnswerOutput, SurfaceError> {
    let progress = host
        .deliver(EngineEvent::HumanAnswered {
            wait_id: input.wait_id,
            answer: input.answer,
            feedback: input.feedback,
        })
        .await?;

    match progress {
        RunProgress::Ignored { .. } => Err(SurfaceError::AlreadySubmitted),
        RunProgress::Parked { node_id, .. } => Ok(SubmitHumanAnswerOutput {
            accepted: true,
            next_node: Some(node_id.to_string()),
        }),
        RunProgress::Finished { .. } | RunProgress::Failed { .. } => Ok(SubmitHumanAnswerOutput {
            accepted: true,
            next_node: None,
        }),
    }
}
