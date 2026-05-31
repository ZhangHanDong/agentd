//! `SurfaceError` — the MCP tool error taxonomy (design §4.12.1). Each variant
//! maps to the wire `code` agents see; a `CoreError` from the host becomes
//! `Internal`.

use agentd_core::CoreError;

/// A tool failure. `code()` is the §4.12.1 error code returned to the agent.
#[derive(Debug, thiserror::Error)]
pub enum SurfaceError {
    /// The engine has no pending task for this agent/(run,node).
    #[error("not_assigned")]
    NotAssigned,
    /// A second, conflicting submission for an already-answered slot.
    #[error("already_submitted")]
    AlreadySubmitted,
    /// The `(run, node, attempt)` park already moved — a stale/replayed submit.
    #[error("stale_attempt")]
    StaleAttempt,
    /// The referenced run does not exist.
    #[error("not_found")]
    NotFound,
    /// Anything else (a host/store failure), surfaced verbatim.
    #[error("internal: {0}")]
    Internal(String),
}

impl SurfaceError {
    /// The wire error code (design §4.12.1).
    #[must_use]
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotAssigned => "not_assigned",
            Self::AlreadySubmitted => "already_submitted",
            Self::StaleAttempt => "stale_attempt",
            Self::NotFound => "not_found",
            Self::Internal(_) => "internal",
        }
    }
}

impl From<CoreError> for SurfaceError {
    fn from(err: CoreError) -> Self {
        SurfaceError::Internal(err.to_string())
    }
}
