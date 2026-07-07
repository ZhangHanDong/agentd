//! `BackendError` (design §4.2) stays private to `agentd-tmux`; the public
//! boundary (`AgentBackend::spawn`) maps it to `CoreError::Backend` (P0.3 D2).

use agentd_core::CoreError;

/// Backend failure taxonomy (design §4.2). Inherent methods return this; the
/// `spawn` trait method maps it to [`CoreError::Backend`] at the public seam.
#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    /// The operation can be retried, or the caller should `rebind` instead.
    #[error("recoverable: {0}")]
    Recoverable(String),

    /// A multi-stage operation partially completed; `stage` names the step that
    /// did not finish so the caller knows what was and was not delivered.
    #[error("partial delivery at {stage}: {msg}")]
    PartialDelivery { stage: &'static str, msg: String },

    /// Unrecoverable: the environment is wrong (e.g. tmux is not installed).
    #[error("fatal: {0}")]
    Fatal(String),

    /// An internal expectation was violated (e.g. unparseable tmux output).
    #[error("invariant violated: {0}")]
    Invariant(String),
}

impl From<BackendError> for CoreError {
    fn from(err: BackendError) -> Self {
        CoreError::Backend(err.to_string())
    }
}
