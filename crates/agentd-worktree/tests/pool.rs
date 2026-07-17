use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentd_core::CoreError;
use agentd_core::ports::WorktreeAllocator;
use agentd_worktree::{WorktreePool, WorktreeProvider};

#[derive(Debug, Default)]
struct RecordingProvider {
    paths: Mutex<Vec<PathBuf>>,
    removed: Mutex<Vec<PathBuf>>,
}

#[derive(Debug)]
struct FilesystemProvider {
    root: PathBuf,
    paths: Mutex<Vec<PathBuf>>,
    removed: Mutex<Vec<PathBuf>>,
}

#[async_trait::async_trait]
impl WorktreeProvider for FilesystemProvider {
    async fn create(&self, name: &str) -> Result<PathBuf, CoreError> {
        let path = self.root.join(name);
        std::fs::create_dir_all(&path).map_err(|error| CoreError::Backend(error.to_string()))?;
        self.paths.lock().expect("paths").push(path.clone());
        Ok(path)
    }

    async fn remove(&self, path: &Path) -> Result<(), CoreError> {
        if path.exists() {
            std::fs::remove_dir_all(path).map_err(|error| CoreError::Backend(error.to_string()))?;
        }
        self.removed
            .lock()
            .expect("removed")
            .push(path.to_path_buf());
        Ok(())
    }

    async fn list(&self) -> Result<Vec<PathBuf>, CoreError> {
        Ok(self.paths.lock().expect("paths").clone())
    }
}

#[tokio::test]
async fn pool_allocates_task_keyed_worktree_via_allocator_port() {
    let provider = Arc::new(RecordingProvider::default());
    let pool = WorktreePool::new(provider.clone());

    let path = WorktreeAllocator::allocate(&pool, "tr_0123456789ABCDEFGHJKMNPQRS")
        .await
        .expect("task worktree");
    assert_eq!(
        path,
        PathBuf::from("/pool/wt-task-tr_0123456789ABCDEFGHJKMNPQRS")
    );
    assert_eq!(provider.paths.lock().expect("paths").as_slice(), [path]);
}

#[tokio::test]
async fn pool_releases_task_keyed_worktree_via_allocator_port() {
    let provider = Arc::new(RecordingProvider::default());
    let pool = WorktreePool::new(provider.clone());
    let path = WorktreeAllocator::allocate(&pool, "tr_0123456789ABCDEFGHJKMNPQRS")
        .await
        .expect("task worktree");

    WorktreeAllocator::release(&pool, "tr_0123456789ABCDEFGHJKMNPQRS", &path)
        .await
        .expect("release task worktree");
    assert_eq!(provider.removed.lock().expect("removed").as_slice(), [path]);
}

#[tokio::test]
async fn pool_allocates_reviewer_keyed_snapshot_worktree() {
    let temporary = tempfile::tempdir().expect("temporary worktree fixture");
    let source = temporary.path().join("source");
    std::fs::create_dir_all(source.join("src")).expect("create source");
    std::fs::write(source.join("src/lib.rs"), "pub fn reviewed() {}\n").expect("write source");
    let provider = Arc::new(FilesystemProvider {
        root: temporary.path().join("reviews"),
        paths: Mutex::new(Vec::new()),
        removed: Mutex::new(Vec::new()),
    });
    let pool = WorktreePool::new(provider);

    let path = WorktreeAllocator::allocate_snapshot(
        &pool,
        "review-rr_0123456789ABCDEFGHJKMNPQRS-codex-sec",
        &source,
    )
    .await
    .expect("review snapshot");
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("wt-review-rr_0123456789ABCDEFGHJKMNPQRS-codex-sec")
    );
    assert_eq!(
        std::fs::read_to_string(path.join("src/lib.rs")).expect("reviewed source"),
        "pub fn reviewed() {}\n"
    );
}

#[async_trait::async_trait]
impl WorktreeProvider for RecordingProvider {
    async fn create(&self, name: &str) -> Result<PathBuf, CoreError> {
        let path = PathBuf::from("/pool").join(name);
        self.paths.lock().expect("paths").push(path.clone());
        Ok(path)
    }

    async fn remove(&self, path: &Path) -> Result<(), CoreError> {
        self.removed
            .lock()
            .expect("removed")
            .push(path.to_path_buf());
        Ok(())
    }

    async fn list(&self) -> Result<Vec<PathBuf>, CoreError> {
        Ok(self.paths.lock().expect("paths").clone())
    }
}

#[tokio::test]
async fn keyed_worktrees_are_isolated_and_boot_gc_preserves_durable_paths() {
    let provider = Arc::new(RecordingProvider::default());
    let pool = WorktreePool::new(provider.clone());
    let first = WorktreeAllocator::allocate(&pool, "tr_0123456789ABCDEFGHJKMNPQRS")
        .await
        .expect("first task worktree");
    let second = WorktreeAllocator::allocate(&pool, "tr_1123456789ABCDEFGHJKMNPQRS")
        .await
        .expect("second task worktree");
    assert_ne!(first, second);

    pool.gc_on_boot_preserving([first.clone()])
        .await
        .expect("boot gc");
    assert_eq!(
        provider.removed.lock().expect("removed").as_slice(),
        [second]
    );
}

#[tokio::test]
async fn pool_release_rejects_mismatched_task_keyed_path() {
    let pool = WorktreePool::new(Arc::new(RecordingProvider::default()));
    let error = WorktreeAllocator::release(
        &pool,
        "tr_0123456789ABCDEFGHJKMNPQRS",
        Path::new("/pool/wt-human-feature"),
    )
    .await
    .expect_err("foreign path must be refused");
    assert!(error.to_string().contains("does not match pool name"));
}

#[tokio::test]
async fn boot_gc_preserves_active_worktrees_by_pool_basename() {
    let provider = Arc::new(RecordingProvider::default());
    provider.paths.lock().expect("paths").extend([
        PathBuf::from("/canonical/wt-task-tr_0123456789ABCDEFGHJKMNPQRS"),
        PathBuf::from("/canonical/wt-review-rr_1123456789ABCDEFGHJKMNPQRS-codex-sec"),
        PathBuf::from("/canonical/wt-task-tr_2123456789ABCDEFGHJKMNPQRS"),
    ]);
    let pool = WorktreePool::new(provider.clone());

    pool.gc_on_boot_preserving([
        PathBuf::from("/durable/wt-task-tr_0123456789ABCDEFGHJKMNPQRS"),
        PathBuf::from("/durable/wt-review-rr_1123456789ABCDEFGHJKMNPQRS-codex-sec"),
    ])
    .await
    .expect("boot gc");
    assert_eq!(
        provider.removed.lock().expect("removed").as_slice(),
        [PathBuf::from(
            "/canonical/wt-task-tr_2123456789ABCDEFGHJKMNPQRS"
        )]
    );
}
