//! P0.9 9a: the production `RunHost` contract, exercised over a REAL `SqliteStore`
//! on a tempfile + the in-memory port fakes (NOT `FakeRunHost`). The full
//! `draft.dot` E2E + emit assertions land in 9a-T3; this skeleton checks
//! construction + a read.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentd_bin::{
    ProductionRunHost, SystemClock,
    daemon::{cleanup_failed_worktrees, gc_worktrees_on_boot},
};
use agentd_core::CoreError;
use agentd_core::engine::RunProgress;
use agentd_core::ports::{
    AgentBackend, CommandError, CommandOutput, CommandRunner, RunOpts, RunStatus, WorktreeAllocator,
};
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::{AgentHandle, NodeId, RunId, SpawnRequest};
use agentd_store::{SqliteStore, review_repo, run_repo, task_repo};
use agentd_surface::host::RunHost;
use agentd_surface::mcp_server::dispatch;
use agentd_tmux::{WorktreePool, WorktreeProvider};
use serde_json::json;

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
}

#[derive(Clone, Debug)]
struct SharedBackend(Arc<FakeBackend>);

#[async_trait::async_trait]
impl AgentBackend for SharedBackend {
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        self.0.spawn(req).await
    }
}

#[derive(Clone, Debug)]
struct SharedRunner(Arc<RecordingCommandRunner>);

#[async_trait::async_trait]
impl CommandRunner for SharedRunner {
    async fn run(
        &self,
        program: &str,
        args: &[String],
        opts: RunOpts,
    ) -> Result<CommandOutput, CommandError> {
        self.0.run(program, args, opts).await
    }
}

#[derive(Debug)]
struct StaticAllocator {
    path: PathBuf,
}

impl StaticAllocator {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

#[async_trait::async_trait]
impl WorktreeAllocator for StaticAllocator {
    async fn allocate(&self, _key: &str) -> Result<PathBuf, CoreError> {
        Ok(self.path.clone())
    }

    async fn release(&self, _key: &str, _path: &std::path::Path) -> Result<(), CoreError> {
        Ok(())
    }
}

#[derive(Debug)]
struct FailingAllocator;

#[async_trait::async_trait]
impl WorktreeAllocator for FailingAllocator {
    async fn allocate(&self, _key: &str) -> Result<PathBuf, CoreError> {
        Err(CoreError::Backend("injected allocator failure".into()))
    }

    async fn release(&self, _key: &str, _path: &std::path::Path) -> Result<(), CoreError> {
        Ok(())
    }
}

#[derive(Debug)]
struct FakeWorktreeProvider {
    paths: Mutex<Vec<PathBuf>>,
}

impl FakeWorktreeProvider {
    fn new(paths: Vec<PathBuf>) -> Self {
        Self {
            paths: Mutex::new(paths),
        }
    }

    async fn paths(&self) -> Vec<PathBuf> {
        self.paths.lock().expect("lock").clone()
    }
}

#[async_trait::async_trait]
impl WorktreeProvider for FakeWorktreeProvider {
    async fn create(&self, _name: &str) -> Result<PathBuf, CoreError> {
        Err(CoreError::Backend("unused fake create".into()))
    }

    async fn remove(&self, path: &Path) -> Result<(), CoreError> {
        self.paths.lock().expect("lock").retain(|p| p != path);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<PathBuf>, CoreError> {
        Ok(self.paths.lock().expect("lock").clone())
    }
}

struct ObservedHost {
    host: ProductionRunHost,
    runner: Arc<RecordingCommandRunner>,
    _dir: tempfile::TempDir,
}

async fn production_host() -> (ProductionRunHost, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let host = ProductionRunHost::new(
        store,
        Box::new(FakeBackend::new()),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    )
    .with_worktree_allocator(Some(Box::new(StaticAllocator::new("/tmp/agentd-task-wt"))));
    (host, dir)
}

async fn production_host_with_allocator(
    allocator: Option<Box<dyn WorktreeAllocator>>,
) -> ObservedHost {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let backend = Arc::new(FakeBackend::new());
    let runner = Arc::new(RecordingCommandRunner::new());
    let host = ProductionRunHost::new(
        store,
        Box::new(SharedBackend(backend.clone())),
        Box::new(SharedRunner(runner.clone())),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    )
    .with_worktree_allocator(allocator);
    ObservedHost {
        host,
        runner,
        _dir: dir,
    }
}

#[tokio::test]
async fn production_run_snapshot_is_none_for_unknown_run() {
    let (host, _dir) = production_host().await;
    let snap = host
        .run_snapshot(&RunId::from_string("ghost"))
        .await
        .expect("run_snapshot");
    assert!(snap.is_none(), "an unknown run has no snapshot");
}

/// The scriptable in-process agent: submit a node's success through the same MCP
/// tool layer a real agent uses (`dispatch`), minus the rmcp wire.
async fn agent_submit_success(
    host: &ProductionRunHost,
    run: &str,
    node: &str,
) -> Result<serde_json::Value, agentd_surface::SurfaceError> {
    dispatch(
        host,
        "submit_outcome",
        json!({
            "run_id": run, "node_id": node, "attempt": 1, "status": "success",
            "context_updates": {}, "suggested_next": []
        }),
    )
    .await
}

/// Record a `draft.dot` run and start it (parks at `propose_spec`).
async fn start_draft(host: &ProductionRunHost, run: &RunId) -> RunProgress {
    run_repo::record_run(host.store().pool(), run, "draft.dot", "sha")
        .await
        .expect("record run");
    host.start_run(run).await.expect("start run")
}

#[tokio::test]
async fn production_runhost_drives_draft_dot_to_done() {
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("r1");

    let parked = start_draft(&host, &run).await;
    assert!(
        matches!(parked, RunProgress::Parked { .. }),
        "draft.dot parks at propose_spec, got {parked:?}"
    );

    agent_submit_success(&host, "r1", "propose_spec")
        .await
        .expect("submit propose_spec");

    let snap = host
        .run_snapshot(&run)
        .await
        .expect("snapshot")
        .expect("run exists");
    assert_eq!(snap.status, "finished", "the run completed");

    let events = host.events_from(&run, 0).await.expect("events");
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds,
        vec!["run_parked", "run_finished"],
        "one row per state change, in order"
    );
    assert!(events[0].seq < events[1].seq, "seq is increasing");
}

#[tokio::test]
async fn production_runhost_replayed_submit_is_rejected_without_new_event() {
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("r1");
    start_draft(&host, &run).await;
    agent_submit_success(&host, "r1", "propose_spec")
        .await
        .expect("first submit");

    // Replay: the task is closed, so find_open_task_run -> None -> NotAssigned.
    let replay = agent_submit_success(&host, "r1", "propose_spec").await;
    assert!(
        replay.is_err(),
        "a replayed submit for a closed task is rejected, got {replay:?}"
    );

    let events = host.events_from(&run, 0).await.expect("events");
    assert_eq!(
        events.len(),
        2,
        "the rejected replay emits no additional event row"
    );
}

#[tokio::test]
async fn production_runhost_non_review_park_payload_stays_node_only() {
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("draft-payload");

    start_draft(&host, &run).await;

    let events = host.events_from(&run, 0).await.expect("events");
    let parked = events
        .iter()
        .find(|e| e.kind == "run_parked")
        .expect("draft emits a park event");
    assert_eq!(
        parked.payload, r#"{"node":"propose_spec"}"#,
        "non-review parks keep the existing node-only payload"
    );
}

/// The scriptable reviewer: submit a pass verdict through the `submit_review`
/// tool (which also exercises the production host's `review_counts`).
async fn agent_submit_review(
    host: &ProductionRunHost,
    review_run_id: &str,
    reviewer: &str,
) -> Result<serde_json::Value, agentd_surface::SurfaceError> {
    dispatch(
        host,
        "submit_review",
        json!({
            "review_run_id": review_run_id, "reviewer_id": reviewer,
            "verdict": "pass", "findings": []
        }),
    )
    .await
}

#[tokio::test]
async fn production_runhost_drives_execute_dot_to_done() {
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("e1");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");

    // start -> pull_frozen_spec, draft_plan (tools) -> implement (codergen) parks.
    let parked = host.start_run(&run).await.expect("start");
    assert!(
        matches!(parked, RunProgress::Parked { .. }),
        "execute.dot parks at implement, got {parked:?}"
    );

    // implement success -> verify_lifecycle (tool) -> review (fan_out) parks.
    agent_submit_success(&host, "e1", "implement")
        .await
        .expect("submit implement");

    // The scriptable agent learns review_run_id from the store (the spawn-context
    // seam; the real rmcp path is D7/deployment).
    let review_run_id = review_repo::find_open_review_run(host.store().pool(), &run)
        .await
        .expect("find review run")
        .expect("an open review run");
    for reviewer in ["claude-sec", "codex-perf", "gemini-readability"] {
        agent_submit_review(&host, review_run_id.as_str(), reviewer)
            .await
            .expect("submit_review");
    }

    // aggregate (majority_pass) -> open_pr -> report_acceptance -> done.
    let snap = host
        .run_snapshot(&run)
        .await
        .expect("snapshot")
        .expect("run exists");
    assert_eq!(snap.status, "finished", "execute.dot reached done");

    // The multi-park emit sequence: parked at implement AND review, then finished.
    let events = host.events_from(&run, 0).await.expect("events");
    let kinds: Vec<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(
        kinds.first(),
        Some(&"run_parked"),
        "starts parked: {kinds:?}"
    );
    assert_eq!(
        kinds.last(),
        Some(&"run_finished"),
        "ends finished: {kinds:?}"
    );
    assert!(
        kinds.iter().filter(|k| **k == "run_parked").count() >= 2,
        "multi-park (implement + review): {kinds:?}"
    );
}

#[tokio::test]
async fn production_runhost_review_park_payload_includes_default_round() {
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("review-payload");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");

    host.start_run(&run).await.expect("start");
    agent_submit_success(&host, "review-payload", "implement")
        .await
        .expect("submit implement");

    let events = host.events_from(&run, 0).await.expect("events");
    let review_park = events
        .iter()
        .find(|e| e.kind == "run_parked" && e.payload.contains(r#""review""#))
        .expect("review park event");
    assert_eq!(
        review_park.payload, r#"{"node":"review","round":1}"#,
        "review parks carry the default Delphi round"
    );
}

#[tokio::test]
async fn production_runhost_execute_uses_injected_worktree_allocator() {
    let observed =
        production_host_with_allocator(Some(Box::new(StaticAllocator::new("/tmp/agentd-task-wt"))))
            .await;
    let host = &observed.host;
    let run = RunId::from_string("e-wt");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");

    let parked = host.start_run(&run).await.expect("start");
    assert!(
        matches!(parked, RunProgress::Parked { .. }),
        "execute.dot parks at implement, got {parked:?}"
    );
    let assignment = host
        .open_task(&run, &NodeId::parsed("implement"))
        .await
        .expect("open task")
        .expect("implement task is open");
    assert_eq!(
        assignment.worktree.as_deref(),
        Some("/tmp/agentd-task-wt"),
        "task assignment exposes the allocated worktree"
    );

    agent_submit_success(host, "e-wt", "implement")
        .await
        .expect("submit implement");

    let calls = observed.runner.calls();
    let lifecycle = calls
        .iter()
        .find(|c| c.program == "agent-spec" && c.args.first().is_some_and(|a| a == "lifecycle"))
        .expect("verify_lifecycle command recorded");
    let code_pos = lifecycle
        .args
        .iter()
        .position(|a| a == "--code")
        .expect("verify_lifecycle has --code");
    assert_eq!(
        lifecycle.args.get(code_pos + 1).map(String::as_str),
        Some("/tmp/agentd-task-wt"),
        "verify_lifecycle checks the allocated implementation worktree"
    );
}

#[tokio::test]
async fn production_open_task_returns_assigned_agent_id() {
    let observed =
        production_host_with_allocator(Some(Box::new(StaticAllocator::new("/tmp/agentd-task-wt"))))
            .await;
    let host = &observed.host;
    let run = RunId::from_string("e-owner");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");

    host.start_run(&run).await.expect("start");

    let assignment = host
        .open_task(&run, &NodeId::parsed("implement"))
        .await
        .expect("open task")
        .expect("implement task is open");
    assert_eq!(
        assignment.agent_id, "implementer",
        "production open_task returns the codergen role as task owner"
    );
}

#[tokio::test]
async fn production_assign_task_accepts_owner_and_rejects_other_agent() {
    let observed =
        production_host_with_allocator(Some(Box::new(StaticAllocator::new("/tmp/agentd-task-wt"))))
            .await;
    let host = &observed.host;
    let run = RunId::from_string("e-assign-task");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");

    host.start_run(&run).await.expect("start");

    let owner = dispatch(
        host,
        "assign_task",
        json!({
            "run_id": "e-assign-task",
            "node_id": "implement",
            "agent_id": "implementer"
        }),
    )
    .await
    .expect("owner is assigned");
    assert_eq!(owner["worktree"], "/tmp/agentd-task-wt");

    let other = dispatch(
        host,
        "assign_task",
        json!({
            "run_id": "e-assign-task",
            "node_id": "implement",
            "agent_id": "someone-else"
        }),
    )
    .await
    .expect_err("different agent is rejected");
    assert_eq!(other.code(), "not_assigned");
}

#[tokio::test]
async fn production_runhost_execute_publishes_worktree_branch_before_pr() {
    let observed =
        production_host_with_allocator(Some(Box::new(StaticAllocator::new("/tmp/agentd-task-wt"))))
            .await;
    let host = &observed.host;
    let run = RunId::from_string("e-pr");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");

    let parked = host.start_run(&run).await.expect("start");
    assert!(
        matches!(parked, RunProgress::Parked { .. }),
        "execute.dot parks at implement, got {parked:?}"
    );
    let assignment = host
        .open_task(&run, &NodeId::parsed("implement"))
        .await
        .expect("open task")
        .expect("implement task is open");
    let task_run_id_arg = assignment.task_run_id.as_str().to_string();
    agent_submit_success(host, "e-pr", "implement")
        .await
        .expect("submit implement");
    let review_run_id = review_repo::find_open_review_run(host.store().pool(), &run)
        .await
        .expect("find review run")
        .expect("an open review run");
    for reviewer in ["claude-sec", "codex-perf", "gemini-readability"] {
        agent_submit_review(host, review_run_id.as_str(), reviewer)
            .await
            .expect("submit_review");
    }

    let snap = host
        .run_snapshot(&run)
        .await
        .expect("snapshot")
        .expect("run exists");
    assert_eq!(snap.status, "finished", "execute.dot reached done");

    let calls = observed.runner.calls();
    let publish_idx = calls
        .iter()
        .position(|c| {
            c.program == "bash"
                && c.args
                    .first()
                    .is_some_and(|a| a == "scripts/agentd_publish_worktree.sh")
        })
        .expect("publish_branch recorded a script call");
    assert_eq!(
        calls[publish_idx].args,
        vec![
            "scripts/agentd_publish_worktree.sh".to_string(),
            "/tmp/agentd-task-wt".to_string(),
            task_run_id_arg.clone(),
        ]
    );

    let open_pr_idx = calls
        .iter()
        .position(|c| {
            c.program == "bash"
                && c.args
                    .first()
                    .is_some_and(|a| a == "scripts/agentd_open_pr.sh")
        })
        .expect("open_pr recorded a script call");
    assert!(
        publish_idx < open_pr_idx,
        "publish_branch must run before open_pr: {calls:?}"
    );
    assert_eq!(
        calls[open_pr_idx].args,
        vec!["scripts/agentd_open_pr.sh".to_string(), task_run_id_arg,]
    );
}

#[tokio::test]
async fn production_runhost_allocator_failure_stops_execute_before_verify() {
    let observed = production_host_with_allocator(Some(Box::new(FailingAllocator))).await;
    let host = &observed.host;
    let run = RunId::from_string("e-wt-fail");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");

    let err = host.start_run(&run).await.expect_err("allocator failure");
    assert!(
        format!("{err:?}").contains("injected allocator failure"),
        "allocator failure is surfaced, got {err:?}"
    );
    assert!(
        observed.runner.calls().iter().all(|c| {
            !(c.program == "agent-spec" && c.args.first().is_some_and(|a| a == "lifecycle"))
        }),
        "verify_lifecycle must not run after allocator failure"
    );
}

#[tokio::test]
async fn build_production_host_preserves_active_worktrees_during_boot_gc() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let run = RunId::from_string("gc-running");
    run_repo::record_run(store.pool(), &run, "execute.dot", "sha")
        .await
        .expect("record run");
    let task_run = task_repo::insert_task_run(store.pool(), &run, &NodeId::parsed("implement"))
        .await
        .expect("insert task");
    task_repo::set_task_run_worktree(
        store.pool(),
        &task_run,
        "/tmp/agentd/worktrees/wt-task-tr_0123456789ABCDEFGHJKMNPQRS",
    )
    .await
    .expect("set task worktree");

    let active =
        PathBuf::from("/private/tmp/agentd/worktrees/wt-task-tr_0123456789ABCDEFGHJKMNPQRS");
    let stale =
        PathBuf::from("/private/tmp/agentd/worktrees/wt-task-tr_11111111111111111111111111");
    let provider = Arc::new(FakeWorktreeProvider::new(vec![
        active.clone(),
        stale.clone(),
    ]));
    let pool = WorktreePool::new(provider.clone());

    gc_worktrees_on_boot(&store, &pool).await.expect("boot gc");

    assert_eq!(
        provider.paths().await,
        vec![active],
        "daemon boot-GC preserves store-active worktrees and removes unreferenced leftovers"
    );
}

async fn seed_cleanup_runs(
    store: &SqliteStore,
) -> (PathBuf, PathBuf, PathBuf, RunId, RunId, RunId) {
    let running = RunId::from_string("cleanup-running");
    run_repo::record_run(store.pool(), &running, "execute.dot", "sha")
        .await
        .expect("record running");
    let running_task =
        task_repo::insert_task_run(store.pool(), &running, &NodeId::parsed("implement"))
            .await
            .expect("running task");
    let running_path = PathBuf::from(format!(
        "/tmp/agentd/worktrees/wt-task-{}",
        running_task.as_str()
    ));
    task_repo::set_task_run_worktree(store.pool(), &running_task, &running_path.to_string_lossy())
        .await
        .expect("running worktree");

    let failed = RunId::from_string("cleanup-failed");
    run_repo::record_run(store.pool(), &failed, "execute.dot", "sha")
        .await
        .expect("record failed");
    let failed_task =
        task_repo::insert_task_run(store.pool(), &failed, &NodeId::parsed("implement"))
            .await
            .expect("failed task");
    let failed_task_path = PathBuf::from(format!(
        "/tmp/agentd/worktrees/wt-task-{}",
        failed_task.as_str()
    ));
    task_repo::set_task_run_worktree(
        store.pool(),
        &failed_task,
        &failed_task_path.to_string_lossy(),
    )
    .await
    .expect("failed task worktree");
    let failed_review = review_repo::insert_review_run(
        store.pool(),
        &failed,
        &NodeId::parsed("review"),
        1,
        1,
        "csha",
    )
    .await
    .expect("failed review");
    let reviewer = agentd_core::types::AgentId::parsed("debug");
    let failed_review_path = PathBuf::from(format!(
        "/tmp/agentd/worktrees/wt-review-{}-{}",
        failed_review.as_str(),
        reviewer.as_str()
    ));
    review_repo::set_review_worktree(
        store.pool(),
        &failed_review,
        &reviewer,
        &failed_review_path.to_string_lossy(),
    )
    .await
    .expect("failed review worktree");
    run_repo::update_run_status(store.pool(), &failed, RunStatus::Failed)
        .await
        .expect("mark failed");

    let finished = RunId::from_string("cleanup-finished");
    run_repo::record_run(store.pool(), &finished, "execute.dot", "sha")
        .await
        .expect("record finished");
    let finished_task =
        task_repo::insert_task_run(store.pool(), &finished, &NodeId::parsed("implement"))
            .await
            .expect("finished task");
    let finished_path = PathBuf::from(format!(
        "/tmp/agentd/worktrees/wt-task-{}",
        finished_task.as_str()
    ));
    task_repo::set_task_run_worktree(
        store.pool(),
        &finished_task,
        &finished_path.to_string_lossy(),
    )
    .await
    .expect("finished worktree");
    run_repo::update_run_status(store.pool(), &finished, RunStatus::Finished)
        .await
        .expect("mark finished");

    (
        failed_task_path,
        failed_review_path,
        running_path,
        failed,
        running,
        finished,
    )
}

#[tokio::test]
async fn cleanup_failed_worktrees_dry_run_lists_without_releasing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let (failed_task_path, failed_review_path, running_path, _failed, _running, _finished) =
        seed_cleanup_runs(&store).await;
    let provider = Arc::new(FakeWorktreeProvider::new(vec![
        failed_task_path.clone(),
        failed_review_path.clone(),
        running_path.clone(),
    ]));
    let pool = WorktreePool::new(provider.clone());

    let plan = cleanup_failed_worktrees(&store, &pool, false)
        .await
        .expect("dry-run cleanup");

    assert_eq!(
        plan.candidates.len(),
        2,
        "dry-run reports failed candidates"
    );
    assert_eq!(plan.released, 0, "dry-run releases nothing");
    assert_eq!(
        provider.paths().await,
        vec![
            failed_task_path.clone(),
            failed_review_path.clone(),
            running_path.clone()
        ],
        "dry-run does not remove provider worktrees"
    );
    let active_paths: std::collections::HashSet<PathBuf> = store
        .active_worktree_paths()
        .await
        .expect("active paths")
        .into_iter()
        .collect();
    assert!(
        active_paths.contains(&failed_task_path) && active_paths.contains(&failed_review_path),
        "dry-run does not clear active failed-run store refs"
    );
}

#[tokio::test]
async fn cleanup_failed_worktrees_execute_removes_failed_worktrees_and_clears_refs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let (failed_task_path, failed_review_path, running_path, _failed, _running, _finished) =
        seed_cleanup_runs(&store).await;
    let provider = Arc::new(FakeWorktreeProvider::new(vec![
        failed_task_path.clone(),
        failed_review_path.clone(),
        running_path.clone(),
    ]));
    let pool = WorktreePool::new(provider.clone());

    let plan = cleanup_failed_worktrees(&store, &pool, true)
        .await
        .expect("execute cleanup");

    assert_eq!(plan.candidates.len(), 2, "execute saw failed candidates");
    assert_eq!(
        plan.released, 2,
        "execute released both failed-run worktrees"
    );
    assert_eq!(
        provider.paths().await,
        vec![running_path.clone()],
        "execute leaves unrelated running worktree alone"
    );
    let active_paths: std::collections::HashSet<PathBuf> = store
        .active_worktree_paths()
        .await
        .expect("active paths")
        .into_iter()
        .collect();
    assert!(
        !active_paths.contains(&failed_task_path) && !active_paths.contains(&failed_review_path),
        "execute clears failed-run store refs"
    );
    assert!(
        active_paths.contains(&running_path),
        "execute does not clear running store refs"
    );
}

#[tokio::test]
async fn emit_persists_and_broadcasts() {
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("r1");
    // Subscribe BEFORE the run starts so the live event is captured.
    let mut rx = host.subscribe_events();

    run_repo::record_run(host.store().pool(), &run, "draft.dot", "sha")
        .await
        .expect("record");
    host.start_run(&run).await.expect("start"); // parks at propose_spec -> emits run_parked

    // Persisted (durable/audit).
    let persisted = host.events_from(&run, 0).await.expect("events");
    assert_eq!(
        persisted.iter().filter(|e| e.kind == "run_parked").count(),
        1,
        "the event is persisted"
    );

    // Broadcast (the live tail) — the same event, non-blocking.
    let live = rx.try_recv().expect("a live event was broadcast");
    assert_eq!(live.run_id, "r1");
    assert_eq!(live.event.kind, "run_parked");
    assert_eq!(
        live.event.seq, persisted[0].seq,
        "same seq as the persisted row"
    );
}

#[tokio::test]
async fn production_list_runs_reflects_statuses() {
    let (host, _dir) = production_host().await;
    // run a: started -> parks (status stays "running"; only terminal updates it).
    let a = RunId::from_string("a");
    run_repo::record_run(host.store().pool(), &a, "draft.dot", "sha")
        .await
        .expect("record a");
    host.start_run(&a).await.expect("start a");
    // run b: recorded then marked finished.
    let b = RunId::from_string("b");
    run_repo::record_run(host.store().pool(), &b, "draft.dot", "sha")
        .await
        .expect("record b");
    run_repo::update_run_status(host.store().pool(), &b, RunStatus::Finished)
        .await
        .expect("finish b");

    let runs = host.list_runs().await.expect("list_runs");
    assert_eq!(runs.len(), 2, "both runs listed");
    assert!(
        runs[0].started_at >= runs[1].started_at,
        "most-recently-started first"
    );
    let a_sum = runs
        .iter()
        .find(|r| r.run_id == "a")
        .expect("run a present");
    let b_sum = runs
        .iter()
        .find(|r| r.run_id == "b")
        .expect("run b present");
    assert_eq!(a_sum.status, "running", "the started run is in-flight");
    assert_eq!(b_sum.status, "finished");
}

#[tokio::test]
async fn production_runhost_dedupes_same_node_reparks() {
    // P1 re-park-noise: a fan-out review re-parks at the SAME node per non-final
    // verdict; the emit point must dedup so only the first park at a node emits.
    let (host, _dir) = production_host().await;
    let run = RunId::from_string("e1");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");

    // start -> implement park; implement success -> review (fan_out) park.
    host.start_run(&run).await.expect("start");
    agent_submit_success(&host, "e1", "implement")
        .await
        .expect("submit implement");
    let review_run_id = review_repo::find_open_review_run(host.store().pool(), &run)
        .await
        .expect("find review run")
        .expect("an open review run");

    // 1 of 3 pass verdicts (majority_pass) does NOT decide -> re-parks at review.
    agent_submit_review(&host, review_run_id.as_str(), "claude-sec")
        .await
        .expect("submit r1");
    // Confirm it WAS a same-node re-park (still at review), so the dedup assertion
    // below is meaningful and not vacuous.
    let mid = host
        .run_snapshot(&run)
        .await
        .expect("snap")
        .expect("exists");
    assert_eq!(
        mid.current_node.as_deref(),
        Some("review"),
        "the 1st verdict re-parks at the same review node"
    );

    // The remaining reviewers complete the review -> the run drives to done.
    for reviewer in ["codex-perf", "gemini-readability"] {
        agent_submit_review(&host, review_run_id.as_str(), reviewer)
            .await
            .expect("submit review");
    }
    let snap = host
        .run_snapshot(&run)
        .await
        .expect("snap")
        .expect("exists");
    assert_eq!(snap.status, "finished", "execute.dot reached done");

    // The same-node re-park emitted no duplicate: exactly one run_parked for review.
    let events = host.events_from(&run, 0).await.expect("events");
    let review_parks = events
        .iter()
        .filter(|e| e.kind == "run_parked" && e.payload == r#"{"node":"review","round":1}"#)
        .count();
    assert_eq!(
        review_parks,
        1,
        "the same-node re-park is deduped to one run_parked: {:?}",
        events
            .iter()
            .map(|e| (e.kind.as_str(), e.payload.as_str()))
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn concurrent_review_verdicts_advance_run_once() {
    // P2 Foundation A (smoke guard; deterministic only WITH the per-run lock — the
    // run_lock_is_per_run unit test is the mechanism proof). Without serialization,
    // N concurrent verdicts can each advance the run → multiple run_finished.
    let (host, _dir) = production_host().await;
    let host = Arc::new(host);
    let run = RunId::from_string("e1");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");
    host.start_run(&run).await.expect("start"); // parks at implement
    agent_submit_success(&host, "e1", "implement")
        .await
        .expect("submit implement"); // -> review park
    let review_run_id = review_repo::find_open_review_run(host.store().pool(), &run)
        .await
        .expect("find review run")
        .expect("an open review run");

    // The three reviewers submit CONCURRENTLY on the shared host.
    let mut tasks = Vec::new();
    for reviewer in ["claude-sec", "codex-perf", "gemini-readability"] {
        let h = host.clone();
        let rr = review_run_id.as_str().to_string();
        tasks.push(tokio::spawn(async move {
            agent_submit_review(h.as_ref(), &rr, reviewer).await
        }));
    }
    for t in tasks {
        t.await.expect("join").expect("submit_review ok");
    }

    let snap = host
        .run_snapshot(&run)
        .await
        .expect("snap")
        .expect("exists");
    assert_eq!(snap.status, "finished", "the run advanced to finished");
    let events = host.events_from(&run, 0).await.expect("events");
    let finishes = events.iter().filter(|e| e.kind == "run_finished").count();
    assert_eq!(
        finishes,
        1,
        "the run advanced EXACTLY once, not N times: {:?}",
        events.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn production_runhost_events_from_unknown_run_is_empty() {
    let (host, _dir) = production_host().await;
    let events = host
        .events_from(&RunId::from_string("ghost"), 0)
        .await
        .expect("events");
    assert!(events.is_empty());
}
