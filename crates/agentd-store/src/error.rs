//! Store-local error. Repos produce `StoreError`; the `ports::Store` trait impl
//! converts to `CoreError` via the `From` impl below.

use agentd_core::CoreError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("not found")]
    NotFound,

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("invariant violated: {0}")]
    Invariant(String),
}

// Legal under the orphan rule: the local `StoreError` is the `From` type
// parameter, so we may implement it for the foreign `CoreError`. Lets the
// `impl ports::Store for SqliteStore` methods return `CoreError` while repos
// produce `StoreError` and `?` auto-converts.
impl From<StoreError> for CoreError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound => CoreError::Store("not found".to_string()),
            StoreError::Conflict(m) => CoreError::Store(format!("conflict: {m}")),
            other => CoreError::Store(other.to_string()),
        }
    }
}
