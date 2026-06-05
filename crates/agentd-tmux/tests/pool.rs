//! §7.3 P1.3: the worktree pool + `PooledBackend` decorator, over an in-memory
//! `WorktreeProvider` fake (no git/tmux). Names match
//! `specs/tmux/p6-worktree-pool.spec.md`.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use agentd_core::CoreError;
use agentd_core::ports::AgentBackend;
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
}

impl FakeProvider {
    fn with_existing(paths: Vec<PathBuf>) -> Self {
        Self {
            created: Mutex::new(paths),
            fail: false,
        }
    }
    fn failing() -> Self {
        Self {
            created: Mutex::new(Vec::new()),
            fail: true,
        }
    }
}

#[async_trait::async_trait]
impl WorktreeProvider for FakeProvider {
    async fn create(&self, name: &str) -> Result<PathBuf, CoreError> {
        if self.fail {
            return Err(CoreError::Backend("injected create failure".into()));
        }
        let p = Path::new("/wt").join(name);
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
async fn pool_allocate_provider_error_is_surfaced() {
    let pool = WorktreePool::new(Arc::new(FakeProvider::failing()));
    let err = pool.allocate().await;
    assert!(
        err.is_err(),
        "a provider create failure is surfaced, not hidden"
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
