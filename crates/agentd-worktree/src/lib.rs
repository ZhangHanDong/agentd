//! Git worktree allocation independent from process/runtime ownership.

#![warn(clippy::unwrap_used, clippy::panic)]

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use agentd_core::CoreError;
use agentd_core::ports::WorktreeAllocator;

#[async_trait::async_trait]
pub trait WorktreeProvider: Send + Sync {
    async fn create(&self, name: &str) -> Result<PathBuf, CoreError>;
    async fn remove(&self, path: &Path) -> Result<(), CoreError>;
    async fn list(&self) -> Result<Vec<PathBuf>, CoreError>;
}

pub struct WorktreePool {
    provider: Arc<dyn WorktreeProvider>,
    counter: AtomicU64,
    pid: u32,
}

impl std::fmt::Debug for WorktreePool {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorktreePool")
            .field("pid", &self.pid)
            .finish_non_exhaustive()
    }
}

impl WorktreePool {
    #[must_use]
    pub fn new(provider: Arc<dyn WorktreeProvider>) -> Self {
        Self {
            provider,
            counter: AtomicU64::new(0),
            pid: std::process::id(),
        }
    }

    pub async fn allocate_unkeyed(&self) -> Result<PathBuf, CoreError> {
        let sequence = self.counter.fetch_add(1, Ordering::Relaxed);
        self.provider
            .create(&format!("wt-{}-{sequence}", self.pid))
            .await
    }

    pub async fn gc_on_boot(&self) -> Result<(), CoreError> {
        self.gc_on_boot_preserving(std::iter::empty::<PathBuf>())
            .await
    }

    pub async fn gc_on_boot_preserving<I>(&self, preserve_paths: I) -> Result<(), CoreError>
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let preserve_names: HashSet<OsString> = preserve_paths
            .into_iter()
            .filter_map(|path| path.file_name().map(std::ffi::OsStr::to_os_string))
            .collect();
        for path in self.provider.list().await? {
            if !path
                .file_name()
                .is_some_and(|name| preserve_names.contains(name))
            {
                self.provider.remove(&path).await?;
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl WorktreeAllocator for WorktreePool {
    async fn allocate(&self, key: &str) -> Result<PathBuf, CoreError> {
        self.provider.create(&pool_name_for_key(key)?).await
    }

    async fn allocate_snapshot(&self, key: &str, source: &Path) -> Result<PathBuf, CoreError> {
        let path = self.provider.create(&pool_name_for_key(key)?).await?;
        if let Err(error) = sync_snapshot(source, &path) {
            let _ = self.provider.remove(&path).await;
            return Err(CoreError::Backend(format!(
                "sync reviewer snapshot {} -> {} failed: {error}",
                source.display(),
                path.display()
            )));
        }
        Ok(path)
    }

    async fn release(&self, key: &str, path: &Path) -> Result<(), CoreError> {
        let expected = pool_name_for_key(key)?;
        if path.file_name().and_then(|name| name.to_str()) != Some(expected.as_str()) {
            return Err(CoreError::Invariant(format!(
                "release path {} does not match pool name {expected}",
                path.display()
            )));
        }
        self.provider.remove(path).await
    }
}

#[derive(Debug)]
pub struct GitWorktreeProvider {
    repo: PathBuf,
    base: PathBuf,
}

impl GitWorktreeProvider {
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
            .map_err(|error| {
                CoreError::Backend(format!("git {args:?} failed to spawn: {error}"))
            })
    }
}

#[async_trait::async_trait]
impl WorktreeProvider for GitWorktreeProvider {
    async fn create(&self, name: &str) -> Result<PathBuf, CoreError> {
        if !is_pool_name(name) {
            return Err(CoreError::Invariant(format!(
                "refusing non-pool worktree name {name:?}"
            )));
        }
        let path = self.base.join(name);
        let path_text = path.to_string_lossy().into_owned();
        let output = self
            .git(&["worktree", "add", "--detach", &path_text, "HEAD"])
            .await?;
        if !output.status.success() {
            return Err(CoreError::Backend(format!(
                "git worktree add {path_text} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        if let Err(error) = validate_git_worktree_root(&path).await {
            let _ = self.remove(&path).await;
            return Err(error);
        }
        Ok(path)
    }

    async fn remove(&self, path: &Path) -> Result<(), CoreError> {
        let name = path.file_name().and_then(|name| name.to_str());
        if !name.is_some_and(is_pool_name) {
            return Err(CoreError::Invariant(format!(
                "refusing to remove non-pool worktree {}",
                path.display()
            )));
        }
        let path_text = path.to_string_lossy().into_owned();
        let output = self
            .git(&["worktree", "remove", "--force", &path_text])
            .await?;
        if !output.status.success() {
            return Err(CoreError::Backend(format!(
                "git worktree remove {path_text} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    async fn list(&self) -> Result<Vec<PathBuf>, CoreError> {
        let output = self.git(&["worktree", "list", "--porcelain"]).await?;
        if !output.status.success() {
            return Err(CoreError::Backend(format!(
                "git worktree list failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(pool_worktrees(&String::from_utf8_lossy(&output.stdout)))
    }
}

fn pool_worktrees(porcelain: &str) -> Vec<PathBuf> {
    porcelain
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(PathBuf::from)
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
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

fn is_pool_name(name: &str) -> bool {
    if is_task_keyed_pool_name(name) || is_reviewer_keyed_pool_name(name) {
        return true;
    }
    let Some(rest) = name.strip_prefix("wt-") else {
        return false;
    };
    let mut parts = rest.split('-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(pid), Some(sequence), None) => {
            !pid.is_empty()
                && !sequence.is_empty()
                && pid.bytes().all(|byte| byte.is_ascii_digit())
                && sequence.bytes().all(|byte| byte.is_ascii_digit())
        }
        _ => false,
    }
}

fn is_task_keyed_pool_name(name: &str) -> bool {
    name.strip_prefix("wt-task-tr_").is_some_and(valid_ulid)
}

fn is_reviewer_keyed_pool_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("wt-review-rr_") else {
        return false;
    };
    let Some((ulid, reviewer)) = rest.split_once('-') else {
        return false;
    };
    valid_ulid(ulid)
        && !reviewer.is_empty()
        && reviewer
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn valid_ulid(value: &str) -> bool {
    value.len() == 26
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
}

async fn validate_git_worktree_root(path: &Path) -> Result<(), CoreError> {
    if !path.join(".git").exists() {
        return Err(CoreError::Backend(format!(
            "not a git worktree root (missing .git metadata): {}",
            path.display()
        )));
    }
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .await
        .map_err(|error| CoreError::Backend(format!("git rev-parse failed: {error}")))?;
    if !output.status.success() {
        return Err(CoreError::Backend(format!(
            "not a git worktree root (git rev-parse failed): {}",
            path.display()
        )));
    }
    let actual_text = String::from_utf8_lossy(&output.stdout);
    let actual_text = actual_text.trim();
    let expected = fs::canonicalize(path).map_err(|error| {
        CoreError::Backend(format!(
            "canonicalize worktree {} failed: {error}",
            path.display()
        ))
    })?;
    let actual = fs::canonicalize(actual_text).map_err(|error| {
        CoreError::Backend(format!(
            "canonicalize git top-level {actual_text} failed: {error}"
        ))
    })?;
    if actual != expected {
        return Err(CoreError::Backend(format!(
            "not a git worktree root: {} resolves to {}",
            path.display(),
            actual.display()
        )));
    }
    Ok(())
}

fn sync_snapshot(source: &Path, destination: &Path) -> io::Result<()> {
    if !source.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("source is not a directory: {}", source.display()),
        ));
    }
    fs::create_dir_all(destination)?;
    sync_dir_contents(source, destination)
}

fn sync_dir_contents(source: &Path, destination: &Path) -> io::Result<()> {
    remove_absent_entries(source, destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
    }
    Ok(())
}

fn remove_absent_entries(source: &Path, destination: &Path) -> io::Result<()> {
    for entry in fs::read_dir(destination)? {
        let entry = entry?;
        if entry.file_name() != ".git" && !source.join(entry.file_name()).exists() {
            remove_path(&entry.path())?;
        }
    }
    Ok(())
}

fn copy_entry(source: &Path, destination: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(source)?;
    if metadata.file_type().is_symlink() {
        replace_with_symlink(source, destination)
    } else if metadata.is_dir() {
        if destination.exists() && !destination.is_dir() {
            remove_path(destination)?;
        }
        fs::create_dir_all(destination)?;
        sync_dir_contents(source, destination)
    } else {
        if destination.exists() {
            remove_path(destination)?;
        }
        fs::copy(source, destination).map(|_| ())
    }
}

fn remove_path(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

#[cfg(unix)]
fn replace_with_symlink(source: &Path, destination: &Path) -> io::Result<()> {
    if destination.exists() {
        remove_path(destination)?;
    }
    std::os::unix::fs::symlink(fs::read_link(source)?, destination)
}

#[cfg(not(unix))]
fn replace_with_symlink(_source: &Path, _destination: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "snapshotting symlinks is unsupported on this platform",
    ))
}
