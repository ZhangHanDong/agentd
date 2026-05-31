//! `agentd-surface` — the daemon's outward surface (design §4.12.1 / §7.2): the
//! rmcp MCP server exposing the agentd tools to agents, plus the HTTP+SSE server
//! (7b). Each tool is a pure function over the [`host::RunHost`] seam, so it is
//! testable against a `FakeRunHost` with no real engine, MCP client, or socket;
//! the production `RunHost` (real `Engine` + store) is wired into the daemon in P0.9.

#![doc(html_root_url = "https://docs.rs/agentd-surface/0.0.0")]
// Production-only lint opt-ins. Test files don't pick these up.
#![warn(clippy::unwrap_used, clippy::panic)]

pub mod error;
pub mod host;
pub mod mcp_server;
pub mod tools;

// In-crate seam fake. Gated so it never ships in a release binary; the crate's
// own tests see it via the `test-support` dev-dependency.
#[cfg(any(feature = "test-support", test))]
pub mod test_support;

pub use error::SurfaceError;
pub use host::{RunHost, RunSnapshot, TaskAssignment};
