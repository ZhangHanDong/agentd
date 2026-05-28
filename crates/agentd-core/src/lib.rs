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

// NOTE (build order): only modules that exist as of P0.1 Task 1 are declared
// here. Later tasks add their own `pub mod` line when they create the module:
// Task 2 → dot, Task 3 → graph, Task 6.5 → ports + test_support, Task 7 → handler.
pub mod engine;
pub mod error;
pub mod types;

pub use engine::{EngineEvent, HandlerStep, ParkReason, RunProgress};
pub use error::CoreError;
