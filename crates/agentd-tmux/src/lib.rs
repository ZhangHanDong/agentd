//! `agentd-tmux` — the real tmux-backed `AgentBackend` (design §4).
//!
//! The agentd-core `AgentBackend` trait stays spawn-only (P0.3 D1); the other
//! six capabilities (§4.6–§4.10) are inherent methods on [`TmuxBackend`] landed
//! across P0.3 Tasks 2–5. Every flow runs through an injected
//! `Arc<dyn CommandRunner>` so tests use a `FakeRunner` — no real tmux server.

#![doc(html_root_url = "https://docs.rs/agentd-tmux/0.0.0")]
// Production-only lint opt-ins. Test files don't pick these up.
#![warn(clippy::unwrap_used, clippy::panic)]

pub mod backend;
pub mod config;
pub mod discovery;
pub mod error;
pub mod runner;

pub use backend::{CaptureOpts, TmuxBackend};
pub use config::{Config, ReadyPatterns};
pub use error::BackendError;
pub use runner::TokioCommandRunner;
