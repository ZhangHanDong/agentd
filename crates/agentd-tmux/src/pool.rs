//! Worktree pool (§7.3 P1.3): per-spawn isolated git worktrees via an
//! `AgentBackend` decorator plus the core `WorktreeAllocator` port.
//!
//! The legacy [`PooledBackend`] path still swaps `"."` for a fresh worktree.
//! The active P2 path allocates keyed implementer worktrees, and P104 adds
//! reviewer snapshot worktrees keyed by review run + reviewer id.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use agentd_core::CoreError;
use agentd_core::ports::{AgentBackend, WorktreeAllocator};
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

    /// Boot-GC: remove every leftover pool worktree when the caller has no
    /// durable store context to preserve.
    ///
    /// # Errors
    /// [`CoreError::Backend`] from the provider's `list`/`remove`.
    pub async fn gc_on_boot(&self) -> Result<(), CoreError> {
        self.gc_on_boot_preserving(std::iter::empty::<PathBuf>())
            .await
    }

    /// Boot-GC with durable preservation: remove only pool worktrees whose
    /// basename is not in `preserve_paths`.
    ///
    /// The provider list is already constrained to tight pool-owned names, so a
    /// basename preserve set is safe and also robust to git reporting canonical
    /// paths such as `/private/tmp/...` while the store contains `/tmp/...`.
    ///
    /// # Errors
    /// [`CoreError::Backend`] from the provider's `list`/`remove`.
    pub async fn gc_on_boot_preserving<I>(&self, preserve_paths: I) -> Result<(), CoreError>
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let preserve_names: HashSet<OsString> = preserve_paths
            .into_iter()
            .filter_map(|path| path.file_name().map(|name| name.to_os_string()))
            .collect();
        for path in self.provider.list().await? {
            let preserve = path
                .file_name()
                .is_some_and(|name| preserve_names.contains(name));
            if !preserve {
                self.provider.remove(&path).await?;
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl WorktreeAllocator for WorktreePool {
    async fn allocate(&self, key: &str) -> Result<PathBuf, CoreError> {
        let name = pool_name_for_key(key)?;
        self.provider.create(&name).await
    }

    async fn allocate_snapshot(&self, key: &str, source: &Path) -> Result<PathBuf, CoreError> {
        let name = pool_name_for_key(key)?;
        let path = self.provider.create(&name).await?;
        if let Err(err) = sync_snapshot(source, &path) {
            let _ = self.provider.remove(&path).await;
            return Err(CoreError::Backend(format!(
                "sync reviewer snapshot {} -> {} failed: {err}",
                source.display(),
                path.display()
            )));
        }
        Ok(path)
    }

    async fn release(&self, key: &str, path: &Path) -> Result<(), CoreError> {
        let expected = pool_name_for_key(key)?;
        let actual = path.file_name().and_then(|n| n.to_str());
        if actual != Some(expected.as_str()) {
            return Err(CoreError::Invariant(format!(
                "release path {} does not match pool name {expected}",
                path.display()
            )));
        }
        self.provider.remove(path).await
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
        Ok(pool_worktrees(&String::from_utf8_lossy(&out.stdout)))
    }
}

/// Parse `git worktree list --porcelain` and return ONLY pool worktrees — either
/// legacy `wt-<digits>-<digits>` names or task-keyed `wt-task-tr_<ULID>` names.
/// A TIGHT match feeds boot-GC's `git worktree remove --force`: a foreign
/// worktree — even one named `wt-feature` or `wt-task-feature` — is never
/// returned, so the `--force` delete can't touch a tree the pool did not create.
/// Matching by dir-name (not the reported path prefix) is also robust to the
/// canonical/symlinked paths git reports (e.g. macOS /tmp → /private/tmp).
fn pool_worktrees(porcelain: &str) -> Vec<PathBuf> {
    porcelain
        .lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(PathBuf::from)
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_pool_name)
        })
        .collect()
}

fn pool_name_for_key(key: &str) -> Result<String, CoreError> {
    let name = if key.starts_with("review-") {
        format!("wt-{key}")
    } else {
        format!("wt-task-{key}")
    };
    if is_task_keyed_pool_name(&name) || is_reviewer_keyed_pool_name(&name) {
        Ok(name)
    } else {
        Err(CoreError::Invariant(format!(
            "worktree allocator key {key:?} is not a supported pool key"
        )))
    }
}

/// True only for exact pool-owned shapes — NOT loose `wt-` or `wt-task-`
/// prefixes (which would match a human's worktree names).
fn is_pool_name(name: &str) -> bool {
    if is_task_keyed_pool_name(name) || is_reviewer_keyed_pool_name(name) {
        return true;
    }
    let Some(rest) = name.strip_prefix("wt-") else {
        return false;
    };
    let mut parts = rest.split('-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(pid), Some(n), None) => {
            !pid.is_empty()
                && !n.is_empty()
                && pid.bytes().all(|c| c.is_ascii_digit())
                && n.bytes().all(|c| c.is_ascii_digit())
        }
        _ => false,
    }
}

fn is_task_keyed_pool_name(name: &str) -> bool {
    let Some(ulid) = name.strip_prefix("wt-task-tr_") else {
        return false;
    };
    ulid.len() == 26
        && ulid
            .bytes()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
}

fn is_reviewer_keyed_pool_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("wt-review-rr_") else {
        return false;
    };
    let Some((ulid, reviewer)) = rest.split_once('-') else {
        return false;
    };
    ulid.len() == 26
        && ulid
            .bytes()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        && !reviewer.is_empty()
        && reviewer
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn sync_snapshot(source: &Path, dest: &Path) -> io::Result<()> {
    if !source.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("source is not a directory: {}", source.display()),
        ));
    }
    fs::create_dir_all(dest)?;
    sync_dir_contents(source, dest)
}

fn sync_dir_contents(source: &Path, dest: &Path) -> io::Result<()> {
    remove_dest_entries_absent_from_source(source, dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let src = entry.path();
        let dst = dest.join(&name);
        copy_entry(&src, &dst)?;
    }
    Ok(())
}

fn remove_dest_entries_absent_from_source(source: &Path, dest: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dest)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        if !source.join(&name).exists() {
            remove_path(&entry.path())?;
        }
    }
    Ok(())
}

fn copy_entry(source: &Path, dest: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() {
        replace_with_symlink(source, dest)
    } else if meta.is_dir() {
        if dest.exists() && !dest.is_dir() {
            remove_path(dest)?;
        }
        fs::create_dir_all(dest)?;
        sync_dir_contents(source, dest)
    } else {
        if dest.exists() {
            remove_path(dest)?;
        }
        let _ = fs::copy(source, dest)?;
        Ok(())
    }
}

fn remove_path(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() && !meta.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(unix)]
fn replace_with_symlink(source: &Path, dest: &Path) -> io::Result<()> {
    if dest.exists() {
        remove_path(dest)?;
    }
    std::os::unix::fs::symlink(fs::read_link(source)?, dest)
}

#[cfg(not(unix))]
fn replace_with_symlink(_source: &Path, _dest: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "snapshotting symlinks is unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::{is_pool_name, pool_worktrees};
    use std::path::PathBuf;

    #[test]
    fn pool_worktrees_keeps_only_pool_dirs_preserving_foreign() {
        // The main tree, a pool worktree, and a human's `my-feature` worktree.
        let porcelain = "worktree /repo\nHEAD aaa\n\n\
             worktree /repo/.agentd/worktrees/wt-4321-0\nHEAD bbb\ndetached\n\n\
             worktree /repo/my-feature\nHEAD ccc\nbranch refs/heads/my-feature\n";
        assert_eq!(
            pool_worktrees(porcelain),
            vec![PathBuf::from("/repo/.agentd/worktrees/wt-4321-0")],
            "only the wt-<pid>-<n> pool worktree; main tree + foreign 'my-feature' preserved"
        );
    }

    #[test]
    fn pool_worktrees_keeps_task_keyed_names_preserving_foreign() {
        let porcelain = "worktree /repo\nHEAD aaa\n\n\
             worktree /repo/.agentd/worktrees/wt-task-tr_0123456789ABCDEFGHJKMNPQRS\nHEAD bbb\ndetached\n\n\
             worktree /repo/.agentd/worktrees/wt-task-feature\nHEAD ccc\nbranch refs/heads/wt-task-feature\n";
        assert_eq!(
            pool_worktrees(porcelain),
            vec![PathBuf::from(
                "/repo/.agentd/worktrees/wt-task-tr_0123456789ABCDEFGHJKMNPQRS"
            )],
            "only the tight wt-task-tr_<ULID> pool worktree; human wt-task-feature preserved"
        );
    }

    #[test]
    fn pool_worktrees_keeps_reviewer_keyed_names_preserving_foreign() {
        let porcelain = "worktree /repo\nHEAD aaa\n\n\
             worktree /repo/.agentd/worktrees/wt-review-rr_0123456789ABCDEFGHJKMNPQRS-claude-sec\nHEAD bbb\ndetached\n\n\
             worktree /repo/.agentd/worktrees/wt-review-feature\nHEAD ccc\nbranch refs/heads/wt-review-feature\n";
        assert_eq!(
            pool_worktrees(porcelain),
            vec![PathBuf::from(
                "/repo/.agentd/worktrees/wt-review-rr_0123456789ABCDEFGHJKMNPQRS-claude-sec"
            )],
            "only the tight wt-review-rr_<ULID>-<reviewer> pool worktree; human wt-review-feature preserved"
        );
    }

    #[test]
    fn is_pool_name_is_tight_not_a_loose_prefix() {
        assert!(is_pool_name("wt-1-0"));
        assert!(is_pool_name("wt-99999-12"));
        assert!(is_pool_name("wt-task-tr_0123456789ABCDEFGHJKMNPQRS"));
        assert!(is_pool_name(
            "wt-review-rr_0123456789ABCDEFGHJKMNPQRS-claude-sec"
        ));
        assert!(!is_pool_name("wt-feature"), "loose prefix must NOT match");
        assert!(
            !is_pool_name("wt-task-feature"),
            "loose task prefix must NOT match"
        );
        assert!(
            !is_pool_name("wt-review-feature"),
            "loose review prefix must NOT match"
        );
        assert!(!is_pool_name("wt-1"), "needs both parts");
        assert!(!is_pool_name("wt-1-0-x"), "no extra parts");
        assert!(!is_pool_name("wt-1-"), "empty second part");
        assert!(!is_pool_name("my-feature"));
    }
}
