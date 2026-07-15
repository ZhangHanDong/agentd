//! Tests for the park-style handlers (`wait.human`/`fan_out`/`fan_in`/
//! `codergen`). Names match the spec `Test:` selectors. Requires `test-support`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use agentd_core::CoreError;
use agentd_core::dot::parser;
use agentd_core::engine::{EngineEvent, HandlerStep, ParkReason};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{
    CodergenHandler, FanInHandler, FanOutHandler, Handler, HandlerCtx, Ports, WaitHumanHandler,
};
use agentd_core::ports::{
    AgentAllocation, AgentAllocationRequest, AgentAllocationStatus, AgentAllocator, AgentBackend,
    DrawerHit, Store, WorktreeAllocator,
};
use agentd_core::test_support::{
    FakeBackend, FixedClock, InMemoryStore, MempalStub, RecordingCommandRunner,
};
use agentd_core::types::{
    AgentHandle, AgentId, BackendKind, CliKind, NodeId, Outcome, ReviewVerdict, RunContext, RunId,
    SpawnRequest, Status, VerdictValue,
};
use std::collections::VecDeque;

struct Deps {
    run_id: RunId,
    backend: FakeBackend,
    runner: RecordingCommandRunner,
    store: InMemoryStore,
    mempal: MempalStub,
    clock: FixedClock,
}

#[derive(Debug)]
struct RecordingAllocator {
    requests: Mutex<Vec<AgentAllocationRequest>>,
    releases: Mutex<Vec<String>>,
    responses: Mutex<VecDeque<AgentAllocation>>,
}

impl RecordingAllocator {
    fn new(responses: Vec<AgentAllocation>) -> Self {
        Self {
            requests: Mutex::new(Vec::new()),
            releases: Mutex::new(Vec::new()),
            responses: Mutex::new(VecDeque::from(responses)),
        }
    }

    fn requests(&self) -> Vec<AgentAllocationRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}
#[async_trait::async_trait]
impl AgentAllocator for RecordingAllocator {
    async fn allocate(&self, req: AgentAllocationRequest) -> Result<AgentAllocation, CoreError> {
        self.requests
            .lock()
            .expect("request lock")
            .push(req.clone());
        self.responses
            .lock()
            .expect("response lock")
            .pop_front()
            .ok_or_else(|| CoreError::Backend("missing test allocation response".to_string()))
    }

    async fn release(&self, agent_id: &AgentId) -> Result<Option<AgentAllocation>, CoreError> {
        self.releases
            .lock()
            .expect("release lock")
            .push(agent_id.as_str().to_string());
        Ok(None)
    }
}

fn routed_allocation(role: &str, agent_id: &str, reservation_id: &str) -> AgentAllocation {
    AgentAllocation {
        requested_role: role.to_string(),
        agent_id: AgentId::parsed(agent_id),
        status: AgentAllocationStatus::Routed,
        tier: Some("medium".to_string()),
        reservation_id: Some(reservation_id.to_string()),
        ticket: None,
        provisioned_name: None,
        runtime: serde_json::json!({}),
    }
}

fn queued_allocation(role: &str, ticket: &str) -> AgentAllocation {
    AgentAllocation {
        requested_role: role.to_string(),
        agent_id: AgentId::parsed(role),
        status: AgentAllocationStatus::Queued,
        tier: Some("medium".to_string()),
        reservation_id: None,
        ticket: Some(ticket.to_string()),
        provisioned_name: None,
        runtime: serde_json::json!({}),
    }
}

#[derive(Debug)]
struct FailingBackend;

#[async_trait::async_trait]
impl AgentBackend for FailingBackend {
    async fn spawn(&self, _req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        Err(CoreError::Backend("injected spawn failure".to_string()))
    }
}

#[derive(Debug, Default)]
struct RecordingDispatchBackend {
    spawns: Mutex<Vec<SpawnRequest>>,
    dispatches: Mutex<Vec<(SpawnRequest, AgentAllocation)>>,
}

impl RecordingDispatchBackend {
    fn spawns(&self) -> Vec<SpawnRequest> {
        self.spawns.lock().expect("spawn lock").clone()
    }

    fn dispatches(&self) -> Vec<(SpawnRequest, AgentAllocation)> {
        self.dispatches.lock().expect("dispatch lock").clone()
    }
}

#[async_trait::async_trait]
impl AgentBackend for RecordingDispatchBackend {
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        self.spawns.lock().expect("spawn lock").push(req);
        Err(CoreError::Backend(
            "plain spawn should not be used for routed allocation".to_string(),
        ))
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
        Ok(AgentHandle {
            agent_id: req.agent_id,
            backend: BackendKind::Tmux,
            address: allocation
                .runtime
                .get("tmuxTarget")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("fake://allocated")
                .to_string(),
            pane_id: Some("%9".to_string()),
            pid: Some(9001),
            session_name: "agentd-codex-coding-1".to_string(),
            spawned_at: SystemTime::UNIX_EPOCH,
        })
    }
}

struct FailingBackendDeps {
    run_id: RunId,
    backend: FailingBackend,
    runner: RecordingCommandRunner,
    store: InMemoryStore,
    mempal: MempalStub,
    clock: FixedClock,
}

impl FailingBackendDeps {
    fn new() -> Self {
        Self {
            run_id: RunId::from_string("r"),
            backend: FailingBackend,
            runner: RecordingCommandRunner::new(),
            store: InMemoryStore::new(),
            mempal: MempalStub::new(),
            clock: FixedClock::new(0),
        }
    }

    fn ports(&self) -> Ports<'_> {
        Ports {
            backend: &self.backend,
            runner: &self.runner,
            store: &self.store,
            mempal: &self.mempal,
            clock: &self.clock,
            agent_allocator: &agentd_core::ports::DirectAgentAllocator,
        }
    }
}

impl Deps {
    fn new() -> Self {
        Self {
            run_id: RunId::from_string("r"),
            backend: FakeBackend::new(),
            runner: RecordingCommandRunner::new(),
            store: InMemoryStore::new(),
            mempal: MempalStub::new(),
            clock: FixedClock::new(0),
        }
    }

    fn ports(&self) -> Ports<'_> {
        Ports {
            backend: &self.backend,
            runner: &self.runner,
            store: &self.store,
            mempal: &self.mempal,
            clock: &self.clock,
            agent_allocator: &agentd_core::ports::DirectAgentAllocator,
        }
    }

    fn ports_with_allocator<'a>(&'a self, allocator: &'a dyn AgentAllocator) -> Ports<'a> {
        Ports {
            backend: &self.backend,
            runner: &self.runner,
            store: &self.store,
            mempal: &self.mempal,
            clock: &self.clock,
            agent_allocator: allocator,
        }
    }
}

#[derive(Debug, Clone)]
struct RecordingSnapshotAllocator {
    base: PathBuf,
    fail_snapshots: bool,
    snapshots: Arc<Mutex<Vec<(String, PathBuf, PathBuf)>>>,
    releases: Arc<Mutex<Vec<(String, PathBuf)>>>,
}

impl RecordingSnapshotAllocator {
    fn new(base: impl Into<PathBuf>) -> Self {
        Self {
            base: base.into(),
            fail_snapshots: false,
            snapshots: Arc::new(Mutex::new(Vec::new())),
            releases: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn failing(base: impl Into<PathBuf>) -> Self {
        Self {
            fail_snapshots: true,
            ..Self::new(base)
        }
    }

    fn releases(&self) -> Vec<(String, PathBuf)> {
        self.releases.lock().expect("release lock").clone()
    }
}

#[async_trait::async_trait]
impl WorktreeAllocator for RecordingSnapshotAllocator {
    async fn allocate(&self, key: &str) -> Result<PathBuf, CoreError> {
        Ok(self.base.join(format!("implement-{key}")))
    }

    async fn allocate_snapshot(&self, key: &str, source: &Path) -> Result<PathBuf, CoreError> {
        if self.fail_snapshots {
            return Err(CoreError::Backend("snapshot boom".to_string()));
        }
        let path = self.base.join(format!("snapshot-{key}"));
        self.snapshots.lock().expect("snapshot lock").push((
            key.to_string(),
            source.to_path_buf(),
            path.clone(),
        ));
        Ok(path)
    }

    async fn release(&self, key: &str, path: &Path) -> Result<(), CoreError> {
        self.releases
            .lock()
            .expect("release lock")
            .push((key.to_string(), path.to_path_buf()));
        Ok(())
    }
}

#[derive(Debug)]
struct FixedTaskAllocator {
    path: PathBuf,
}

impl FixedTaskAllocator {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

#[async_trait::async_trait]
impl WorktreeAllocator for FixedTaskAllocator {
    async fn allocate(&self, _key: &str) -> Result<PathBuf, CoreError> {
        Ok(self.path.clone())
    }

    async fn release(&self, _key: &str, _path: &Path) -> Result<(), CoreError> {
        Ok(())
    }
}

fn graph(src: &str) -> NodeGraph {
    let ast = parser::parse(src).expect("dot parse");
    NodeGraph::from_ast_unvalidated(&ast)
}

fn done(step: HandlerStep) -> Outcome {
    match step {
        HandlerStep::Done(o) => o,
        HandlerStep::Park(r) => panic!("expected Done, parked with {r:?}"),
    }
}

// ---- wait.human -----------------------------------------------------------

fn wait_graph() -> NodeGraph {
    graph(
        r#"digraph m {
            "ask" [handler="wait.human", prompt="approve?"];
        }"#,
    )
}

#[tokio::test]
async fn wait_human_run_parks_with_human_answer_reason() {
    let g = wait_graph();
    let node = g.node("ask").expect("ask node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let step = WaitHumanHandler.run(&mut ctx).await.expect("run");
    let wait_id = match step {
        HandlerStep::Park(ParkReason::HumanAnswer { wait_id }) => wait_id,
        other => panic!("expected HumanAnswer park, got {other:?}"),
    };
    // The wait is open in the store and resolves back to (run, node).
    let parked = deps
        .store
        .lookup_park_by_wait_id(&wait_id)
        .await
        .expect("lookup");
    assert_eq!(
        parked,
        Some((RunId::from_string("r"), NodeId::parsed("ask")))
    );
}

#[tokio::test]
async fn wait_human_resume_stages_answer_and_returns_done() {
    let g = wait_graph();
    let node = g.node("ask").expect("ask node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let wait_id = match WaitHumanHandler.run(&mut ctx).await.expect("run") {
        HandlerStep::Park(ParkReason::HumanAnswer { wait_id }) => wait_id,
        other => panic!("expected park, got {other:?}"),
    };
    let step = WaitHumanHandler
        .resume(
            &mut ctx,
            EngineEvent::HumanAnswered {
                wait_id,
                answer: "approve".to_string(),
                feedback: None,
            },
        )
        .await
        .expect("resume");
    assert!(matches!(step, HandlerStep::Done(_)));
    assert_eq!(
        ctx.staged_updates()
            .get("answer")
            .and_then(serde_json::Value::as_str),
        Some("approve")
    );
}

#[tokio::test]
async fn wait_human_resume_sets_preferred_label_to_answer() {
    let g = wait_graph();
    let node = g.node("ask").expect("ask node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let wait_id = match WaitHumanHandler.run(&mut ctx).await.expect("run") {
        HandlerStep::Park(ParkReason::HumanAnswer { wait_id }) => wait_id,
        other => panic!("expected park, got {other:?}"),
    };
    let outcome = done(
        WaitHumanHandler
            .resume(
                &mut ctx,
                EngineEvent::HumanAnswered {
                    wait_id,
                    answer: "approve".to_string(),
                    feedback: Some("looks good".to_string()),
                },
            )
            .await
            .expect("resume"),
    );
    assert_eq!(outcome.preferred_label.as_deref(), Some("approve"));
}

// ---- fan_out --------------------------------------------------------------

fn fan_out_graph() -> NodeGraph {
    graph(
        r#"digraph m {
            "review" [handler="parallel.fan_out", reviewers="claude-sec,codex-perf,gemini-readability"];
        }"#,
    )
}

fn delphi_fan_out_graph() -> NodeGraph {
    graph(
        r#"digraph m {
            "review" [handler="parallel.fan_out", reviewers="claude-sec,codex-perf", visibility=delphi, max_rounds=3];
        }"#,
    )
}

fn stance_pack_fan_out_graph() -> NodeGraph {
    graph(
        r#"digraph m {
            "review" [
                handler="parallel.fan_out",
                reviewers="claude-sec,codex-perf",
                stance_queries="claude-sec=security review memories;codex-perf=performance review memories"
            ];
        }"#,
    )
}

fn prompt_profile_fan_out_graph() -> NodeGraph {
    graph(
        r#"digraph m {
            "review" [
                handler="parallel.fan_out",
                reviewers="claude-sec,codex-perf",
                prompt_profiles="claude-sec=security-hardening;codex-perf=runtime-performance"
            ];
        }"#,
    )
}

fn hit(id: &str, body: &str) -> DrawerHit {
    DrawerHit {
        drawer_id: id.to_string(),
        body: body.to_string(),
        score: 1.0,
    }
}

fn reviewer_prompt(deps: &Deps, reviewer: &str) -> String {
    deps.backend
        .spawned()
        .into_iter()
        .find(|req| req.agent_id.as_str() == reviewer)
        .and_then(|req| req.initial_prompt)
        .unwrap_or_else(|| panic!("no prompt for reviewer {reviewer}"))
}

fn prompt_context_sha(prompt: &str) -> &str {
    let start = prompt
        .find("context_sha=")
        .map(|idx| idx + "context_sha=".len())
        .expect("prompt has context_sha");
    let rest = &prompt[start..];
    let end = rest.find([')', ';', '\n']).unwrap_or(rest.len());
    &rest[..end]
}

#[tokio::test]
async fn fan_out_run_parks_with_expected_reviewer_count() {
    let g = fan_out_graph();
    let node = g.node("review").expect("review node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let step = FanOutHandler.run(&mut ctx).await.expect("run");
    match step {
        HandlerStep::Park(ParkReason::ReviewVerdicts { expected, .. }) => assert_eq!(expected, 3),
        other => panic!("expected ReviewVerdicts park, got {other:?}"),
    }
    assert_eq!(deps.backend.spawned().len(), 3, "three reviewers spawned");
}

#[tokio::test]
async fn fan_out_prompt_includes_review_submission_context() {
    let g = graph(
        r#"digraph m {
            "review" [handler="parallel.fan_out", reviewers="claude-sec,codex-perf"];
        }"#,
    );
    let node = g.node("review").expect("review node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let step = FanOutHandler.run(&mut ctx).await.expect("run");
    let review_run_id = match step {
        HandlerStep::Park(ParkReason::ReviewVerdicts { review_run_id, .. }) => review_run_id,
        other => panic!("expected ReviewVerdicts park, got {other:?}"),
    };

    let spawned = deps.backend.spawned();
    assert_eq!(spawned.len(), 2, "two reviewers spawned");
    for req in spawned {
        let prompt = req.initial_prompt.as_deref().expect("review prompt");
        assert!(prompt.contains("agentd_run_id: r"), "{prompt}");
        assert!(prompt.contains("agentd_node_id: review"), "{prompt}");
        assert!(
            prompt.contains(&format!("agentd_reviewer_id: {}", req.agent_id.as_str())),
            "{prompt}"
        );
        assert!(
            prompt.contains(&format!("agentd_review_run_id: {}", review_run_id.as_str())),
            "{prompt}"
        );
        assert!(prompt.contains("submit_review"), "{prompt}");
        assert!(prompt.contains("tools/call"), "{prompt}");
    }
}

#[tokio::test]
async fn fan_out_prompt_includes_review_runtime_context() {
    let g = fan_out_graph();
    let node = g.node("review").expect("review node");
    let mut context = RunContext::new();
    context.set(
        "spec_path",
        serde_json::Value::String(".agentd/run/frozen.spec.md".to_string()),
    );
    context.set(
        "plan_path",
        serde_json::Value::String(".agentd/run/plan.md".to_string()),
    );
    context.set(
        "worktree",
        serde_json::Value::String("/tmp/agentd-task-wt".to_string()),
    );
    let deps = Deps::new();
    let allocator = RecordingSnapshotAllocator::new("/tmp/reviews");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports())
        .with_worktree_allocator(Some(&allocator));

    FanOutHandler.run(&mut ctx).await.expect("run");

    for req in deps.backend.spawned() {
        let prompt = req.initial_prompt.as_deref().expect("review prompt");
        assert!(prompt.contains("agentd_daemon_cwd:"), "{prompt}");
        assert!(
            prompt.contains("spec_path: .agentd/run/frozen.spec.md"),
            "{prompt}"
        );
        assert!(
            prompt.contains("plan_path: .agentd/run/plan.md"),
            "{prompt}"
        );
        assert!(
            prompt.contains("implementation_worktree: /tmp/agentd-task-wt"),
            "{prompt}"
        );
        assert!(
            prompt.contains(&format!(
                "review_worktree: {}",
                req.worktree.to_string_lossy()
            )),
            "{prompt}"
        );
        assert!(
            prompt.contains("agentd_review_task: review the current worktree"),
            "{prompt}"
        );
        assert!(prompt.contains("pass|concern|blocker"), "{prompt}");
    }
}

#[tokio::test]
async fn fan_out_adds_distinct_stance_pack_to_each_reviewer_prompt() {
    let g = stance_pack_fan_out_graph();
    let node = g.node("review").expect("review node");
    let context = RunContext::new();
    let deps = Deps::new();
    deps.mempal.set_query_hits(
        "security review memories",
        vec![hit("d-sec", "security stale auth finding")],
    );
    deps.mempal.set_query_hits(
        "performance review memories",
        vec![hit("d-perf", "performance latency finding")],
    );
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    FanOutHandler.run(&mut ctx).await.expect("run");

    assert_eq!(
        deps.mempal.searches(),
        vec![
            (
                "security review memories".to_string(),
                "project".to_string(),
                String::new()
            ),
            (
                "performance review memories".to_string(),
                "project".to_string(),
                String::new()
            )
        ],
        "one mempal search per reviewer query"
    );
    let sec_prompt = reviewer_prompt(&deps, "claude-sec");
    let perf_prompt = reviewer_prompt(&deps, "codex-perf");
    assert!(
        sec_prompt.contains("stance_pack_query: security review memories"),
        "security prompt carries its stance query: {sec_prompt}"
    );
    assert!(
        sec_prompt.contains("[d-sec] security stale auth finding"),
        "security prompt carries its stance hit: {sec_prompt}"
    );
    assert!(
        !sec_prompt.contains("performance latency finding"),
        "security prompt must not leak performance pack: {sec_prompt}"
    );
    assert!(
        perf_prompt.contains("stance_pack_query: performance review memories"),
        "performance prompt carries its stance query: {perf_prompt}"
    );
    assert!(
        perf_prompt.contains("[d-perf] performance latency finding"),
        "performance prompt carries its stance hit: {perf_prompt}"
    );
    assert!(
        !perf_prompt.contains("security stale auth finding"),
        "performance prompt must not leak security pack: {perf_prompt}"
    );
    assert_eq!(
        prompt_context_sha(&sec_prompt),
        prompt_context_sha(&perf_prompt),
        "stance packs differ but the frozen context_sha stays shared"
    );
}

#[tokio::test]
async fn fan_out_adds_per_reviewer_prompt_profiles() {
    let g = prompt_profile_fan_out_graph();
    let node = g.node("review").expect("review node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    FanOutHandler.run(&mut ctx).await.expect("run");

    assert!(
        deps.mempal.searches().is_empty(),
        "prompt profiles alone should not trigger mempal search"
    );
    let sec_prompt = reviewer_prompt(&deps, "claude-sec");
    let perf_prompt = reviewer_prompt(&deps, "codex-perf");
    assert!(
        sec_prompt.contains("prompt_profile: security-hardening"),
        "security reviewer gets its prompt profile: {sec_prompt}"
    );
    assert!(
        !sec_prompt.contains("runtime-performance"),
        "security reviewer must not receive perf profile: {sec_prompt}"
    );
    assert!(
        perf_prompt.contains("prompt_profile: runtime-performance"),
        "perf reviewer gets its prompt profile: {perf_prompt}"
    );
    assert!(
        !perf_prompt.contains("security-hardening"),
        "perf reviewer must not receive security profile: {perf_prompt}"
    );
}

#[tokio::test]
async fn fan_out_rejects_incomplete_stance_query_map() {
    let g = graph(
        r#"digraph m {
            "review" [
                handler="parallel.fan_out",
                reviewers="claude-sec,codex-perf",
                stance_queries="claude-sec=security review memories"
            ];
        }"#,
    );
    let node = g.node("review").expect("review node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let err = FanOutHandler
        .run(&mut ctx)
        .await
        .expect_err("incomplete stance_queries map should be rejected");

    assert!(
        err.to_string().contains("stance_queries") && err.to_string().contains("codex-perf"),
        "error names the missing reviewer: {err}"
    );
    assert!(
        deps.backend.spawned().is_empty(),
        "no reviewer should spawn after stance_queries validation fails"
    );
}

#[tokio::test]
async fn fan_out_rejects_duplicate_stance_queries() {
    let g = graph(
        r#"digraph m {
            "review" [
                handler="parallel.fan_out",
                reviewers="claude-sec,codex-perf",
                stance_queries="claude-sec=shared review memories;codex-perf=shared review memories"
            ];
        }"#,
    );
    let node = g.node("review").expect("review node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let err = FanOutHandler
        .run(&mut ctx)
        .await
        .expect_err("duplicate stance query map should be rejected");

    assert!(
        err.to_string().contains("distinct stance_queries"),
        "error reports non-distinct stance queries: {err}"
    );
    assert!(
        deps.backend.spawned().is_empty(),
        "no reviewer should spawn after duplicate stance query validation fails"
    );
}

#[tokio::test]
async fn fan_out_resume_reparks_with_stored_round() {
    let g = fan_out_graph();
    let node = g.node("review").expect("review node");
    let context = RunContext::new();
    let deps = Deps::new();
    let review_run_id = deps
        .store
        .insert_review_run(&deps.run_id, &NodeId::parsed("review"), 3, 2, "csha")
        .await
        .expect("review run");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let step = FanOutHandler
        .resume(
            &mut ctx,
            EngineEvent::ReviewVerdictSubmitted {
                review_run_id: review_run_id.clone(),
                reviewer_id: AgentId::parsed("claude-sec"),
                verdict: VerdictValue::Pass,
                findings: String::new(),
            },
        )
        .await
        .expect("resume");

    match step {
        HandlerStep::Park(ParkReason::ReviewVerdicts {
            review_run_id: parked_id,
            expected,
            round,
        }) => {
            assert_eq!(parked_id, review_run_id);
            assert_eq!(expected, 3);
            assert_eq!(round, 2, "resume reuses the stored review round");
        }
        other => panic!("expected ReviewVerdicts park, got {other:?}"),
    }
}

#[tokio::test]
async fn fan_out_delphi_run_uses_context_next_round() {
    let g = delphi_fan_out_graph();
    let node = g.node("review").expect("review node");
    let mut context = RunContext::new();
    context.set(
        "delphi_next_round",
        serde_json::Value::Number(serde_json::Number::from(2)),
    );
    context.set(
        "delphi_previous_verdicts",
        serde_json::Value::String("a=pass;b=fail".to_string()),
    );
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let step = FanOutHandler.run(&mut ctx).await.expect("run");

    match step {
        HandlerStep::Park(ParkReason::ReviewVerdicts { round, .. }) => {
            assert_eq!(
                round, 2,
                "fan_out must park with the requested Delphi round"
            );
        }
        other => panic!("expected ReviewVerdicts park, got {other:?}"),
    }
    let prompts: Vec<String> = deps
        .backend
        .spawned()
        .iter()
        .map(|req| req.initial_prompt.clone().unwrap_or_default())
        .collect();
    assert!(
        prompts
            .iter()
            .all(|prompt| prompt.contains("Delphi round 2")),
        "round-2 prompts should name the Delphi round: {prompts:?}"
    );
    assert!(
        prompts
            .iter()
            .all(|prompt| prompt.contains("a=pass;b=fail")),
        "round-2 prompts should carry previous verdicts: {prompts:?}"
    );
}

#[tokio::test]
async fn fan_out_without_allocator_uses_staged_worktree() {
    let g = fan_out_graph();
    let node = g.node("review").expect("review node");
    let mut context = RunContext::new();
    context.set(
        "worktree",
        serde_json::Value::String("/tmp/agentd-task-wt".to_string()),
    );
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    FanOutHandler.run(&mut ctx).await.expect("run");

    let spawned = deps.backend.spawned();
    assert_eq!(spawned.len(), 3);
    assert!(
        spawned
            .iter()
            .all(|req| req.worktree == Path::new("/tmp/agentd-task-wt")),
        "without allocator, fan_out keeps the staged-worktree fallback: {spawned:?}"
    );
}

#[tokio::test]
async fn fan_out_reviewer_snapshot_failure_does_not_fall_back_to_shared_worktree() {
    let g = fan_out_graph();
    let node = g.node("review").expect("review node");
    let mut context = RunContext::new();
    context.set(
        "worktree",
        serde_json::Value::String("/tmp/agentd-task-wt".to_string()),
    );
    let deps = Deps::new();
    let allocator = RecordingSnapshotAllocator::failing("/tmp/reviews");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports())
        .with_worktree_allocator(Some(&allocator));

    let err = FanOutHandler
        .run(&mut ctx)
        .await
        .expect_err("snapshot allocation failure must be loud");

    assert!(
        err.to_string().contains("snapshot boom"),
        "allocator error should surface, got {err}"
    );
    assert!(
        deps.backend
            .spawned()
            .iter()
            .all(|req| { req.worktree != Path::new("/tmp/agentd-task-wt") }),
        "failed snapshot allocation must not fall back to the shared implementer worktree"
    );
}

#[tokio::test]
async fn fan_out_releases_reviewer_worktree_once_per_distinct_verdict() {
    let g = fan_out_graph();
    let node = g.node("review").expect("review node");
    let mut context = RunContext::new();
    context.set(
        "worktree",
        serde_json::Value::String("/tmp/agentd-task-wt".to_string()),
    );
    let deps = Deps::new();
    let allocator = RecordingSnapshotAllocator::new("/tmp/reviews");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports())
        .with_worktree_allocator(Some(&allocator));
    let review_run_id = match FanOutHandler.run(&mut ctx).await.expect("run") {
        HandlerStep::Park(ParkReason::ReviewVerdicts { review_run_id, .. }) => review_run_id,
        other => panic!("expected park, got {other:?}"),
    };

    let submit = |who: &str| EngineEvent::ReviewVerdictSubmitted {
        review_run_id: review_run_id.clone(),
        reviewer_id: AgentId::parsed(who),
        verdict: VerdictValue::Pass,
        findings: String::new(),
    };

    FanOutHandler
        .resume(&mut ctx, submit("claude-sec"))
        .await
        .expect("first reviewer");
    FanOutHandler
        .resume(&mut ctx, submit("claude-sec"))
        .await
        .expect("duplicate reviewer");
    FanOutHandler
        .resume(&mut ctx, submit("codex-perf"))
        .await
        .expect("second reviewer");
    FanOutHandler
        .resume(&mut ctx, submit("gemini-readability"))
        .await
        .expect("third reviewer");

    let releases = allocator.releases();
    assert_eq!(
        releases.len(),
        3,
        "one release per distinct reviewer, no duplicate release: {releases:?}"
    );
    let unique: std::collections::HashSet<_> = releases.iter().collect();
    assert_eq!(unique.len(), 3, "release keys and paths are distinct");
    assert!(
        releases
            .iter()
            .all(|(key, path)| key.starts_with("review-rr_") && path.starts_with("/tmp/reviews")),
        "release uses the same reviewer-scoped keys and snapshot paths: {releases:?}"
    );
}

#[tokio::test]
async fn fan_out_computes_deterministic_context_sha_in_memory() {
    let g = fan_out_graph();
    let node = g.node("review").expect("review node");
    let mut context = RunContext::new();
    context.set("k", serde_json::Value::String("v".to_string()));

    let sha = |deps: &Deps, ctx: &HandlerCtx<'_>| {
        ctx.staged_updates()
            .get("context_sha")
            .and_then(serde_json::Value::as_str)
            .map_or_else(
                || {
                    panic!(
                        "no context_sha staged (deps spawned {})",
                        deps.backend.spawned().len()
                    )
                },
                ToString::to_string,
            )
    };

    let deps1 = Deps::new();
    let mut ctx1 = HandlerCtx::new(&deps1.run_id, &g, node, &context, deps1.ports());
    FanOutHandler.run(&mut ctx1).await.expect("run 1");
    let sha1 = sha(&deps1, &ctx1);

    let deps2 = Deps::new();
    let mut ctx2 = HandlerCtx::new(&deps2.run_id, &g, node, &context, deps2.ports());
    FanOutHandler.run(&mut ctx2).await.expect("run 2");
    let sha2 = sha(&deps2, &ctx2);

    assert_eq!(sha1, sha2, "context_sha must be deterministic");
    assert_eq!(sha1.len(), 64, "hex sha256 is 64 chars");
}

#[tokio::test]
async fn fan_out_resume_stays_parked_until_all_verdicts_in() {
    let g = fan_out_graph();
    let node = g.node("review").expect("review node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let review_run_id = match FanOutHandler.run(&mut ctx).await.expect("run") {
        HandlerStep::Park(ParkReason::ReviewVerdicts { review_run_id, .. }) => review_run_id,
        other => panic!("expected park, got {other:?}"),
    };

    let reviewers = ["claude-sec", "codex-perf", "gemini-readability"];
    for (i, reviewer) in reviewers.iter().enumerate() {
        let step = FanOutHandler
            .resume(
                &mut ctx,
                EngineEvent::ReviewVerdictSubmitted {
                    review_run_id: review_run_id.clone(),
                    reviewer_id: AgentId::parsed(*reviewer),
                    verdict: VerdictValue::Pass,
                    findings: String::new(),
                },
            )
            .await
            .expect("resume");
        if i < 2 {
            assert!(
                matches!(step, HandlerStep::Park(_)),
                "verdict {i} should re-park"
            );
        } else {
            assert!(
                matches!(step, HandlerStep::Done(_)),
                "third verdict completes"
            );
        }
    }
}

// ---- fan_in ---------------------------------------------------------------

fn fan_in_graph(aggregator: &str) -> NodeGraph {
    graph(&format!(
        r#"digraph m {{
            "aggregate" [handler="parallel.fan_in", aggregator="{aggregator}"];
        }}"#
    ))
}

fn delphi_fan_in_graph(aggregator: &str, max_rounds: u32) -> NodeGraph {
    graph(&format!(
        r#"digraph m {{
            "review" [handler="parallel.fan_out", visibility=delphi, max_rounds={max_rounds}];
            "aggregate" [handler="parallel.fan_in", aggregator="{aggregator}"];
            "review" -> "aggregate";
        }}"#
    ))
}

fn delphi_fan_in_graph_with_convergence(
    aggregator: &str,
    max_rounds: u32,
    convergence: &str,
) -> NodeGraph {
    graph(&format!(
        r#"digraph m {{
            "review" [handler="parallel.fan_out", visibility=delphi, max_rounds={max_rounds}, convergence="{convergence}"];
            "aggregate" [handler="parallel.fan_in", aggregator="{aggregator}"];
            "review" -> "aggregate";
        }}"#
    ))
}

async fn seed_review(deps: &Deps, verdicts: &[(&str, VerdictValue)]) -> RunContext {
    seed_review_round(deps, 1, verdicts).await
}

async fn seed_review_round(
    deps: &Deps,
    round: u32,
    verdicts: &[(&str, VerdictValue)],
) -> RunContext {
    let rr = deps
        .store
        .insert_review_run(
            &deps.run_id,
            &NodeId::parsed("review"),
            verdicts.len(),
            round,
            "sha",
        )
        .await
        .expect("insert review run");
    for (who, value) in verdicts {
        deps.store
            .insert_review_verdict(
                &rr,
                ReviewVerdict {
                    reviewer_id: AgentId::parsed(*who),
                    value: *value,
                    findings: String::new(),
                },
            )
            .await
            .expect("insert verdict");
    }
    let mut context = RunContext::new();
    context.set(
        "review_run_id",
        serde_json::Value::String(rr.as_str().to_string()),
    );
    context
}

async fn seed_review_round_with_findings(
    deps: &Deps,
    round: u32,
    verdicts: &[(&str, VerdictValue, &str)],
) -> RunContext {
    let rr = deps
        .store
        .insert_review_run(
            &deps.run_id,
            &NodeId::parsed("review"),
            verdicts.len(),
            round,
            "sha",
        )
        .await
        .expect("insert review run");
    for (who, value, findings) in verdicts {
        deps.store
            .insert_review_verdict(
                &rr,
                ReviewVerdict {
                    reviewer_id: AgentId::parsed(*who),
                    value: *value,
                    findings: (*findings).to_string(),
                },
            )
            .await
            .expect("insert verdict");
    }
    let mut context = RunContext::new();
    context.set(
        "review_run_id",
        serde_json::Value::String(rr.as_str().to_string()),
    );
    context
}

#[tokio::test]
async fn fan_in_aggregator_majority_pass_returns_success_when_majority() {
    let deps = Deps::new();
    let context = seed_review(
        &deps,
        &[
            ("a", VerdictValue::Pass),
            ("b", VerdictValue::Pass),
            ("c", VerdictValue::Fail),
        ],
    )
    .await;
    let g = fan_in_graph("majority_pass");
    let node = g.node("aggregate").expect("aggregate node");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let outcome = done(FanInHandler.run(&mut ctx).await.expect("run"));
    assert_eq!(outcome.status, Status::Success);
}

#[tokio::test]
async fn fan_in_converge_or_majority_pass_uses_majority_fallback() {
    let deps = Deps::new();
    let context = seed_review_round(
        &deps,
        3,
        &[
            ("a", VerdictValue::Pass),
            ("b", VerdictValue::Pass),
            ("c", VerdictValue::Fail),
        ],
    )
    .await;
    let g = delphi_fan_in_graph("converge_or_majority_pass", 3);
    let node = g.node("aggregate").expect("aggregate node");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let outcome = done(FanInHandler.run(&mut ctx).await.expect("run"));
    assert_eq!(outcome.status, Status::Success);
}

#[tokio::test]
async fn fan_in_converge_or_majority_pass_requests_next_round_before_max() {
    let deps = Deps::new();
    let context = seed_review_round(
        &deps,
        1,
        &[
            ("a", VerdictValue::Pass),
            ("b", VerdictValue::Pass),
            ("c", VerdictValue::Fail),
        ],
    )
    .await;
    let g = delphi_fan_in_graph("converge_or_majority_pass", 3);
    let node = g.node("aggregate").expect("aggregate node");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let outcome = done(FanInHandler.run(&mut ctx).await.expect("run"));

    assert_eq!(outcome.status, Status::PartialSuccess);
    assert_eq!(
        outcome
            .context_updates
            .get("delphi_next_round")
            .and_then(serde_json::Value::as_u64),
        Some(2)
    );
    assert_eq!(
        outcome
            .context_updates
            .get("delphi_previous_verdicts")
            .and_then(serde_json::Value::as_str),
        Some("a=pass;b=pass;c=fail")
    );
}

#[tokio::test]
async fn fan_in_converge_or_majority_pass_finishes_when_verdicts_stabilize() {
    let deps = Deps::new();
    let mut context = seed_review_round(
        &deps,
        2,
        &[
            ("a", VerdictValue::Pass),
            ("b", VerdictValue::Pass),
            ("c", VerdictValue::Fail),
        ],
    )
    .await;
    context.set(
        "delphi_previous_verdicts",
        serde_json::Value::String("a=pass;b=pass;c=fail".to_string()),
    );
    let g = delphi_fan_in_graph("converge_or_majority_pass", 3);
    let node = g.node("aggregate").expect("aggregate node");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let outcome = done(FanInHandler.run(&mut ctx).await.expect("run"));

    assert_eq!(outcome.status, Status::Success);
    assert!(
        !outcome.context_updates.contains_key("delphi_next_round"),
        "stable Delphi verdicts should finish instead of requesting another round"
    );
}

#[tokio::test]
async fn fan_in_findings_diff_requests_next_round_when_findings_changed_above_threshold() {
    let deps = Deps::new();
    let mut context = seed_review_round_with_findings(
        &deps,
        2,
        &[
            ("a", VerdictValue::Pass, "new crash in parser"),
            ("b", VerdictValue::Fail, "new timeout in runner"),
            ("c", VerdictValue::Fail, "new data loss path"),
        ],
    )
    .await;
    context.set(
        "delphi_previous_verdicts",
        serde_json::Value::String("a=fail;b=fail;c=fail".to_string()),
    );
    context.set(
        "delphi_previous_findings",
        serde_json::Value::String("a=old note;b=old note;c=old note".to_string()),
    );
    let g =
        delphi_fan_in_graph_with_convergence("converge_or_majority_pass", 3, "findings_diff<0.1>");
    let node = g.node("aggregate").expect("aggregate node");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let outcome = done(FanInHandler.run(&mut ctx).await.expect("run"));

    assert_eq!(outcome.status, Status::PartialSuccess);
    assert_eq!(
        outcome
            .context_updates
            .get("delphi_next_round")
            .and_then(serde_json::Value::as_u64),
        Some(3)
    );
    let previous_findings = outcome
        .context_updates
        .get("delphi_previous_findings")
        .and_then(serde_json::Value::as_str)
        .expect("updated findings signature");
    assert!(previous_findings.contains("new crash in parser"));
}

#[tokio::test]
async fn fan_in_findings_diff_finishes_when_findings_change_below_threshold() {
    let deps = Deps::new();
    let mut context = seed_review_round_with_findings(
        &deps,
        2,
        &[
            ("a", VerdictValue::Pass, "parser loses location metadata"),
            ("b", VerdictValue::Pass, "runner timeout is bounded"),
            ("c", VerdictValue::Fail, "docs need a small wording update"),
        ],
    )
    .await;
    context.set(
        "delphi_previous_verdicts",
        serde_json::Value::String("a=fail;b=pass;c=fail".to_string()),
    );
    context.set(
        "delphi_previous_findings",
        serde_json::Value::String(
            "a=parser loses metadata;b=runner timeout is bounded;c=docs need wording update"
                .to_string(),
        ),
    );
    let g =
        delphi_fan_in_graph_with_convergence("converge_or_majority_pass", 3, "findings_diff<0.5>");
    let node = g.node("aggregate").expect("aggregate node");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let outcome = done(FanInHandler.run(&mut ctx).await.expect("run"));

    assert_eq!(outcome.status, Status::Success);
    assert!(
        !outcome.context_updates.contains_key("delphi_next_round"),
        "low findings diff should finish instead of requesting another round"
    );
}

#[tokio::test]
async fn fan_in_findings_diff_uses_fallback_at_max_rounds() {
    let deps = Deps::new();
    let mut context = seed_review_round_with_findings(
        &deps,
        3,
        &[
            ("a", VerdictValue::Pass, "brand new blocker text"),
            ("b", VerdictValue::Fail, "another unrelated finding"),
            ("c", VerdictValue::Fail, "third unrelated finding"),
        ],
    )
    .await;
    context.set(
        "delphi_previous_verdicts",
        serde_json::Value::String("a=fail;b=fail;c=fail".to_string()),
    );
    context.set(
        "delphi_previous_findings",
        serde_json::Value::String("a=old;b=old;c=old".to_string()),
    );
    let g =
        delphi_fan_in_graph_with_convergence("converge_or_majority_pass", 3, "findings_diff<0.1>");
    let node = g.node("aggregate").expect("aggregate node");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let outcome = done(FanInHandler.run(&mut ctx).await.expect("run"));

    assert_eq!(outcome.status, Status::Fail);
    assert!(
        !outcome.context_updates.contains_key("delphi_next_round"),
        "max_rounds should force fallback instead of another round"
    );
}

#[tokio::test]
async fn fan_in_aggregator_any_fail_returns_fail_when_one_blocker() {
    let deps = Deps::new();
    let context = seed_review(
        &deps,
        &[
            ("a", VerdictValue::Pass),
            ("b", VerdictValue::Pass),
            ("c", VerdictValue::Block),
        ],
    )
    .await;
    let g = fan_in_graph("any_fail");
    let node = g.node("aggregate").expect("aggregate node");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let outcome = done(FanInHandler.run(&mut ctx).await.expect("run"));
    assert_eq!(outcome.status, Status::Fail);
}

// ---- codergen -------------------------------------------------------------

fn codergen_graph() -> NodeGraph {
    graph(
        r#"digraph m {
            "implement" [handler="codergen", role="implementer", initial_prompt_includes="$spec_path", pre_tools="mempal_search"];
        }"#,
    )
}

#[tokio::test]
async fn codergen_run_parks_with_agent_outcome_reason_and_assembles_prompt() {
    let g = codergen_graph();
    let node = g.node("implement").expect("implement node");
    let mut context = RunContext::new();
    context.set(
        "spec_path",
        serde_json::Value::String("specs/x.spec.md".to_string()),
    );
    let deps = Deps::new();
    deps.mempal.set_hits(vec![DrawerHit {
        drawer_id: "d1".to_string(),
        body: "prior art note".to_string(),
        score: 0.9,
    }]);
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let step = CodergenHandler.run(&mut ctx).await.expect("run");
    assert!(
        matches!(step, HandlerStep::Park(ParkReason::AgentOutcome { .. })),
        "codergen parks awaiting agent outcome, got {step:?}"
    );
    let spawned = deps.backend.spawned();
    assert_eq!(spawned.len(), 1, "one implementer spawned");
    let prompt = spawned[0]
        .initial_prompt
        .as_deref()
        .expect("initial prompt set");
    assert!(
        prompt.contains("specs/x.spec.md"),
        "prompt carries spec_path: {prompt}"
    );
    assert!(
        prompt.contains("prior art note"),
        "prompt carries mempal hit: {prompt}"
    );
}

#[tokio::test]
async fn codex_prefixed_roles_spawn_codex_cli() {
    let g = graph(
        r#"digraph m {
            "implement" [handler="codergen", role="codex-impl"];
        }"#,
    );
    let node = g.node("implement").expect("implement node");
    let deps = Deps::new();
    let context = RunContext::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let step = CodergenHandler.run(&mut ctx).await.expect("run");
    assert!(matches!(
        step,
        HandlerStep::Park(ParkReason::AgentOutcome { .. })
    ));
    let spawned = deps.backend.spawned();
    assert_eq!(spawned.len(), 1, "one implementer spawned");
    assert_eq!(spawned[0].agent_id.as_str(), "codex-impl");
    assert_eq!(spawned[0].cli, CliKind::Codex);

    let g = graph(
        r#"digraph m {
            "implement" [handler="codergen", role="implementer"];
        }"#,
    );
    let node = g.node("implement").expect("implement node");
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let step = CodergenHandler.run(&mut ctx).await.expect("run");
    assert!(matches!(
        step,
        HandlerStep::Park(ParkReason::AgentOutcome { .. })
    ));
    let spawned = deps.backend.spawned();
    assert_eq!(spawned.len(), 1, "one legacy implementer spawned");
    assert_eq!(spawned[0].agent_id.as_str(), "implementer");
    assert_eq!(spawned[0].cli, CliKind::ClaudeCode);
}

#[tokio::test]
async fn fan_out_prefixed_reviewers_select_matching_cli() {
    let g = graph(
        r#"digraph m {
            "review" [handler="parallel.fan_out", reviewers="claude-sec,codex-perf,gemini-readability"];
        }"#,
    );
    let node = g.node("review").expect("review node");
    let deps = Deps::new();
    let context = RunContext::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let step = FanOutHandler.run(&mut ctx).await.expect("run");
    assert!(matches!(
        step,
        HandlerStep::Park(ParkReason::ReviewVerdicts { .. })
    ));
    let spawned = deps.backend.spawned();
    assert_eq!(spawned.len(), 3, "three reviewers spawned");

    let cli_for = |role: &str| {
        spawned
            .iter()
            .find(|request| request.agent_id.as_str() == role)
            .unwrap_or_else(|| panic!("missing spawned role {role}: {spawned:?}"))
            .cli
    };
    assert_eq!(cli_for("claude-sec"), CliKind::ClaudeCode);
    assert_eq!(cli_for("codex-perf"), CliKind::Codex);
    assert_eq!(cli_for("gemini-readability"), CliKind::ClaudeCode);
}

#[tokio::test]
async fn fan_out_allocates_reviewers_and_uses_selected_reviewer_ids() {
    let g = graph(
        r#"digraph m {
            "review" [handler="parallel.fan_out", reviewers="review,testing"];
        }"#,
    );
    let node = g.node("review").expect("review node");
    let deps = Deps::new();
    let context = RunContext::new();
    let allocator = RecordingAllocator::new(vec![
        routed_allocation("review", "codex-review-1", "sched_res_review"),
        routed_allocation("testing", "codex-testing-1", "sched_res_testing"),
    ]);
    let mut ctx = HandlerCtx::new(
        &deps.run_id,
        &g,
        node,
        &context,
        deps.ports_with_allocator(&allocator),
    );

    let step = FanOutHandler.run(&mut ctx).await.expect("run");
    assert!(matches!(
        step,
        HandlerStep::Park(ParkReason::ReviewVerdicts { .. })
    ));

    let requests = allocator.requests();
    assert_eq!(requests.len(), 2, "one allocation per reviewer");
    assert_eq!(requests[0].role, "review");
    assert_eq!(requests[1].role, "testing");

    let spawned = deps.backend.spawned();
    assert_eq!(spawned.len(), 2, "two allocated reviewers spawned");
    let prompt_for = |agent_id: &str| {
        spawned
            .iter()
            .find(|request| request.agent_id.as_str() == agent_id)
            .unwrap_or_else(|| panic!("missing spawned reviewer {agent_id}: {spawned:?}"))
            .initial_prompt
            .as_deref()
            .expect("review prompt")
            .to_string()
    };
    let review_prompt = prompt_for("codex-review-1");
    assert!(
        review_prompt.contains("agentd_reviewer_id: codex-review-1"),
        "{review_prompt}"
    );
    assert!(
        review_prompt.contains("agentd_scheduler_reservation_id: sched_res_review"),
        "{review_prompt}"
    );
    let testing_prompt = prompt_for("codex-testing-1");
    assert!(
        testing_prompt.contains("agentd_reviewer_id: codex-testing-1"),
        "{testing_prompt}"
    );
    assert!(
        testing_prompt.contains("agentd_scheduler_reservation_id: sched_res_testing"),
        "{testing_prompt}"
    );

    let staged = ctx.staged_updates();
    let allocations = staged["agentd_scheduler_allocations"]["review"]
        .as_array()
        .expect("staged reviewer allocations");
    assert_eq!(allocations.len(), 2);
    assert_eq!(allocations[0]["requestedRole"], "review");
    assert_eq!(allocations[0]["agentId"], "codex-review-1");
    assert_eq!(allocations[1]["requestedRole"], "testing");
    assert_eq!(allocations[1]["agentId"], "codex-testing-1");
}

#[tokio::test]
async fn fan_out_queues_scheduler_reviewer_without_dispatching_backend() {
    let g = graph(
        r#"digraph m {
            "review" [handler="parallel.fan_out", reviewers="review", capability="medium"];
        }"#,
    );
    let node = g.node("review").expect("review node");
    let mut context = RunContext::new();
    context.set(
        "worktree",
        serde_json::Value::String("/tmp/agentd-impl-wt".to_string()),
    );
    let deps = Deps::new();
    let backend = RecordingDispatchBackend::default();
    let allocator = RecordingAllocator::new(vec![queued_allocation("review", "sched_ticket_r1")]);
    let ports = Ports {
        backend: &backend,
        runner: &deps.runner,
        store: &deps.store,
        mempal: &deps.mempal,
        clock: &deps.clock,
        agent_allocator: &allocator,
    };
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, ports);

    let step = FanOutHandler.run(&mut ctx).await.expect("run");
    let review_run_id = match step {
        HandlerStep::Park(ParkReason::ReviewVerdicts {
            review_run_id,
            expected,
            round,
        }) => {
            assert_eq!(expected, 1);
            assert_eq!(round, 1);
            review_run_id
        }
        other => panic!("expected queued ReviewVerdicts park, got {other:?}"),
    };

    assert!(
        backend.spawns().is_empty(),
        "queued reviewer allocation must not plain-spawn"
    );
    assert!(
        backend.dispatches().is_empty(),
        "queued reviewer allocation must not dispatch before scheduler release drains it"
    );
    let requests = allocator.requests();
    assert_eq!(requests.len(), 1, "one reviewer allocation request");
    assert_eq!(requests[0].role, "review");
    assert_eq!(
        requests[0].task["kind"], "workflow_fan_out_reviewer",
        "queued reviewer task carries fan_out wakeup kind"
    );
    assert_eq!(
        requests[0].task["reviewRunId"],
        review_run_id.as_str(),
        "scheduler task points at the exact review run"
    );

    let staged = ctx.staged_updates();
    let allocations = staged["agentd_scheduler_allocations"]["review"]
        .as_array()
        .expect("staged reviewer allocations");
    assert_eq!(allocations.len(), 1);
    assert_eq!(allocations[0]["requestedRole"], "review");
    assert_eq!(allocations[0]["schedulerStatus"], "queued");
    assert_eq!(allocations[0]["schedulerTicket"], "sched_ticket_r1");

    let queued = &staged["agentd_queued_workflow_dispatches"]["review"]["reviewers"]["review"];
    assert_eq!(queued["handler"], "parallel.fan_out");
    assert_eq!(queued["reviewRunId"], review_run_id.as_str());
    assert_eq!(queued["requestedRole"], "review");
    assert_eq!(queued["sourceWorktree"], "/tmp/agentd-impl-wt");
    assert_eq!(queued["round"], 1);
    assert!(
        queued["contextSha"]
            .as_str()
            .is_some_and(|sha| sha.len() == 64),
        "queued wakeup stores context sha: {queued:?}"
    );
    assert!(
        queued["basePrompt"]
            .as_str()
            .expect("base prompt")
            .contains("adversarial review"),
        "queued wakeup stores reviewer prompt base: {queued:?}"
    );
}

#[tokio::test]
async fn codergen_prompt_includes_outcome_submission_context() {
    let g = codergen_graph();
    let node = g.node("implement").expect("implement node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let step = CodergenHandler.run(&mut ctx).await.expect("run");
    let task_run_id = match step {
        HandlerStep::Park(ParkReason::AgentOutcome { task_run_id }) => task_run_id,
        other => panic!("expected AgentOutcome park, got {other:?}"),
    };

    let spawned = deps.backend.spawned();
    assert_eq!(spawned.len(), 1, "one implementer spawned");
    let prompt = spawned[0].initial_prompt.as_deref().expect("agent prompt");
    assert!(prompt.contains("agentd_run_id: r"), "{prompt}");
    assert!(prompt.contains("agentd_node_id: implement"), "{prompt}");
    assert!(prompt.contains("agentd_agent_id: implementer"), "{prompt}");
    assert!(
        prompt.contains(&format!("agentd_task_run_id: {}", task_run_id.as_str())),
        "{prompt}"
    );
    assert!(prompt.contains("submit_outcome"), "{prompt}");
    assert!(prompt.contains("tools/call"), "{prompt}");
}

#[tokio::test]
async fn codergen_prompt_explains_daemon_relative_runtime_paths() {
    let g = graph(
        r#"digraph m {
            "implement" [handler="codergen", role="implementer", initial_prompt_includes="spec_path,plan_path"];
        }"#,
    );
    let node = g.node("implement").expect("implement node");
    let mut context = RunContext::new();
    context.set(
        "spec_path",
        serde_json::Value::String(".agentd/run/frozen.spec.md".to_string()),
    );
    context.set(
        "plan_path",
        serde_json::Value::String(".agentd/run/plan.md".to_string()),
    );
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let step = CodergenHandler.run(&mut ctx).await.expect("run");
    assert!(
        matches!(step, HandlerStep::Park(ParkReason::AgentOutcome { .. })),
        "codergen parks awaiting agent outcome, got {step:?}"
    );
    let spawned = deps.backend.spawned();
    let prompt = spawned[0].initial_prompt.as_deref().expect("agent prompt");
    assert!(prompt.contains("agentd_daemon_cwd:"), "{prompt}");
    assert!(
        prompt.contains(
            "agentd_runtime_path_rule: relative paths in this prompt resolve from agentd_daemon_cwd"
        ),
        "{prompt}"
    );
    assert!(
        prompt.contains("agentd_role_task: read the listed inputs, complete this node's role"),
        "{prompt}"
    );
}

#[tokio::test]
async fn codergen_run_persists_agent_id_for_task_run() {
    let g = codergen_graph();
    let node = g.node("implement").expect("implement node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());

    let step = CodergenHandler.run(&mut ctx).await.expect("run");
    let task_run_id = match step {
        HandlerStep::Park(ParkReason::AgentOutcome { task_run_id }) => task_run_id,
        other => panic!("expected AgentOutcome park, got {other:?}"),
    };

    assert_eq!(
        deps.store
            .task_agent(&task_run_id)
            .as_ref()
            .map(AgentId::as_str),
        Some("implementer"),
        "codergen persists the role as the task-run owner"
    );
}

#[tokio::test]
async fn codergen_allocates_agent_before_spawn_and_task_ownership() {
    let g = graph(
        r#"digraph m {
            "implement" [handler="codergen", role="coding", capability="medium"];
        }"#,
    );
    let node = g.node("implement").expect("implement node");
    let context = RunContext::new();
    let deps = Deps::new();
    let allocator = RecordingAllocator::new(vec![routed_allocation(
        "coding",
        "codex-coding-1",
        "sched_res_1",
    )]);
    let mut ctx = HandlerCtx::new(
        &deps.run_id,
        &g,
        node,
        &context,
        deps.ports_with_allocator(&allocator),
    );

    let step = CodergenHandler.run(&mut ctx).await.expect("run");
    let task_run_id = match step {
        HandlerStep::Park(ParkReason::AgentOutcome { task_run_id }) => task_run_id,
        other => panic!("expected AgentOutcome park, got {other:?}"),
    };

    assert_eq!(
        deps.store
            .task_agent(&task_run_id)
            .as_ref()
            .map(AgentId::as_str),
        Some("codex-coding-1"),
        "codergen persists the allocated agent as task owner"
    );
    assert_eq!(allocator.requests().len(), 1, "one allocation request");
    assert_eq!(allocator.requests()[0].role, "coding");
    assert_eq!(
        allocator.requests()[0].capability.as_deref(),
        Some("medium")
    );

    let spawned = deps.backend.spawned();
    assert_eq!(spawned.len(), 1, "one allocated agent spawned");
    assert_eq!(spawned[0].agent_id.as_str(), "codex-coding-1");
    assert_eq!(spawned[0].cli, CliKind::Codex);
    let prompt = spawned[0].initial_prompt.as_deref().expect("agent prompt");
    assert!(
        prompt.contains("agentd_agent_id: codex-coding-1"),
        "{prompt}"
    );
    assert!(
        prompt.contains("agentd_scheduler_status: routed"),
        "{prompt}"
    );
    assert!(
        prompt.contains("agentd_scheduler_reservation_id: sched_res_1"),
        "{prompt}"
    );

    let staged = ctx.staged_updates();
    let allocations = staged["agentd_scheduler_allocations"]["implement"]
        .as_array()
        .expect("staged allocation array");
    assert_eq!(allocations.len(), 1);
    assert_eq!(allocations[0]["agentId"], "codex-coding-1");
    assert_eq!(allocations[0]["requestedRole"], "coding");
    assert_eq!(allocations[0]["schedulerStatus"], "routed");
    assert_eq!(allocations[0]["schedulerReservationId"], "sched_res_1");
}

#[tokio::test]
async fn codergen_dispatches_allocated_agent_without_calling_plain_spawn() {
    let g = graph(
        r#"digraph m {
            "implement" [handler="codergen", role="coding", capability="medium"];
        }"#,
    );
    let node = g.node("implement").expect("implement node");
    let context = RunContext::new();
    let deps = Deps::new();
    let backend = RecordingDispatchBackend::default();
    let allocation = AgentAllocation {
        runtime: serde_json::json!({
            "tmuxTarget": "agentd-codex-coding-1:0.0",
            "tmux_target": "agentd-codex-coding-1:0.0"
        }),
        ..routed_allocation("coding", "codex-coding-1", "sched_res_1")
    };
    let allocator = RecordingAllocator::new(vec![allocation]);
    let ports = Ports {
        backend: &backend,
        runner: &deps.runner,
        store: &deps.store,
        mempal: &deps.mempal,
        clock: &deps.clock,
        agent_allocator: &allocator,
    };
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, ports);

    let step = CodergenHandler.run(&mut ctx).await.expect("run");
    let task_run_id = match step {
        HandlerStep::Park(ParkReason::AgentOutcome { task_run_id }) => task_run_id,
        other => panic!("expected AgentOutcome park, got {other:?}"),
    };

    assert!(backend.spawns().is_empty(), "plain spawn must not be used");
    let dispatches = backend.dispatches();
    assert_eq!(dispatches.len(), 1, "one allocation-aware dispatch");
    assert_eq!(dispatches[0].0.agent_id.as_str(), "codex-coding-1");
    assert_eq!(dispatches[0].1.agent_id.as_str(), "codex-coding-1");
    assert_eq!(
        dispatches[0].1.runtime["tmuxTarget"],
        "agentd-codex-coding-1:0.0"
    );
    assert_eq!(
        deps.store
            .task_agent(&task_run_id)
            .as_ref()
            .map(AgentId::as_str),
        Some("codex-coding-1")
    );
    let allocations = ctx.staged_updates()["agentd_scheduler_allocations"]["implement"]
        .as_array()
        .expect("staged allocation array");
    assert_eq!(
        allocations[0]["runtime"]["tmuxTarget"],
        "agentd-codex-coding-1:0.0"
    );
}

#[tokio::test]
async fn codergen_queues_scheduler_allocation_without_dispatching_backend() {
    let g = graph(
        r#"digraph m {
            "implement" [handler="codergen", role="coding", capability="medium"];
        }"#,
    );
    let node = g.node("implement").expect("implement node");
    let context = RunContext::new();
    let deps = Deps::new();
    let backend = RecordingDispatchBackend::default();
    let allocator = RecordingAllocator::new(vec![queued_allocation("coding", "sched_ticket_1")]);
    let worktree_allocator = FixedTaskAllocator::new("/tmp/agentd-queued-codergen");
    let ports = Ports {
        backend: &backend,
        runner: &deps.runner,
        store: &deps.store,
        mempal: &deps.mempal,
        clock: &deps.clock,
        agent_allocator: &allocator,
    };
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, ports)
        .with_worktree_allocator(Some(&worktree_allocator));

    let step = CodergenHandler.run(&mut ctx).await.expect("run");
    let task_run_id = match step {
        HandlerStep::Park(ParkReason::AgentOutcome { task_run_id }) => task_run_id,
        other => panic!("expected queued AgentOutcome park, got {other:?}"),
    };

    assert!(
        backend.spawns().is_empty(),
        "queued allocation must not plain-spawn"
    );
    assert!(
        backend.dispatches().is_empty(),
        "queued allocation must not dispatch before scheduler release drains it"
    );
    assert_eq!(
        deps.store.task_agent(&task_run_id),
        None,
        "queued task-run must not be owned by the requested role before drain"
    );
    assert_eq!(
        deps.store.task_worktree(&task_run_id),
        Some(PathBuf::from("/tmp/agentd-queued-codergen")),
        "queued task keeps its allocated worktree for later wakeup"
    );
    let staged = ctx.staged_updates();
    let allocations = staged["agentd_scheduler_allocations"]["implement"]
        .as_array()
        .expect("staged allocation array");
    assert_eq!(allocations.len(), 1);
    assert_eq!(allocations[0]["requestedRole"], "coding");
    assert_eq!(allocations[0]["schedulerStatus"], "queued");
    assert_eq!(allocations[0]["schedulerTicket"], "sched_ticket_1");

    let queued = &staged["agentd_queued_workflow_dispatches"]["implement"];
    assert_eq!(queued["handler"], "codergen");
    assert_eq!(queued["taskRunId"], task_run_id.as_str());
    assert_eq!(queued["worktree"], "/tmp/agentd-queued-codergen");
    assert!(
        queued["basePrompt"]
            .as_str()
            .expect("base prompt")
            .contains("agentd_role_task"),
        "queued wakeup stores the base prompt: {queued:?}"
    );
}

#[tokio::test]
async fn codergen_persists_allocated_worktree_before_spawn() {
    let g = codergen_graph();
    let node = g.node("implement").expect("implement node");
    let context = RunContext::new();
    let deps = FailingBackendDeps::new();
    let allocator = FixedTaskAllocator::new("/tmp/agentd-task-before-spawn");
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports())
        .with_worktree_allocator(Some(&allocator));

    let err = CodergenHandler
        .run(&mut ctx)
        .await
        .expect_err("backend spawn fails");
    assert!(
        format!("{err:?}").contains("injected spawn failure"),
        "backend error should be surfaced: {err:?}"
    );

    let task_run_id = deps
        .store
        .task_run_ids()
        .into_iter()
        .next()
        .expect("task run inserted before spawn");
    assert_eq!(
        deps.store.task_worktree(&task_run_id),
        Some(PathBuf::from("/tmp/agentd-task-before-spawn")),
        "allocated worktree must be persisted before backend spawn so agent-side MCP boot-GC preserves it"
    );
}

#[tokio::test]
async fn codergen_resume_returns_agent_reported_outcome() {
    let g = codergen_graph();
    let node = g.node("implement").expect("implement node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let task_run_id = match CodergenHandler.run(&mut ctx).await.expect("run") {
        HandlerStep::Park(ParkReason::AgentOutcome { task_run_id }) => task_run_id,
        other => panic!("expected park, got {other:?}"),
    };
    let outcome = done(
        CodergenHandler
            .resume(
                &mut ctx,
                EngineEvent::AgentOutcomeSubmitted {
                    task_run_id,
                    outcome: Outcome::fail(),
                },
            )
            .await
            .expect("resume"),
    );
    assert_eq!(outcome.status, Status::Fail);
}

#[tokio::test]
async fn fan_out_resume_ignores_duplicate_reviewer_verdict() {
    let g = fan_out_graph(); // three reviewers
    let node = g.node("review").expect("review node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let review_run_id = match FanOutHandler.run(&mut ctx).await.expect("run") {
        HandlerStep::Park(ParkReason::ReviewVerdicts { review_run_id, .. }) => review_run_id,
        other => panic!("expected park, got {other:?}"),
    };
    let submit = |who: &str| EngineEvent::ReviewVerdictSubmitted {
        review_run_id: review_run_id.clone(),
        reviewer_id: AgentId::parsed(who),
        verdict: VerdictValue::Pass,
        findings: String::new(),
    };
    // First reviewer votes, then the SAME reviewer's verdict is replayed: the
    // duplicate must not advance toward quorum.
    let step = FanOutHandler
        .resume(&mut ctx, submit("claude-sec"))
        .await
        .expect("v1");
    assert!(matches!(step, HandlerStep::Park(_)));
    let step = FanOutHandler
        .resume(&mut ctx, submit("claude-sec"))
        .await
        .expect("v1 replay");
    assert!(
        matches!(step, HandlerStep::Park(_)),
        "a duplicate reviewer verdict must not count toward quorum"
    );
    // Two more DISTINCT reviewers complete the three-way review.
    let step = FanOutHandler
        .resume(&mut ctx, submit("codex-perf"))
        .await
        .expect("v2");
    assert!(matches!(step, HandlerStep::Park(_)));
    let step = FanOutHandler
        .resume(&mut ctx, submit("gemini-readability"))
        .await
        .expect("v3");
    assert!(
        matches!(step, HandlerStep::Done(_)),
        "three distinct reviewers complete the review"
    );
}

#[tokio::test]
async fn codergen_resume_closes_task_run_so_replay_is_noop() {
    let g = codergen_graph();
    let node = g.node("implement").expect("implement node");
    let context = RunContext::new();
    let deps = Deps::new();
    let mut ctx = HandlerCtx::new(&deps.run_id, &g, node, &context, deps.ports());
    let task_run_id = match CodergenHandler.run(&mut ctx).await.expect("run") {
        HandlerStep::Park(ParkReason::AgentOutcome { task_run_id }) => task_run_id,
        other => panic!("expected park, got {other:?}"),
    };
    assert!(
        deps.store
            .lookup_park_by_task_run(&task_run_id)
            .await
            .expect("lookup")
            .is_some(),
        "task run parks before its outcome arrives"
    );
    CodergenHandler
        .resume(
            &mut ctx,
            EngineEvent::AgentOutcomeSubmitted {
                task_run_id: task_run_id.clone(),
                outcome: Outcome::success(),
            },
        )
        .await
        .expect("resume");
    assert_eq!(
        deps.store
            .lookup_park_by_task_run(&task_run_id)
            .await
            .expect("lookup after resume"),
        None,
        "resume closes the task run so a replayed event is a no-op"
    );
}
