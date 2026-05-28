//! The memory/cowork-bus seam. agentd never touches mempal's on-disk database
//! directly; it speaks to mempal through this port (a real impl lands in a
//! later phase via mempal's API/MCP). All calls are best-effort: failures map
//! to [`CoreError::Mempal`] and callers (e.g. a handler's `pre_tools`) tolerate
//! them rather than aborting the run.

use crate::CoreError;

/// A single search/fact-check hit from a mempal drawer.
#[derive(Debug, Clone, PartialEq)]
pub struct DrawerHit {
    pub drawer_id: String,
    pub body: String,
    pub score: f32,
}

/// Read/write access to the shared memory bus (design §5).
#[async_trait::async_trait]
pub trait MempalClient: Send + Sync {
    /// Semantic search within a `wing`, optionally narrowed by drawer `kind`.
    ///
    /// # Errors
    /// Returns [`CoreError::Mempal`] when the backing store is unreachable.
    async fn search(
        &self,
        query: &str,
        wing: &str,
        kind: &str,
    ) -> Result<Vec<DrawerHit>, CoreError>;

    /// Store a new drawer body under `wing`/`kind`.
    ///
    /// # Errors
    /// Returns [`CoreError::Mempal`] on write failure.
    async fn ingest(&self, wing: &str, kind: &str, body: &str) -> Result<(), CoreError>;

    /// Add a knowledge-graph triple.
    ///
    /// # Errors
    /// Returns [`CoreError::Mempal`] on write failure.
    async fn kg_add(&self, subject: &str, predicate: &str, object: &str) -> Result<(), CoreError>;

    /// Look up evidence for or against a claim.
    ///
    /// # Errors
    /// Returns [`CoreError::Mempal`] when the backing store is unreachable.
    async fn fact_check(&self, claim: &str) -> Result<Vec<DrawerHit>, CoreError>;
}
