//! Review verdict types. Shared across layers — the `Store` port persists them,
//! the `ReviewVerdictSubmitted` engine event carries one, and `fan_in`
//! aggregates them — so they live in `types`, not in any single subsystem.

use crate::types::ids::AgentId;

/// A reviewer's vote in a fan-out review. `fan_in` aggregates these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerdictValue {
    Pass,
    Fail,
    Block,
}

/// One recorded review verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewVerdict {
    pub reviewer_id: AgentId,
    pub value: VerdictValue,
    /// Opaque reviewer findings text. The surface serializes structured MCP
    /// findings into a deterministic string; core stores and compares it.
    pub findings: String,
}
