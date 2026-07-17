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
async fn release_refuses_a_path_not_owned_by_the_requested_key() {
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
