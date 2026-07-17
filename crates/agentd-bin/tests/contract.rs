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
    AgentAllocation, AgentAllocationStatus, AgentBackend, CommandError, CommandOutput,
    CommandRunner, RunOpts, RunStatus, WorktreeAllocator,
};
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::{AgentHandle, NodeId, RunId, SpawnRequest};
use agentd_store::{
    SqliteStore, agent_repo, agent_scheduler_repo, review_repo, run_repo, task_repo,
};
use agentd_surface::host::RunHost;
use agentd_surface::mcp_server::dispatch;
use agentd_worktree::{WorktreePool, WorktreeProvider};
use serde_json::json;

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[derive(Clone, Debug)]
struct SharedBackend(Arc<FakeBackend>);

#[async_trait::async_trait]
impl AgentBackend for SharedBackend {
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        self.0.spawn(req).await
    }
}

#[derive(Clone, Debug, Default)]
struct RecordingAllocationBackend {
    spawns: Arc<Mutex<Vec<SpawnRequest>>>,
    dispatches: Arc<Mutex<Vec<(SpawnRequest, AgentAllocation)>>>,
}

impl RecordingAllocationBackend {
    fn spawns(&self) -> Vec<SpawnRequest> {
        self.spawns.lock().expect("spawn lock").clone()
    }

    fn dispatches(&self) -> Vec<(SpawnRequest, AgentAllocation)> {
        self.dispatches.lock().expect("dispatch lock").clone()
    }

    fn handle(req: SpawnRequest, address: String) -> AgentHandle {
        AgentHandle {
            agent_id: req.agent_id,
            backend: agentd_core::types::BackendKind::NativeRuntime,
            address,
            pane_id: Some("%42".to_string()),
            pid: Some(4242),
            session_name: "agentd-codex-coding-1".to_string(),
            spawned_at: std::time::SystemTime::UNIX_EPOCH,
        }
    }
}

#[async_trait::async_trait]
impl AgentBackend for RecordingAllocationBackend {
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        self.spawns.lock().expect("spawn lock").push(req.clone());
        Ok(Self::handle(req, "fake://spawn".to_string()))
    }

    async fn dispatch_allocated(
        &self,
        req: SpawnRequest,
        allocation: &AgentAllocation,
    ) -> Result<AgentHandle, CoreError> {
        self.dispatches
            .lock()
            .expect("dispatch lock")
            .push((req.clone(), allocation.clone()));
        let address = allocation
            .runtime
            .get("tmuxTarget")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("fake://dispatch")
            .to_string();
        Ok(Self::handle(req, address))
    }
}

#[derive(Debug)]
struct FailingBackend;

#[async_trait::async_trait]
impl AgentBackend for FailingBackend {
    async fn spawn(&self, _req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        Err(CoreError::Backend(
            "injected backend spawn failure".to_string(),
        ))
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

    fn paths(&self) -> Vec<PathBuf> {
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
    backend: Arc<FakeBackend>,
    runner: Arc<RecordingCommandRunner>,
    dir: tempfile::TempDir,
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
    .with_tool_cwd(repo_root())
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
    .with_tool_cwd(repo_root())
    .with_worktree_allocator(allocator);
    ObservedHost {
        host,
        backend,
        runner,
        dir,
    }
}

async fn production_host_with_scheduler_allocator(max_per_cell: i64) -> ObservedHost {
    let mut observed =
        production_host_with_allocator(Some(Box::new(StaticAllocator::new("/tmp/agentd-task-wt"))))
            .await;
    observed.host = observed.host.with_scheduler_allocator(max_per_cell);
    observed
}

async fn production_host_with_recording_allocation_backend(
    max_per_cell: i64,
) -> (
    ProductionRunHost,
    RecordingAllocationBackend,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let backend = RecordingAllocationBackend::default();
    let host = ProductionRunHost::new(
        store,
        Box::new(backend.clone()),
        Box::new(SharedRunner(Arc::new(RecordingCommandRunner::new()))),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    )
    .with_tool_cwd(repo_root())
    .with_worktree_allocator(Some(Box::new(StaticAllocator::new("/tmp/agentd-task-wt"))))
    .with_scheduler_allocator(max_per_cell);
    (host, backend, dir)
}

async fn register_online_agent(host: &ProductionRunHost, name: &str, role: &str, capability: &str) {
    agent_repo::register_agent(
        host.store().pool(),
        agent_repo::RegisterAgent {
            name: name.to_string(),
            role: Some(role.to_string()),
            capability: Some(capability.to_string()),
            runtime: Some("codex".to_string()),
            model: None,
            native_runtime_ref: Some(format!("native://rs_{name}/ra_{name}")),
            home_dir: None,
            workdir: None,
            state_dir: None,
            server: Some("local".to_string()),
            runtime_profile: json!({}),
        },
    )
    .await
    .expect("register online agent");
}

fn write_p230_codergen_workflow(dir: &tempfile::TempDir) -> PathBuf {
    let path = dir.path().join("p230-codergen.dot");
    std::fs::write(
        &path,
        r#"digraph p230 {
  "start" [shape=Mdiamond];
  "implement" [handler="codergen", role="coding", capability="medium"];
  "done" [shape=Msquare];
  "start" -> "implement";
  "implement" -> "done" [condition="outcome=success"];
}
"#,
    )
    .expect("write p230 workflow");
    path
}

fn write_p233_fanout_workflow(dir: &tempfile::TempDir) -> PathBuf {
    let path = dir.path().join("p233-fanout.dot");
    std::fs::write(
        &path,
        r#"digraph p233 {
  "start" [shape=Mdiamond];
  "review" [handler="parallel.fan_out", reviewers="review", capability="medium"];
  "done" [shape=Msquare];
  "start" -> "review";
  "review" -> "done";
}
"#,
    )
    .expect("write p233 fanout workflow");
    path
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

#[tokio::test]
async fn production_workflow_scheduler_allocation_is_visible_in_run_events() {
    let observed = production_host_with_scheduler_allocator(0).await;
    let host = &observed.host;
    register_online_agent(host, "codex-coding-1", "coding", "medium").await;
    let workflow = write_p230_codergen_workflow(&observed.dir);
    let run = RunId::from_string("p230-event");
    run_repo::record_run(
        host.store().pool(),
        &run,
        workflow.to_string_lossy().as_ref(),
        "sha",
    )
    .await
    .expect("record");

    let parked = host.start_run(&run).await.expect("start");
    assert!(
        matches!(parked, RunProgress::Parked { .. }),
        "scheduler-backed workflow parks at implement, got {parked:?}"
    );
    let spawned = observed.backend.spawned();
    assert_eq!(spawned.len(), 1, "fake backend records one spawn");
    assert_eq!(spawned[0].agent_id.as_str(), "codex-coding-1");

    let busy = agent_scheduler_repo::pool_snapshot(
        host.store().pool(),
        agent_scheduler_repo::PoolFilters {
            role: Some("coding".to_string()),
            capability: Some("medium".to_string()),
            state: Some("busy".to_string()),
        },
    )
    .await
    .expect("busy pool snapshot");
    assert_eq!(
        busy.total, 1,
        "workflow allocation creates durable reservation"
    );
    assert_eq!(busy.agents[0].name, "codex-coding-1");

    let events = host.events_from(&run, 0).await.expect("events");
    let parked = events
        .iter()
        .find(|event| event.kind == "run_parked")
        .expect("parked event");
    let payload: serde_json::Value =
        serde_json::from_str(&parked.payload).expect("event payload json");
    let allocation = &payload["scheduler"]["implement"][0];
    assert_eq!(payload["node"], "implement");
    assert_eq!(allocation["requestedRole"], "coding");
    assert_eq!(allocation["agentId"], "codex-coding-1");
    assert_eq!(allocation["schedulerStatus"], "routed");
    assert!(
        allocation["schedulerReservationId"].as_str().is_some(),
        "event payload includes scheduler reservation metadata: {payload}"
    );
}

#[tokio::test]
async fn production_workflow_scheduler_release_on_agent_completion() {
    let observed = production_host_with_scheduler_allocator(0).await;
    let host = &observed.host;
    register_online_agent(host, "codex-coding-1", "coding", "medium").await;
    let workflow = write_p230_codergen_workflow(&observed.dir);
    let run = RunId::from_string("p230-release");
    run_repo::record_run(
        host.store().pool(),
        &run,
        workflow.to_string_lossy().as_ref(),
        "sha",
    )
    .await
    .expect("record");

    host.start_run(&run).await.expect("start");
    let assignment = host
        .open_task(&run, &NodeId::parsed("implement"))
        .await
        .expect("open task")
        .expect("open implement task");
    assert_eq!(assignment.agent_id, "codex-coding-1");

    agent_submit_success(host, "p230-release", "implement")
        .await
        .expect("submit implement");
    let busy_after_release = agent_scheduler_repo::pool_snapshot(
        host.store().pool(),
        agent_scheduler_repo::PoolFilters {
            role: Some("coding".to_string()),
            capability: Some("medium".to_string()),
            state: Some("busy".to_string()),
        },
    )
    .await
    .expect("busy pool snapshot after release");
    assert_eq!(
        busy_after_release.total, 0,
        "completion releases the scheduler reservation"
    );

    let replay = agent_submit_success(host, "p230-release", "implement").await;
    assert!(replay.is_err(), "replayed outcome is rejected");
    let busy_after_replay = agent_scheduler_repo::pool_snapshot(
        host.store().pool(),
        agent_scheduler_repo::PoolFilters {
            role: Some("coding".to_string()),
            capability: Some("medium".to_string()),
            state: Some("busy".to_string()),
        },
    )
    .await
    .expect("busy pool snapshot after replay");
    assert_eq!(
        busy_after_replay.total, 0,
        "replay does not make the scheduler busy again"
    );
}

#[tokio::test]
async fn production_workflow_scheduler_reuses_registered_pane_without_spawn() {
    let (host, backend, dir) = production_host_with_recording_allocation_backend(0).await;
    agent_repo::register_agent(
        host.store().pool(),
        agent_repo::RegisterAgent {
            name: "codex-coding-1".to_string(),
            role: Some("coding".to_string()),
            capability: Some("medium".to_string()),
            runtime: Some("codex".to_string()),
            model: Some("codex-test".to_string()),
            native_runtime_ref: Some("native://rs_coding/ra_coding".to_string()),
            home_dir: None,
            workdir: Some("/tmp/codex-coding-1".to_string()),
            state_dir: None,
            server: Some("local".to_string()),
            runtime_profile: json!({"profile": "p231"}),
        },
    )
    .await
    .expect("register online agent");
    let workflow = write_p230_codergen_workflow(&dir);
    let run = RunId::from_string("p231-reuse");
    run_repo::record_run(
        host.store().pool(),
        &run,
        workflow.to_string_lossy().as_ref(),
        "sha",
    )
    .await
    .expect("record");

    let parked = host.start_run(&run).await.expect("start");
    assert!(
        matches!(parked, RunProgress::Parked { .. }),
        "scheduler-backed workflow parks at implement, got {parked:?}"
    );

    assert!(
        backend.spawns().is_empty(),
        "routed online workflow allocation must not plain-spawn a duplicate session"
    );
    let dispatches = backend.dispatches();
    assert_eq!(dispatches.len(), 1, "one allocation-aware dispatch");
    assert_eq!(dispatches[0].0.agent_id.as_str(), "codex-coding-1");
    assert_eq!(
        dispatches[0].1.runtime["tmuxTarget"],
        "agentd-codex-coding-1:0.0"
    );
    assert_eq!(
        dispatches[0].1.runtime["tmux_target"],
        "agentd-codex-coding-1:0.0"
    );
    assert_eq!(dispatches[0].1.runtime["runtime"], "codex");
    assert_eq!(dispatches[0].1.runtime["model"], "codex-test");
    assert_eq!(dispatches[0].1.runtime["workdir"], "/tmp/codex-coding-1");
    assert_eq!(dispatches[0].1.runtime["runtimeProfile"]["profile"], "p231");

    let events = host.events_from(&run, 0).await.expect("events");
    let parked = events
        .iter()
        .find(|event| event.kind == "run_parked")
        .expect("parked event");
    let payload: serde_json::Value =
        serde_json::from_str(&parked.payload).expect("event payload json");
    let allocation = &payload["scheduler"]["implement"][0];
    assert_eq!(
        allocation["runtime"]["tmuxTarget"],
        "agentd-codex-coding-1:0.0"
    );
    assert_eq!(
        allocation["runtime"]["tmux_target"],
        "agentd-codex-coding-1:0.0"
    );
}

#[tokio::test]
async fn production_workflow_scheduler_drains_queued_codergen_ticket_on_release() {
    let (host, backend, dir) = production_host_with_recording_allocation_backend(0).await;
    register_online_agent(&host, "codex-coding-1", "coding", "medium").await;
    let workflow = write_p230_codergen_workflow(&dir);
    let busy_run = RunId::from_string("p232-busy");
    let queued_run = RunId::from_string("p232-queued");
    for run in [&busy_run, &queued_run] {
        run_repo::record_run(
            host.store().pool(),
            run,
            workflow.to_string_lossy().as_ref(),
            "sha",
        )
        .await
        .expect("record run");
    }

    host.start_run(&busy_run).await.expect("start busy run");
    assert_eq!(
        backend.dispatches().len(),
        1,
        "first run dispatches the only online coding agent"
    );

    let queued_progress = host.start_run(&queued_run).await.expect("start queued run");
    assert!(
        matches!(queued_progress, RunProgress::Parked { .. }),
        "queued run parks instead of failing, got {queued_progress:?}"
    );
    assert_eq!(
        backend.dispatches().len(),
        1,
        "queued run must not dispatch until scheduler release drains it"
    );
    assert_eq!(
        scheduler_queue_status_count(&host, "queued").await,
        1,
        "second run creates one durable queued scheduler ticket"
    );
    let queued_assignment = host
        .open_task(&queued_run, &NodeId::parsed("implement"))
        .await
        .expect("open queued task")
        .expect("queued task is open");
    assert_eq!(
        queued_assignment.agent_id, "",
        "queued task is not owned until drain assigns the freed agent"
    );

    agent_submit_success(&host, "p232-busy", "implement")
        .await
        .expect("submit busy run");

    let dispatches = backend.dispatches();
    assert_eq!(
        dispatches.len(),
        2,
        "release-drain dispatches the queued run exactly once"
    );
    let drained = dispatch_for_run(&dispatches, "p232-queued");
    assert_eq!(drained.0.agent_id.as_str(), "codex-coding-1");
    assert_eq!(drained.1.status, AgentAllocationStatus::Drained);
    assert!(
        drained.1.ticket.as_deref().is_some(),
        "drained allocation keeps the scheduler ticket"
    );
    assert!(
        drained
            .0
            .initial_prompt
            .as_deref()
            .expect("queued prompt")
            .contains("agentd_scheduler_status: drained"),
        "queued prompt includes drained scheduler metadata: {:?}",
        drained.0.initial_prompt
    );

    let assigned_after_drain = host
        .open_task(&queued_run, &NodeId::parsed("implement"))
        .await
        .expect("open queued task after drain")
        .expect("queued task remains open");
    assert_eq!(
        assigned_after_drain.task_run_id, queued_assignment.task_run_id,
        "drain reuses the original task-run id"
    );
    assert_eq!(assigned_after_drain.agent_id, "codex-coding-1");
    assert_eq!(
        scheduler_queue_status_count(&host, "drained").await,
        1,
        "queued ticket is marked drained after wakeup"
    );

    let payload = latest_run_parked_payload(&host, &queued_run).await;
    let allocations = payload["scheduler"]["implement"]
        .as_array()
        .expect("scheduler allocations");
    assert!(
        allocations
            .iter()
            .any(|allocation| allocation["schedulerStatus"] == "drained"
                && allocation["agentId"] == "codex-coding-1"
                && allocation["schedulerReservationId"].as_str().is_some()),
        "latest queued run park event exposes drained allocation: {payload}"
    );
}

#[tokio::test]
async fn production_workflow_scheduler_queued_codergen_wakeup_is_idempotent() {
    let (host, backend, dir) = production_host_with_recording_allocation_backend(0).await;
    register_online_agent(&host, "codex-coding-1", "coding", "medium").await;
    let workflow = write_p230_codergen_workflow(&dir);
    let busy_run = RunId::from_string("p232-idem-busy");
    let queued_run = RunId::from_string("p232-idem-queued");
    for run in [&busy_run, &queued_run] {
        run_repo::record_run(
            host.store().pool(),
            run,
            workflow.to_string_lossy().as_ref(),
            "sha",
        )
        .await
        .expect("record run");
    }

    host.start_run(&busy_run).await.expect("start busy run");
    host.start_run(&queued_run).await.expect("start queued run");
    agent_submit_success(&host, "p232-idem-busy", "implement")
        .await
        .expect("submit busy run");
    let after_first_wakeup = backend.dispatches().len();
    assert_eq!(
        after_first_wakeup, 2,
        "setup produced one original dispatch and one queued wakeup dispatch"
    );

    let replay = agent_submit_success(&host, "p232-idem-busy", "implement").await;
    assert!(replay.is_err(), "replayed completion is rejected");
    assert_eq!(
        backend.dispatches().len(),
        after_first_wakeup,
        "replayed completion must not dispatch the queued run again"
    );
    assert_eq!(
        scheduler_queue_status_count(&host, "queued").await,
        0,
        "woken queue does not recreate a queued ticket"
    );
    assert_eq!(
        scheduler_queue_status_count(&host, "drained").await,
        1,
        "woken queue keeps one drained ticket"
    );
}

#[tokio::test]
async fn production_workflow_scheduler_drains_queued_fanout_reviewer_ticket_on_release() {
    let (host, backend, dir) = production_host_with_recording_allocation_backend(0).await;
    register_online_agent(&host, "codex-review-1", "review", "medium").await;
    let workflow = write_p233_fanout_workflow(&dir);
    let busy_run = RunId::from_string("p233-busy-review");
    let queued_run = RunId::from_string("p233-queued-review");
    for run in [&busy_run, &queued_run] {
        run_repo::record_run(
            host.store().pool(),
            run,
            workflow.to_string_lossy().as_ref(),
            "sha",
        )
        .await
        .expect("record run");
    }

    host.start_run(&busy_run).await.expect("start busy run");
    assert_eq!(
        backend.dispatches().len(),
        1,
        "first review run dispatches the only online review agent"
    );

    let queued_progress = host.start_run(&queued_run).await.expect("start queued run");
    assert!(
        matches!(queued_progress, RunProgress::Parked { .. }),
        "queued review run parks instead of failing, got {queued_progress:?}"
    );
    assert!(
        backend.spawns().is_empty(),
        "scheduler-routed reviewers use allocation-aware dispatch, not plain spawn"
    );
    assert_eq!(
        backend.dispatches().len(),
        1,
        "queued fan_out reviewer must not dispatch until scheduler release drains it"
    );
    assert_eq!(
        scheduler_queue_status_count(&host, "queued").await,
        1,
        "second review run creates one durable queued scheduler ticket"
    );

    let busy_review_run_id = review_repo::find_open_review_run(host.store().pool(), &busy_run)
        .await
        .expect("find busy review run")
        .expect("busy review run is open");
    let queued_review_run_id = review_repo::find_open_review_run(host.store().pool(), &queued_run)
        .await
        .expect("find queued review run")
        .expect("queued review run is open");

    agent_submit_review(&host, busy_review_run_id.as_str(), "codex-review-1")
        .await
        .expect("submit busy reviewer");

    let dispatches = backend.dispatches();
    assert_eq!(
        dispatches.len(),
        2,
        "release-drain dispatches the queued reviewer exactly once"
    );
    let drained = dispatch_for_review_run(&dispatches, queued_review_run_id.as_str());
    assert_eq!(drained.0.agent_id.as_str(), "codex-review-1");
    assert_eq!(drained.1.status, AgentAllocationStatus::Drained);
    assert!(
        drained.1.ticket.as_deref().is_some(),
        "drained reviewer allocation keeps the scheduler ticket"
    );
    let prompt = drained.0.initial_prompt.as_deref().expect("queued prompt");
    assert!(
        prompt.contains("agentd_scheduler_status: drained"),
        "queued reviewer prompt includes drained scheduler metadata: {prompt}"
    );
    assert!(
        prompt.contains("agentd_reviewer_id: codex-review-1"),
        "queued reviewer prompt uses the freed agent id: {prompt}"
    );
    assert!(
        prompt.contains(&format!(
            "agentd_review_run_id: {}",
            queued_review_run_id.as_str()
        )),
        "queued reviewer prompt targets the original review run: {prompt}"
    );
    assert_eq!(
        scheduler_queue_status_count(&host, "drained").await,
        1,
        "queued reviewer ticket is marked drained after wakeup"
    );

    let payload = latest_run_parked_payload(&host, &queued_run).await;
    let allocations = payload["scheduler"]["review"]
        .as_array()
        .expect("scheduler allocations");
    assert!(
        allocations
            .iter()
            .any(|allocation| allocation["schedulerStatus"] == "drained"
                && allocation["agentId"] == "codex-review-1"
                && allocation["schedulerReservationId"].as_str().is_some()),
        "latest queued review park event exposes drained allocation: {payload}"
    );
}

#[tokio::test]
async fn production_workflow_scheduler_queued_fanout_wakeup_is_idempotent() {
    let (host, backend, dir) = production_host_with_recording_allocation_backend(0).await;
    register_online_agent(&host, "codex-review-1", "review", "medium").await;
    let workflow = write_p233_fanout_workflow(&dir);
    let busy_run = RunId::from_string("p233-idem-busy-review");
    let queued_run = RunId::from_string("p233-idem-queued-review");
    for run in [&busy_run, &queued_run] {
        run_repo::record_run(
            host.store().pool(),
            run,
            workflow.to_string_lossy().as_ref(),
            "sha",
        )
        .await
        .expect("record run");
    }

    host.start_run(&busy_run).await.expect("start busy run");
    host.start_run(&queued_run).await.expect("start queued run");
    let busy_review_run_id = review_repo::find_open_review_run(host.store().pool(), &busy_run)
        .await
        .expect("find busy review run")
        .expect("busy review run is open");
    agent_submit_review(&host, busy_review_run_id.as_str(), "codex-review-1")
        .await
        .expect("submit busy reviewer");
    let after_first_wakeup = backend.dispatches().len();
    assert_eq!(
        after_first_wakeup, 2,
        "setup produced one original review dispatch and one queued reviewer wakeup dispatch"
    );

    let _ = agent_submit_review(&host, busy_review_run_id.as_str(), "codex-review-1").await;
    assert_eq!(
        backend.dispatches().len(),
        after_first_wakeup,
        "replayed review completion must not dispatch the queued reviewer again"
    );
    assert_eq!(
        scheduler_queue_status_count(&host, "queued").await,
        0,
        "woken reviewer queue does not recreate a queued ticket"
    );
    assert_eq!(
        scheduler_queue_status_count(&host, "drained").await,
        1,
        "woken reviewer queue keeps one drained ticket"
    );
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

async fn scheduler_queue_status_count(host: &ProductionRunHost, status: &str) -> i64 {
    agent_scheduler_repo::queue_status_count(host.store().pool(), status)
        .await
        .expect("scheduler queue status count")
}

fn dispatch_for_run<'a>(
    dispatches: &'a [(SpawnRequest, AgentAllocation)],
    run_id: &str,
) -> &'a (SpawnRequest, AgentAllocation) {
    dispatches
        .iter()
        .find(|(req, _)| {
            req.initial_prompt
                .as_deref()
                .is_some_and(|prompt| prompt.contains(&format!("agentd_run_id: {run_id}")))
        })
        .unwrap_or_else(|| panic!("missing dispatch for run {run_id}: {dispatches:?}"))
}

fn dispatch_for_review_run<'a>(
    dispatches: &'a [(SpawnRequest, AgentAllocation)],
    review_run_id: &str,
) -> &'a (SpawnRequest, AgentAllocation) {
    dispatches
        .iter()
        .find(|(req, _)| {
            req.initial_prompt.as_deref().is_some_and(|prompt| {
                prompt.contains(&format!("agentd_review_run_id: {review_run_id}"))
            })
        })
        .unwrap_or_else(|| {
            panic!("missing dispatch for review run {review_run_id}: {dispatches:?}")
        })
}

async fn latest_run_parked_payload(host: &ProductionRunHost, run: &RunId) -> serde_json::Value {
    let events = host.events_from(run, 0).await.expect("events");
    let latest_parked = events
        .iter()
        .rev()
        .find(|event| event.kind == "run_parked")
        .expect("run has parked events");
    serde_json::from_str(&latest_parked.payload).expect("parked payload json")
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
async fn production_runhost_execute_tools_use_stable_repo_cwd_after_review_fan_in() {
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
    assert_eq!(
        calls[publish_idx].cwd,
        Some(repo_root()),
        "publish_branch must not inherit a transient reviewer/MCP cwd"
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
    assert_eq!(
        calls[open_pr_idx].cwd,
        Some(repo_root()),
        "open_pr must not inherit a transient reviewer/MCP cwd"
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

    let progress = host
        .start_run(&run)
        .await
        .expect("allocator failure is recorded as failed progress");
    match progress {
        RunProgress::Failed { reason, .. } => assert!(
            reason.contains("injected allocator failure"),
            "failed reason should contain allocator error: {reason}"
        ),
        other => panic!("expected failed progress, got {other:?}"),
    }

    let snap = host
        .run_snapshot(&run)
        .await
        .expect("snapshot")
        .expect("run exists");
    assert_eq!(snap.status, "failed");
    let events = host.events_from(&run, 0).await.expect("events");
    assert_eq!(events.len(), 1, "one terminal failure event: {events:?}");
    assert_eq!(events[0].kind, "run_failed");
    assert!(
        events[0].payload.contains("injected allocator failure"),
        "event payload includes allocator error: {}",
        events[0].payload
    );
    assert!(
        observed.runner.calls().iter().all(|c| {
            !(c.program == "agent-spec" && c.args.first().is_some_and(|a| a == "lifecycle"))
        }),
        "verify_lifecycle must not run after allocator failure"
    );
}

#[tokio::test]
async fn production_runhost_backend_failure_marks_run_failed_and_emits_event() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let host = ProductionRunHost::new(
        store,
        Box::new(FailingBackend),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    )
    .with_worktree_allocator(Some(Box::new(StaticAllocator::new("/tmp/agentd-task-wt"))));
    let run = RunId::from_string("e-backend-fail");
    run_repo::record_run(host.store().pool(), &run, "execute.dot", "sha")
        .await
        .expect("record");

    let progress = host
        .start_run(&run)
        .await
        .expect("backend launch failure is recorded as failed progress");
    match progress {
        RunProgress::Failed { reason, .. } => assert!(
            reason.contains("injected backend spawn failure"),
            "failed reason should contain backend error: {reason}"
        ),
        other => panic!("expected failed progress, got {other:?}"),
    }

    let snap = host
        .run_snapshot(&run)
        .await
        .expect("snapshot")
        .expect("run exists");
    assert_eq!(snap.status, "failed");
    let events = host.events_from(&run, 0).await.expect("events");
    assert_eq!(events.len(), 1, "one terminal failure event: {events:?}");
    assert_eq!(events[0].kind, "run_failed");
    assert!(
        events[0].payload.contains("injected backend spawn failure"),
        "event payload includes backend error: {}",
        events[0].payload
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
        provider.paths(),
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
        provider.paths(),
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
        provider.paths(),
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
