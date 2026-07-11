//! `agentd-store` — the real `SqliteStore` backing `agentd_core::ports::Store`.
//!
//! sqlx in runtime mode (no compile-time query macros, no `.sqlx` metadata, so
//! no `DATABASE_URL` at build time) — repos call the runtime `query`/`query_as`
//! functions. Migrations are embedded with `sqlx::migrate!` and applied on
//! `connect`. See `migrations/0001_init.sql` for the schema and the P0.1-trait
//! ↔ P0.2-schema reconciliation rationale.

#![doc(html_root_url = "https://docs.rs/agentd-store/0.0.0")]
// Production-only lint opt-ins. Test files don't pick these up.
#![warn(clippy::unwrap_used, clippy::panic)]

pub mod agent_chat_import;
pub mod agent_chat_task_graph_repo;
pub mod agent_chat_task_repo;
pub mod agent_profile_repo;
pub mod agent_repo;
pub mod agent_scheduler_repo;
pub mod artifact_repo;
pub mod checkpoint_repo;
pub mod error;
pub mod event_repo;
pub mod human_wait_repo;
pub mod matrix_bridge_repo;
pub mod message_repo;
pub mod outbox_repo;
pub mod outcome_repo;
pub mod paths;
pub mod pool;
pub mod project_repo;
pub mod relay_repo;
pub mod review_repo;
pub mod run_repo;
pub mod runtime_session_repo;
pub mod store;
mod store_impl;
pub mod task_repo;
mod util;
pub mod worker_repo;
pub mod worktree_cleanup_repo;

pub use error::StoreError;
pub use store::SqliteStore;
pub use worktree_cleanup_repo::{FailedWorktreeCleanupCandidate, FailedWorktreeKind};
