//! Protocol value types for the optional Specify seam.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Issue context pulled from Specify for a draft workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueContext {
    /// Specify issue id, for example `ACME-742`.
    pub issue_id: String,
    /// Human-readable issue title.
    pub title: String,
    /// Issue body or assembled project context.
    pub body: String,
    /// Optional labels provided by Specify.
    pub labels: Vec<String>,
    /// Optional GitHub number when Specify reports a GitHub backing issue.
    pub github_number: Option<u64>,
}

/// Draft spec payload pushed back to Specify after the draft workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftSpec {
    /// Specify issue id the draft belongs to.
    pub issue_id: String,
    /// Specify spec id.
    pub spec_id: String,
    /// Markdown or agent-spec content.
    pub content: String,
}

/// Receipt returned after pushing a draft.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftReceipt {
    /// Specify spec id.
    pub spec_id: String,
    /// Draft version/id assigned by Specify.
    pub draft_id: String,
}

/// Frozen spec payload pulled from Specify for execute workflows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrozenSpec {
    /// Specify spec id.
    pub spec_id: String,
    /// Immutable Specify version.
    pub version: String,
    /// Frozen spec content.
    pub content: String,
}

/// Opaque semantic event payload sent to Specify.
///
/// P142 deliberately leaves `kind` as a string. The internal agentd
/// `EventRecord` -> Specify vocabulary mapping is the Δ8 follow-up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticEvent {
    /// Specify workflow id.
    pub workflow_id: String,
    /// Specify semantic event kind, for example `workflow.started`.
    pub kind: String,
    /// Event payload.
    pub payload: Value,
}

/// Acceptance result reported after execute workflows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptanceReport {
    /// Specify workflow id.
    pub workflow_id: String,
    /// Whether the local acceptance gate passed.
    pub accepted: bool,
    /// Optional PR URL opened by the local runtime.
    pub pr_url: Option<String>,
    /// Human-readable summary.
    pub summary: String,
}
