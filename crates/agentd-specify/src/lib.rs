//! `agentd-specify` — the optional outbound Specify client seam (boundary Δ7).
//!
//! This crate intentionally starts with only a trait, protocol value types,
//! [`OfflineSpecify`], and a recording test double. Real HTTP/WS transport waits
//! for a concrete Specify API contract; standalone agentd must keep running
//! without any Specify service.

#![doc(html_root_url = "https://docs.rs/agentd-specify/0.0.0")]
// Production-only lint opt-ins. Test files don't pick these up.
#![warn(clippy::unwrap_used, clippy::panic)]

pub mod client;
pub mod error;
pub mod events;
pub mod types;

#[cfg(any(feature = "test-support", test))]
pub mod test_support;

pub use client::{OfflineSpecify, SpecifyClient};
pub use error::SpecifyError;
pub use events::{
    AgentdEventRef, SPECIFY_AGENT_BLOCKED, SPECIFY_WORKFLOW_FAILED, SPECIFY_WORKFLOW_FINISHED,
    map_agentd_event, report_agentd_event,
};
pub use types::{
    AcceptanceReport, DraftReceipt, DraftSpec, FrozenSpec, IssueContext, SemanticEvent,
};
