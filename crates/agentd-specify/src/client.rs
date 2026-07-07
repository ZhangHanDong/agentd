//! Client trait and standalone offline implementation for Specify boundary Δ7.

use crate::error::SpecifyError;
use crate::types::SemanticEvent;
use crate::types::{AcceptanceReport, DraftReceipt, DraftSpec, FrozenSpec, IssueContext};

const OP_PULL_ISSUE_CONTEXT: &str = "pull_issue_context";
const OP_PUSH_DRAFT: &str = "push_draft";
const OP_PULL_FROZEN_SPEC: &str = "pull_frozen_spec";

/// Object-safe seam for outbound Specify operations.
#[async_trait::async_trait]
pub trait SpecifyClient: Send + Sync {
    /// Pull issue/project context for a draft workflow.
    async fn pull_issue_context(&self, issue_id: &str) -> Result<IssueContext, SpecifyError>;

    /// Push a draft spec produced by a draft workflow.
    async fn push_draft(&self, draft: DraftSpec) -> Result<DraftReceipt, SpecifyError>;

    /// Pull an immutable frozen spec for an execute workflow.
    async fn pull_frozen_spec(
        &self,
        spec_id: &str,
        version: &str,
    ) -> Result<FrozenSpec, SpecifyError>;

    /// Report a semantic workflow event.
    async fn report_event(&self, event: SemanticEvent) -> Result<(), SpecifyError>;

    /// Report the local acceptance result.
    async fn report_acceptance(&self, report: AcceptanceReport) -> Result<(), SpecifyError>;
}

/// No-network Specify client for standalone agentd.
#[derive(Debug, Clone, Copy, Default)]
pub struct OfflineSpecify;

impl OfflineSpecify {
    /// Build the no-network Specify client.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl SpecifyClient for OfflineSpecify {
    async fn pull_issue_context(&self, _issue_id: &str) -> Result<IssueContext, SpecifyError> {
        Err(SpecifyError::Offline {
            operation: OP_PULL_ISSUE_CONTEXT,
        })
    }

    async fn push_draft(&self, _draft: DraftSpec) -> Result<DraftReceipt, SpecifyError> {
        Err(SpecifyError::Offline {
            operation: OP_PUSH_DRAFT,
        })
    }

    async fn pull_frozen_spec(
        &self,
        _spec_id: &str,
        _version: &str,
    ) -> Result<FrozenSpec, SpecifyError> {
        Err(SpecifyError::Offline {
            operation: OP_PULL_FROZEN_SPEC,
        })
    }

    async fn report_event(&self, _event: SemanticEvent) -> Result<(), SpecifyError> {
        Ok(())
    }

    async fn report_acceptance(&self, _report: AcceptanceReport) -> Result<(), SpecifyError> {
        Ok(())
    }
}
