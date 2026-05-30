//! `agentd-mempal` — the mempal MCP client + outbox drainer + consistency check
//! (design §3.4/§4.12.2).
//!
//! agentd speaks to mempal ONLY through MCP tool calls (the [`transport::McpToolCaller`]
//! seam); it never touches mempal's on-disk database (§3.1). Every flow is
//! testable against a fake — no real rmcp or mempal server in v0 (the real
//! transport lands in P0.7).

#![doc(html_root_url = "https://docs.rs/agentd-mempal/0.0.0")]
// Production-only lint opt-ins. Test files don't pick these up.
#![warn(clippy::unwrap_used, clippy::panic)]

pub mod client;
pub mod drainer;
pub mod error;
pub mod transport;

// In-crate transport fake. Gated so it never ships in a release binary; the
// crate's own integration tests see it via the `test-support` dev-dependency.
#[cfg(any(feature = "test-support", test))]
pub mod test_support;

pub use client::{MempalConfig, MempalMcpClient};
pub use error::MempalError;
pub use transport::McpToolCaller;
