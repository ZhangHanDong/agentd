//! Recording Specify client for contract tests.

use std::sync::{Arc, Mutex};

use crate::client::SpecifyClient;
use crate::error::SpecifyError;
use crate::types::{
    AcceptanceReport, DraftReceipt, DraftSpec, FrozenSpec, IssueContext, SemanticEvent,
};

const OP_PULL_ISSUE_CONTEXT: &str = "pull_issue_context";
const OP_PUSH_DRAFT: &str = "push_draft";
const OP_PULL_FROZEN_SPEC: &str = "pull_frozen_spec";

/// A recorded Specify seam operation.
#[derive(Debug, Clone, PartialEq)]
pub enum SpecifyCall {
    /// `pull_issue_context(issue_id)`.
    PullIssueContext {
        /// Specify issue id.
        issue_id: String,
    },
    /// `push_draft(draft)`.
    PushDraft {
        /// Draft payload.
        draft: DraftSpec,
    },
    /// `pull_frozen_spec(spec_id, version)`.
    PullFrozenSpec {
        /// Specify spec id.
        spec_id: String,
        /// Frozen version.
        version: String,
    },
    /// `report_event(event)`.
    ReportEvent {
        /// Semantic event payload.
        event: SemanticEvent,
    },
    /// `report_acceptance(report)`.
    ReportAcceptance {
        /// Acceptance report payload.
        report: AcceptanceReport,
    },
}

#[derive(Debug, Default)]
struct RecordingState {
    calls: Vec<SpecifyCall>,
    issue_response: Option<IssueContext>,
    draft_response: Option<DraftReceipt>,
    frozen_response: Option<FrozenSpec>,
}

/// Test double for the Specify seam.
#[derive(Debug, Clone, Default)]
pub struct RecordingSpecifyClient {
    state: Arc<Mutex<RecordingState>>,
}

impl RecordingSpecifyClient {
    /// Build an empty recording client.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Script the issue-context response.
    #[must_use]
    pub fn with_issue_context(self, response: IssueContext) -> Self {
        self.state
            .lock()
            .expect("recording Specify mutex poisoned")
            .issue_response = Some(response);
        self
    }

    /// Script the draft receipt.
    #[must_use]
    pub fn with_draft_receipt(self, response: DraftReceipt) -> Self {
        self.state
            .lock()
            .expect("recording Specify mutex poisoned")
            .draft_response = Some(response);
        self
    }

    /// Script the frozen-spec response.
    #[must_use]
    pub fn with_frozen_spec(self, response: FrozenSpec) -> Self {
        self.state
            .lock()
            .expect("recording Specify mutex poisoned")
            .frozen_response = Some(response);
        self
    }

    /// Return all recorded calls in order.
    #[must_use]
    pub fn calls(&self) -> Vec<SpecifyCall> {
        self.state
            .lock()
            .expect("recording Specify mutex poisoned")
            .calls
            .clone()
    }
}

#[async_trait::async_trait]
impl SpecifyClient for RecordingSpecifyClient {
    async fn pull_issue_context(&self, issue_id: &str) -> Result<IssueContext, SpecifyError> {
        let mut state = self.state.lock().expect("recording Specify mutex poisoned");
        state.calls.push(SpecifyCall::PullIssueContext {
            issue_id: issue_id.to_string(),
        });
        state
            .issue_response
            .clone()
            .ok_or(SpecifyError::MissingScriptedResponse {
                operation: OP_PULL_ISSUE_CONTEXT,
            })
    }

    async fn push_draft(&self, draft: DraftSpec) -> Result<DraftReceipt, SpecifyError> {
        let mut state = self.state.lock().expect("recording Specify mutex poisoned");
        state.calls.push(SpecifyCall::PushDraft { draft });
        state
            .draft_response
            .clone()
            .ok_or(SpecifyError::MissingScriptedResponse {
                operation: OP_PUSH_DRAFT,
            })
    }

    async fn pull_frozen_spec(
        &self,
        spec_id: &str,
        version: &str,
    ) -> Result<FrozenSpec, SpecifyError> {
        let mut state = self.state.lock().expect("recording Specify mutex poisoned");
        state.calls.push(SpecifyCall::PullFrozenSpec {
            spec_id: spec_id.to_string(),
            version: version.to_string(),
        });
        state
            .frozen_response
            .clone()
            .ok_or(SpecifyError::MissingScriptedResponse {
                operation: OP_PULL_FROZEN_SPEC,
            })
    }

    async fn report_event(&self, event: SemanticEvent) -> Result<(), SpecifyError> {
        self.state
            .lock()
            .expect("recording Specify mutex poisoned")
            .calls
            .push(SpecifyCall::ReportEvent { event });
        Ok(())
    }

    async fn report_acceptance(&self, report: AcceptanceReport) -> Result<(), SpecifyError> {
        self.state
            .lock()
            .expect("recording Specify mutex poisoned")
            .calls
            .push(SpecifyCall::ReportAcceptance { report });
        Ok(())
    }
}
