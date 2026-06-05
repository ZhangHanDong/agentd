//! Worktree pool (§7.3 P1.3): per-spawn isolated git worktrees via an
//! `AgentBackend` decorator, keeping the frozen core untouched (D1).
//!
//! The core hardcodes `SpawnRequest.worktree = "."` and the daemon serializes
//! nothing, so concurrent agents would collide in the repo root. [`PooledBackend`]
//! wraps the real backend and, when it sees the `"."` auto-sentinel, swaps in a
//! FRESH worktree from [`WorktreePool`]. No reuse → concurrent allocations are
//! inherently distinct (isolation by fresh allocation, not by a lock; reuse
//! would need the per-`task_run` release lifecycle that D1 defers to P2).
//! Cleanup is boot-GC only: the trait has no `kill`, and nothing pool-owned
//! survives a restart (in-flight runs re-spawn fresh on resume from checkpoint).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use agentd_core::CoreError;
use agentd_core::ports::AgentBackend;
use agentd_core::types::{AgentHandle, SpawnRequest};

/// The git-worktree operations the pool needs, behind a seam so the
/// allocation/GC logic is unit-testable over an in-memory fake (no git/tmux).
#[async_trait::async_trait]
pub trait WorktreeProvider: Send + Sync {
    /// Create an isolated worktree named `name`, returning its path.
    ///
    /// # Errors
    /// [`CoreError::Backend`] if the worktree cannot be created.
    async fn create(&self, name: &str) -> Result<PathBuf, CoreError>;

    /// Remove the worktree at `path`.
    ///
    /// # Errors
    /// [`CoreError::Backend`] if the worktree cannot be removed.
    async fn remove(&self, path: &Path) -> Result<(), CoreError>;

    /// List existing pool worktrees (for boot-GC).
    ///
    /// # Errors
    /// [`CoreError::Backend`] if the worktrees cannot be listed.
    async fn list(&self) -> Result<Vec<PathBuf>, CoreError>;
}

/// Allocates a FRESH isolated worktree per request, named `wt-{pid}-{counter}`
/// (a process-id prefix + a lock-free atomic counter). No reuse — so concurrent
/// allocations are inherently distinct without a lock (see `p6` spec).
pub struct WorktreePool {
    provider: Arc<dyn WorktreeProvider>,
    counter: AtomicU64,
    pid: u32,
}

impl std::fmt::Debug for WorktreePool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorktreePool")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

impl WorktreePool {
    /// Build a pool over `provider`.
    #[must_use]
    pub fn new(provider: Arc<dyn WorktreeProvider>) -> Self {
        Self {
            provider,
            counter: AtomicU64::new(0),
            pid: std::process::id(),
        }
    }

    /// Allocate a fresh isolated worktree. The `pid` prefix keeps names unique
    /// across daemon restarts even before boot-GC has run.
    ///
    /// # Errors
    /// Propagates the provider's [`CoreError::Backend`] on a create failure
    /// (never silently falls back to `"."` — that would re-introduce the
    /// collision this pool exists to prevent).
    pub async fn allocate(&self) -> Result<PathBuf, CoreError> {
        let n = self.counter.fetch_add(1, Ordering::Relaxed);
        let name = format!("wt-{}-{n}", self.pid);
        self.provider.create(&name).await
    }

    /// Boot-GC: remove every leftover pool worktree. Correct because nothing
    /// pool-owned outlives a restart — in-flight runs re-spawn fresh on resume.
    ///
    /// # Errors
    /// [`CoreError::Backend`] from the provider's `list`/`remove`.
    pub async fn gc_on_boot(&self) -> Result<(), CoreError> {
        for path in self.provider.list().await? {
            self.provider.remove(&path).await?;
        }
        Ok(())
    }
}

/// `AgentBackend` decorator: allocate + override the `"."` auto worktree per
/// spawn; any explicit worktree passes through unchanged.
pub struct PooledBackend<B: AgentBackend> {
    inner: B,
    pool: WorktreePool,
}

impl<B: AgentBackend> std::fmt::Debug for PooledBackend<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PooledBackend")
            .field("pool", &self.pool)
            .finish_non_exhaustive()
    }
}

impl<B: AgentBackend> PooledBackend<B> {
    /// Wrap `inner`, allocating worktrees from `pool`.
    #[must_use]
    pub fn new(inner: B, pool: WorktreePool) -> Self {
        Self { inner, pool }
    }
}

#[async_trait::async_trait]
impl<B: AgentBackend> AgentBackend for PooledBackend<B> {
    async fn spawn(&self, mut req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        // Frozen core always passes "." (the auto-sentinel); swap in a fresh
        // isolated worktree. An explicit path is respected as-is.
        if req.worktree == Path::new(".") {
            req.worktree = self.pool.allocate().await?;
        }
        self.inner.spawn(req).await
    }
}

/// The real provider: shells `git worktree add/remove/list` under `base` in
/// `repo`. Integration-level (not unit-tested; the fake covers the pool logic).
#[derive(Debug)]
pub struct GitWorktreeProvider {
    repo: PathBuf,
    base: PathBuf,
}

impl GitWorktreeProvider {
    /// Worktrees live under `base`, created from the repo at `repo`.
    #[must_use]
    pub fn new(repo: impl Into<PathBuf>, base: impl Into<PathBuf>) -> Self {
        Self {
            repo: repo.into(),
            base: base.into(),
        }
    }

    async fn git(&self, args: &[&str]) -> Result<std::process::Output, CoreError> {
        tokio::process::Command::new("git")
            .arg("-C")
            .arg(&self.repo)
            .args(args)
            .output()
            .await
            .map_err(|e| CoreError::Backend(format!("git {args:?} failed to spawn: {e}")))
    }
}

#[async_trait::async_trait]
impl WorktreeProvider for GitWorktreeProvider {
    async fn create(&self, name: &str) -> Result<PathBuf, CoreError> {
        let path = self.base.join(name);
        let path_str = path.to_string_lossy().into_owned();
        // --detach: a throwaway worktree at HEAD, no new branch.
        let out = self
            .git(&["worktree", "add", "--detach", &path_str, "HEAD"])
            .await?;
        if !out.status.success() {
            return Err(CoreError::Backend(format!(
                "git worktree add {path_str} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(path)
    }

    async fn remove(&self, path: &Path) -> Result<(), CoreError> {
        let path_str = path.to_string_lossy().into_owned();
        let out = self
            .git(&["worktree", "remove", "--force", &path_str])
            .await?;
        if !out.status.success() {
            return Err(CoreError::Backend(format!(
                "git worktree remove {path_str} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }

    async fn list(&self) -> Result<Vec<PathBuf>, CoreError> {
        let out = self.git(&["worktree", "list", "--porcelain"]).await?;
        if !out.status.success() {
            return Err(CoreError::Backend(format!(
                "git worktree list failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        // Porcelain: `worktree <path>` lines. Keep only POOL worktrees, matched
        // by the `wt-` dir-name prefix — robust to the relative/symlinked/
        // canonical path differences `git` reports (e.g. macOS /tmp →
        // /private/tmp) that a `base`-prefix match would silently miss.
        let text = String::from_utf8_lossy(&out.stdout);
        Ok(text
            .lines()
            .filter_map(|l| l.strip_prefix("worktree "))
            .map(PathBuf::from)
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("wt-"))
            })
            .collect())
    }
}
