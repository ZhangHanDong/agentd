//! `submit_review` (design §4.12.1): a reviewer submits a verdict; the tool
//! reports how many reviewers the fan-in still waits on. A review run that has
//! already closed comes back as `RunProgress::Ignored` → `already_submitted`.

use agentd_core::types::{AgentId, ReviewRunId, VerdictValue};
use agentd_core::{EngineEvent, RunProgress};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::SurfaceError;
use crate::host::RunHost;

#[derive(Debug, Clone, Deserialize)]
pub struct SubmitReviewInput {
    pub review_run_id: String,
    pub reviewer_id: String,
    pub verdict: String,
    /// Structured MCP findings. The engine/store layer treats the serialized
    /// JSON as opaque text.
    #[serde(default)]
    pub findings: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubmitReviewOutput {
    pub accepted: bool,
    pub fan_in_pending: usize,
}

/// Submit a reviewer's verdict through the host.
///
/// # Errors
/// [`SurfaceError::AlreadySubmitted`] when the review run already closed;
/// [`SurfaceError::Internal`] on an invalid verdict or a host failure.
pub async fn submit_review(
    host: &dyn RunHost,
    input: SubmitReviewInput,
) -> Result<SubmitReviewOutput, SurfaceError> {
    let review_run_id = ReviewRunId::from_string(input.review_run_id.as_str());
    let reviewer_id = AgentId::parsed(input.reviewer_id.as_str());
    let verdict = match input.verdict.as_str() {
        "pass" => VerdictValue::Pass,
        "concern" => VerdictValue::Fail,
        "blocker" => VerdictValue::Block,
        other => return Err(SurfaceError::Internal(format!("invalid verdict {other:?}"))),
    };
    let findings = serde_json::to_string(&input.findings)
        .map_err(|err| SurfaceError::Internal(format!("serialize findings: {err}")))?;

    let progress = host
        .deliver(EngineEvent::ReviewVerdictSubmitted {
            review_run_id: review_run_id.clone(),
            reviewer_id,
            verdict,
            findings,
        })
        .await?;
    if let RunProgress::Ignored { .. } = progress {
        return Err(SurfaceError::AlreadySubmitted);
    }

    let (expected, got) = host.review_counts(&review_run_id).await?;
    Ok(SubmitReviewOutput {
        accepted: true,
        fan_in_pending: expected.saturating_sub(got),
    })
}
