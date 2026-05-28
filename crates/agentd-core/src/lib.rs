//! `agentd-core` — pure-domain workflow engine and trait ports.
//!
//! See `docs/specs/2026-05-29-agentd-design.md` §1, §2, §4 for design, and
//! `docs/specs/2026-05-29-agentd-specify-boundary.md` for the agentd/Specify
//! role boundary (agentd is the local execution runtime).
//!
//! No I/O lives here. All side effects flow through `ports::` traits.

#![doc(html_root_url = "https://docs.rs/agentd-core/0.0.0")]
// Production-only lint opt-ins. Test files don't pick these up.
#![warn(clippy::unwrap_used, clippy::panic)]

// NOTE (build order): only modules that exist as of the current P0.1 task are
// declared here. Later tasks add their own `pub mod` line when they create the
// module: Task 7 → handler.
pub mod dot;
pub mod engine;
pub mod error;
pub mod graph;
pub mod ports;
pub mod types;

// In-memory fakes for the `ports` traits. Gated so they never ship in a release
// binary; agentd-core's own integration tests + examples see them via the
// `test-support` dev-dependency on itself (see Cargo.toml).
#[cfg(any(feature = "test-support", test))]
pub mod test_support;

pub use engine::{EngineEvent, HandlerStep, ParkReason, RunProgress};
pub use error::CoreError;
