//! §7.3 P1.3: the worktree pool + `PooledBackend` decorator, over an in-memory
//! `WorktreeProvider` fake (no git/tmux). Names match
//! `specs/tmux/p6-worktree-pool.spec.md`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use agentd_core::CoreError;
use agentd_core::ports::{AgentBackend, WorktreeAllocator};
use agentd_core::types::{
    AgentHandle, AgentId, BackendKind, CliKind, LaunchStrategy, SpawnRequest,
};
use agentd_tmux::{PooledBackend, WorktreePool, WorktreeProvider};

/// In-memory provider: `create` mints `/wt/<name>` and records it; `list`/`remove`
/// operate on that set; `fail` injects a create error.
#[derive(Default)]
struct FakeProvider {
    created: Mutex<Vec<PathBuf>>,
    fail: bool,
    root: Option<PathBuf>,
}

impl FakeProvider {
    fn with_existing(paths: Vec<PathBuf>) -> Self {
        Self {
            created: Mutex::new(paths),
            fail: false,
            root: None,
        }
    }

    fn materialized(root: impl Into<PathBuf>) -> Self {
        Self {
            created: Mutex::new(Vec::new()),
            fail: false,
            root: Some(root.into()),
        }
    }

    fn failing() -> Self {
        Self {
            created: Mutex::new(Vec::new()),
            fail: true,
            root: None,
        }
    }
}

#[async_trait::async_trait]
impl WorktreeProvider for FakeProvider {
    async fn create(&self, name: &str) -> Result<PathBuf, CoreError> {
        if self.fail {
            return Err(CoreError::Backend("injected create failure".into()));
        }
        let p = self
            .root
            .as_deref()
            .unwrap_or_else(|| Path::new("/wt"))
            .join(name);
        if self.root.is_some() {
            std::fs::create_dir_all(&p).map_err(|e| {
                CoreError::Backend(format!("create fake worktree {}: {e}", p.display()))
            })?;
        }
        self.created.lock().expect("lock").push(p.clone());
        Ok(p)
    }
    async fn remove(&self, path: &Path) -> Result<(), CoreError> {
        self.created.lock().expect("lock").retain(|p| p != path);
        Ok(())
    }
    async fn list(&self) -> Result<Vec<PathBuf>, CoreError> {
        Ok(self.created.lock().expect("lock").clone())
    }
}

fn req_with_worktree(worktree: &str) -> SpawnRequest {
    SpawnRequest {
        agent_id: AgentId::parsed("implementer"),
        mxid: None,
        cli: CliKind::ClaudeCode,
        worktree: PathBuf::from(worktree),
        initial_prompt: None,
        env_overrides: HashMap::new(),
        launch_strategy: LaunchStrategy::Direct,
    }
}

#[tokio::test]
async fn pool_allocates_distinct_worktrees() {
    let pool = WorktreePool::new(Arc::new(FakeProvider::default()));
    let a = pool.allocate().await.expect("alloc a");
    let b = pool.allocate().await.expect("alloc b");
    assert_ne!(a, b, "each allocation is a fresh, distinct worktree");
}

#[tokio::test]
async fn pool_concurrent_allocations_are_distinct() {
    let pool = Arc::new(WorktreePool::new(Arc::new(FakeProvider::default())));
    let mut handles = Vec::new();
    for _ in 0..32 {
        let p = pool.clone();
        handles.push(tokio::spawn(async move { p.allocate().await }));
    }
    let mut paths = HashSet::new();
    for h in handles {
        let path = h.await.expect("join").expect("alloc");
        assert!(paths.insert(path), "every concurrent allocation is unique");
    }
    assert_eq!(paths.len(), 32);
}

#[tokio::test]
async fn pooled_backend_overrides_auto_worktree() {
    let recorder = SharedRecorder::default();
    let backend = PooledBackend::new(
        recorder.clone(),
        WorktreePool::new(Arc::new(FakeProvider::default())),
    );
    backend.spawn(req_with_worktree(".")).await.expect("spawn");
    let seen = recorder.last().expect("a spawn was recorded");
    assert_ne!(seen, PathBuf::from("."), "the auto worktree was overridden");
    assert!(
        seen.starts_with("/wt"),
        "the inner backend got a pool worktree: {seen:?}"
    );
}

#[tokio::test]
async fn pooled_backend_passes_through_explicit_worktree() {
    let recorder = SharedRecorder::default();
    let backend = PooledBackend::new(
        recorder.clone(),
        WorktreePool::new(Arc::new(FakeProvider::default())),
    );
    backend
        .spawn(req_with_worktree("/explicit/path"))
        .await
        .expect("spawn");
    assert_eq!(
        recorder.last().expect("recorded"),
        PathBuf::from("/explicit/path"),
        "an explicit worktree passes through unchanged"
    );
}

#[tokio::test]
async fn boot_gc_removes_leftover_worktrees() {
    let provider = Arc::new(FakeProvider::with_existing(vec![
        PathBuf::from("/wt/wt-1-0"),
        PathBuf::from("/wt/wt-1-1"),
        PathBuf::from("/wt/wt-1-2"),
    ]));
    let pool = WorktreePool::new(provider.clone());
    pool.gc_on_boot().await.expect("gc");
    assert!(
        provider.list().await.expect("list").is_empty(),
        "boot-GC reclaimed every leftover pool worktree"
    );
}

#[tokio::test]
async fn boot_gc_preserves_active_worktrees_by_pool_basename() {
    let active_task =
        PathBuf::from("/private/tmp/agentd/worktrees/wt-task-tr_0123456789ABCDEFGHJKMNPQRS");
    let active_review = PathBuf::from(
        "/private/tmp/agentd/worktrees/wt-review-rr_0123456789ABCDEFGHJKMNPQRS-claude-sec",
    );
    let unreferenced =
        PathBuf::from("/private/tmp/agentd/worktrees/wt-task-tr_11111111111111111111111111");
    let provider = Arc::new(FakeProvider::with_existing(vec![
        active_task.clone(),
        active_review.clone(),
        unreferenced,
    ]));
    let pool = WorktreePool::new(provider.clone());

    pool.gc_on_boot_preserving([
        PathBuf::from("/tmp/agentd/worktrees/wt-task-tr_0123456789ABCDEFGHJKMNPQRS"),
        PathBuf::from("/tmp/agentd/worktrees/wt-review-rr_0123456789ABCDEFGHJKMNPQRS-claude-sec"),
    ])
    .await
    .expect("gc preserving active paths");

    assert_eq!(
        provider.list().await.expect("list"),
        vec![active_task, active_review],
        "boot-GC preserves active pool worktrees by basename and removes unreferenced leftovers"
    );
}

#[tokio::test]
async fn pool_allocate_provider_error_is_surfaced() {
    let pool = WorktreePool::new(Arc::new(FakeProvider::failing()));
    let err = pool.allocate().await;
    assert!(
        err.is_err(),
        "a provider create failure is surfaced, not hidden"
    );
}

#[tokio::test]
async fn pool_allocates_task_keyed_worktree_via_allocator_port() {
    let provider = Arc::new(FakeProvider::default());
    let pool = WorktreePool::new(provider.clone());
    let task_run_id = "tr_0123456789ABCDEFGHJKMNPQRS";

    let path = WorktreeAllocator::allocate(&pool, task_run_id)
        .await
        .expect("allocate via core port");

    let expected = PathBuf::from("/wt/wt-task-tr_0123456789ABCDEFGHJKMNPQRS");
    assert_eq!(path, expected, "the returned path uses the task-keyed name");
    assert_eq!(
        provider.list().await.expect("list"),
        vec![expected],
        "the provider created the same task-keyed worktree"
    );
}

#[tokio::test]
async fn pool_releases_task_keyed_worktree_via_allocator_port() {
    let provider = Arc::new(FakeProvider::default());
    let pool = WorktreePool::new(provider.clone());
    let task_run_id = "tr_0123456789ABCDEFGHJKMNPQRS";
    let path = WorktreeAllocator::allocate(&pool, task_run_id)
        .await
        .expect("allocate via core port");

    WorktreeAllocator::release(&pool, task_run_id, &path)
        .await
        .expect("release via core port");

    assert!(
        provider.list().await.expect("list").is_empty(),
        "release removes the task-keyed worktree"
    );
}

#[tokio::test]
async fn pool_allocates_reviewer_keyed_snapshot_worktree() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let source = tmp.path().join("source");
    std::fs::create_dir_all(source.join("src")).expect("source dirs");
    std::fs::write(source.join("src/lib.rs"), "pub fn reviewed() {}\n").expect("source file");
    std::fs::write(source.join(".git"), "gitdir: /real/git/dir\n").expect("git pointer");

    let provider = Arc::new(FakeProvider::materialized(tmp.path().join("worktrees")));
    let pool = WorktreePool::new(provider.clone());
    let key = "review-rr_0123456789ABCDEFGHJKMNPQRS-claude-sec";

    let path = pool
        .allocate_snapshot(key, &source)
        .await
        .expect("allocate snapshot via core port");

    let expected = tmp
        .path()
        .join("worktrees/wt-review-rr_0123456789ABCDEFGHJKMNPQRS-claude-sec");
    assert_eq!(
        path, expected,
        "the returned path uses the reviewer-keyed name"
    );
    assert_eq!(
        provider.list().await.expect("list"),
        vec![expected.clone()],
        "the provider created the same reviewer-keyed worktree"
    );
    assert_eq!(
        std::fs::read_to_string(expected.join("src/lib.rs")).expect("copied source"),
        "pub fn reviewed() {}\n",
        "reviewer snapshot contains source content"
    );
    assert!(
        !expected.join(".git").exists(),
        "snapshot copy must not overwrite the destination worktree .git metadata"
    );
}

#[tokio::test]
async fn pool_release_rejects_mismatched_task_keyed_path() {
    let provider = Arc::new(FakeProvider::with_existing(vec![PathBuf::from(
        "/wt/wt-task-feature",
    )]));
    let pool = WorktreePool::new(provider.clone());
    let err = WorktreeAllocator::release(
        &pool,
        "tr_0123456789ABCDEFGHJKMNPQRS",
        Path::new("/wt/wt-task-feature"),
    )
    .await
    .expect_err("mismatched path must be rejected");

    assert!(
        err.to_string().contains("does not match"),
        "error should explain the mismatch, got {err}"
    );
    assert_eq!(
        provider.list().await.expect("list"),
        vec![PathBuf::from("/wt/wt-task-feature")],
        "foreign path was not removed"
    );
}

/// A recorder whose `Clone` shares one inner `Arc<Mutex>`, so a `PooledBackend`
/// (which takes its inner by value) can still be observed after the spawn.
#[derive(Default, Clone)]
struct SharedRecorder {
    last_worktree: Arc<Mutex<Option<PathBuf>>>,
}

impl SharedRecorder {
    fn last(&self) -> Option<PathBuf> {
        self.last_worktree.lock().expect("lock").clone()
    }
}

#[async_trait::async_trait]
impl AgentBackend for SharedRecorder {
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        *self.last_worktree.lock().expect("lock") = Some(req.worktree.clone());
        Ok(AgentHandle {
            agent_id: req.agent_id,
            backend: BackendKind::Tmux,
            address: "test".into(),
            pane_id: None,
            pid: None,
            session_name: "s".into(),
            spawned_at: SystemTime::UNIX_EPOCH,
        })
    }
}
