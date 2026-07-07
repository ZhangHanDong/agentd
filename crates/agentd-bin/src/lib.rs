//! `agentd_bin` — the daemon's library surface (P0.9). The composition root: it
//! may depend on the store/tmux/mempal adapters (unlike `agentd-surface`, which
//! stays store-free, P0.7 D2). `main.rs` is the thin entrypoint over this lib so
//! the assembly is integration-testable.

#![warn(clippy::unwrap_used, clippy::panic)]

pub mod agent_mcp_context;
pub mod cli;
pub mod clock;
pub mod daemon;
pub mod host;
pub mod mempal;
pub mod stdio_mcp;

pub use cli::{AgentdCli, AgentdCommand, CleanupWorktreesArgs, DaemonConfig};
pub use clock::SystemClock;
pub use host::ProductionRunHost;
pub use mempal::OfflineMempal;
