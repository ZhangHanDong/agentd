//! Error taxonomy for the optional Specify seam.

/// Failure taxonomy for Specify client operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpecifyError {
    /// The operation needs a configured Specify service, but the standalone
    /// offline seam is active.
    #[error("specify offline for operation {operation}")]
    Offline {
        /// Operation name.
        operation: &'static str,
    },

    /// A recording test double was asked for a response it was not scripted to
    /// return.
    #[error("missing scripted Specify response for operation {operation}")]
    MissingScriptedResponse {
        /// Operation name.
        operation: &'static str,
    },

    /// A future transport failed before returning a protocol value.
    #[error("specify transport: {0}")]
    Transport(String),

    /// A future transport returned a shape the seam could not decode.
    #[error("specify decode: {0}")]
    Decode(String),
}

impl SpecifyError {
    /// Stable machine-readable error code for surface mappings and tests.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Offline { .. } => "offline",
            Self::MissingScriptedResponse { .. } => "missing_scripted_response",
            Self::Transport(_) => "transport",
            Self::Decode(_) => "decode",
        }
    }
}
