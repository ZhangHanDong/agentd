//! `MempalError` (design §3.4) stays private to `agentd-mempal`; the public
//! boundary (`MempalClient`) maps it to `CoreError::Mempal` (P0.4 D5).

use std::time::Duration;

use agentd_core::CoreError;

/// Failure taxonomy for the mempal client. Internal helpers return this; the
/// `MempalClient` impl maps it to [`CoreError::Mempal`] at the public seam.
#[derive(Debug, thiserror::Error)]
pub enum MempalError {
    /// The MCP tool call could not be delivered.
    #[error("mempal transport: {0}")]
    Transport(String),

    /// A best-effort read exceeded its timeout (design §3.4).
    #[error("mempal timed out after {0:?}")]
    Timeout(Duration),

    /// A tool result did not match the expected shape.
    #[error("mempal decode: {0}")]
    Decode(String),
}

impl From<MempalError> for CoreError {
    fn from(err: MempalError) -> Self {
        CoreError::Mempal(err.to_string())
    }
}
