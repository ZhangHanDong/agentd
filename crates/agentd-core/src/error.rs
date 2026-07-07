//! Crate-wide error. Per-subsystem error types in their modules; this one wraps them.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("dot parse error: {0}")]
    DotParse(String),

    #[error("graph validation error: {0}")]
    GraphValidate(String),

    #[error("unknown handler: {0}")]
    UnknownHandler(String),

    #[error("goal gate(s) not met: {0:?}")]
    GoalGateNotMet(Vec<String>),

    #[error("workflow sha changed since checkpoint; pass --accept-workflow-change to override")]
    WorkflowShaChanged,

    #[error("invariant violated: {0}")]
    Invariant(String),

    #[error("backend error: {0}")]
    Backend(String),

    #[error("store error: {0}")]
    Store(String),

    #[error("mempal error: {0}")]
    Mempal(String),

    #[error("checkpoint error: {0}")]
    Checkpoint(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),
}
