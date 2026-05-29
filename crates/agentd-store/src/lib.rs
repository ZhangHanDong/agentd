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

pub mod error;
pub mod paths;
pub mod pool;
pub mod store;

pub use error::StoreError;
pub use store::SqliteStore;
