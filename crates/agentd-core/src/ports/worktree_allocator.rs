//! Worktree allocation seam for per-task_run isolation (P2 C1' R3a).
//!
//! This is intentionally only the core port. Real git-backed allocation and
//! release are adapter/daemon concerns.

use std::path::{Path, PathBuf};

use crate::CoreError;

#[async_trait::async_trait]
pub trait WorktreeAllocator: Send + Sync + std::fmt::Debug {
    /// Allocate a worktree keyed by the task_run id.
    ///
    /// # Errors
    /// [`CoreError`] if allocation fails.
    async fn allocate(&self, key: &str) -> Result<PathBuf, CoreError>;

    /// Allocate a worktree keyed by a reviewer or task id and populate it from
    /// `source`.
    ///
    /// The default implementation preserves older fakes by delegating to
    /// [`Self::allocate`]; production allocators should override this when
    /// snapshot semantics matter.
    ///
    /// # Errors
    /// [`CoreError`] if allocation or snapshotting fails.
    async fn allocate_snapshot(&self, key: &str, _source: &Path) -> Result<PathBuf, CoreError> {
        self.allocate(key).await
    }

    /// Release the worktree previously allocated for `key`.
    ///
    /// # Errors
    /// [`CoreError`] if release fails.
    async fn release(&self, key: &str, path: &Path) -> Result<(), CoreError>;
}
