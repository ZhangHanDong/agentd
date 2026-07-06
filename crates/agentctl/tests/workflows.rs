//! P0.8 workflow-authoring: the standalone Path-B workflows conform to the
//! frozen DOT grammar and walk on the real `Engine`. Test names match
//! `specs/workflow/p80-draft-dot.spec.md` (and p81 for execute.dot).
//!
//! The walk-tests (added with the walk-test tasks) construct the real
//! `agentd_core::Engine` over the `test-support` fakes — NOT `FakeRunHost`,
//! which scripts `RunProgress` and exercises only the MCP tool layer.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentd_core::CoreError;
use agentd_core::dot::parser;
use agentd_core::engine::{Engine, EngineEvent, ParkReason, RunProgress};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{HandlerRegistry, Ports};
use agentd_core::ports::{CommandOutput, WorktreeAllocator};
use agentd_core::test_support::{
    FakeBackend, FixedClock, InMemoryStore, MempalStub, RecordingCommandRunner,
};
use agentd_core::types::{AgentId, Outcome, RunId, TaskRunId, VerdictValue};

/// Repo-root `workflows/` dir, resolved from the agentctl crate manifest.
fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
}

/// Parse + validate a workflow file, returning the built graph.
fn load(name: &str) -> NodeGraph {
    let path = workflows_dir().join(name);
    let src =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let ast = parser::parse(&src).unwrap_or_else(|e| panic!("parse {}: {e:?}", path.display()));
    NodeGraph::from_ast(&ast).unwrap_or_else(|e| panic!("validate {}: {e:?}", path.display()))
}

// ─── walk harness: the REAL Engine over in-memory fakes (mirrors agentd-core's
// own tests/engine_execute.rs — NOT FakeRunHost). ───────────────────────────

struct Harness {
    run_id: RunId,
    registry: HandlerRegistry,
    backend: FakeBackend,
    runner: RecordingCommandRunner,
    store: InMemoryStore,
    mempal: MempalStub,
    clock: FixedClock,
}

impl Harness {
    fn new() -> Self {
        Self {
            run_id: RunId::from_string("run-1"),
            registry: HandlerRegistry::with_builtins(),
            backend: FakeBackend::new(),
            runner: RecordingCommandRunner::new(),
            store: InMemoryStore::new(),
            mempal: MempalStub::new(),
            clock: FixedClock::new(0),
        }
    }

    fn engine<'a>(&'a self, graph: &'a NodeGraph) -> Engine<'a> {
        Engine::new(
            graph,
            &self.registry,
            Ports {
                backend: &self.backend,
                runner: &self.runner,
                store: &self.store,
                mempal: &self.mempal,
                clock: &self.clock,
            },
            "sha-test",
        )
    }

    fn engine_with_allocator<'a>(
        &'a self,
        graph: &'a NodeGraph,
        allocator: &'a dyn WorktreeAllocator,
    ) -> Engine<'a> {
        self.engine(graph).with_worktree_allocator(Some(allocator))
    }

    /// Queue `n` successful (exit-0) tool outputs for the run's `tool` nodes.
    fn push_ok(&self, n: usize) {
        for _ in 0..n {
            self.runner.push_output(Ok(CommandOutput {
                stdout: String::new(),
                stderr: String::new(),
                status: 0,
            }));
        }
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

#[derive(Debug, Clone)]
struct SnapshotAllocator {
    implementer_path: PathBuf,
    snapshots: Arc<Mutex<Vec<(String, PathBuf, PathBuf)>>>,
}

impl SnapshotAllocator {
    fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            implementer_path: path.into(),
            snapshots: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn snapshots(&self) -> Vec<(String, PathBuf, PathBuf)> {
        self.snapshots.lock().expect("snapshot lock").clone()
    }
}

#[async_trait::async_trait]
impl WorktreeAllocator for SnapshotAllocator {
    async fn allocate(&self, key: &str) -> Result<PathBuf, CoreError> {
        assert!(
            key.starts_with("tr_") || key.starts_with("tr"),
            "implementer allocation should be keyed by task_run_id, got {key}"
        );
        Ok(self.implementer_path.clone())
    }

    async fn allocate_snapshot(&self, key: &str, source: &Path) -> Result<PathBuf, CoreError> {
        assert_eq!(
            source, self.implementer_path,
            "reviewer snapshots must copy from the implementer worktree"
        );
        let path = PathBuf::from("/tmp").join(format!("review-{key}"));
        self.snapshots.lock().expect("snapshot lock").push((
            key.to_string(),
            source.to_path_buf(),
            path.clone(),
        ));
        Ok(path)
    }

    async fn release(&self, _key: &str, _path: &std::path::Path) -> Result<(), CoreError> {
        Ok(())
    }
}

fn park_reason(progress: &RunProgress) -> &ParkReason {
    match progress {
        RunProgress::Parked { reason, .. } => reason,
        other => panic!("expected Parked, got {other:?}"),
    }
}

// ─── execute.dot (p81) ──────────────────────────────────────────────────────

#[test]
fn execute_dot_validates() {
    let g = load("execute.dot");
    assert!(!g.nodes.is_empty(), "execute.dot has nodes");
}

#[test]
fn execute_dot_single_start_single_terminal() {
    let g = load("execute.dot");
    assert_eq!(g.starts().len(), 1, "exactly one start (Mdiamond)");
    assert_eq!(g.terminals().len(), 1, "exactly one terminal (Msquare)");
}

#[test]
fn execute_dot_has_goal_gate_unmet_recovery_edge() {
    let g = load("execute.dot");
    let recovery: Vec<_> = g
        .edges
        .iter()
        .filter(|e| e.attr("label") == Some("goal_gate_unmet"))
        .collect();
    assert_eq!(
        recovery.len(),
        1,
        "exactly one goal_gate_unmet recovery edge"
    );
    let target = recovery[0].to.as_str();
    let terminal_ids: Vec<&str> = g.terminals().iter().map(|n| n.id.as_str()).collect();
    assert!(
        !terminal_ids.contains(&target),
        "recovery edge must route to a non-terminal, got '{target}'"
    );
}

#[test]
fn execute_dot_verify_lifecycle_uses_worktree_variable() {
    let g = load("execute.dot");
    let verify = g
        .nodes
        .iter()
        .find(|n| n.id.as_str() == "verify_lifecycle")
        .expect("verify_lifecycle node exists");
    let cmd = verify.attr("cmd").expect("verify_lifecycle has cmd");
    assert!(
        cmd.contains("--code ${worktree}"),
        "verify_lifecycle must explicitly check the allocated worktree, got {cmd:?}"
    );
}

#[test]
fn execute_dot_declares_spec_and_plan_context_bridge() {
    let g = load("execute.dot");
    let pull = g
        .node("pull_frozen_spec")
        .expect("pull_frozen_spec node exists");
    assert_eq!(
        pull.attr("context_updates"),
        Some("spec_path=.agentd/run/frozen.spec.md"),
        "pull_frozen_spec stages the frozen spec path"
    );

    let plan = g.node("draft_plan").expect("draft_plan node exists");
    assert_eq!(
        plan.attr("cmd"),
        Some("bash scripts/agentd_write_plan.sh .agentd/run/frozen.spec.md .agentd/run/plan.md"),
        "draft_plan writes a concrete local plan file"
    );
    assert_eq!(
        plan.attr("context_updates"),
        Some("plan_path=.agentd/run/plan.md"),
        "draft_plan stages the generated plan path"
    );

    let implement = g.node("implement").expect("implement node exists");
    assert_eq!(
        implement.attr("initial_prompt_includes"),
        Some("spec_path,plan_path"),
        "implementer prompt includes both staged path keys"
    );
}

#[test]
fn execute_dot_publishes_worktree_before_pr() {
    let g = load("execute.dot");
    let publish = g
        .node("publish_branch")
        .expect("publish_branch node exists");
    assert_eq!(
        publish.attr("cmd"),
        Some("bash scripts/agentd_publish_worktree.sh ${worktree} ${task_run_id}")
    );

    let open_pr = g.node("open_pr").expect("open_pr node exists");
    assert_eq!(
        open_pr.attr("cmd"),
        Some("gh pr create --fill --head agentd/${task_run_id}")
    );

    for (from, to) in [
        ("aggregate", "publish_branch"),
        ("publish_branch", "open_pr"),
        ("open_pr", "report_acceptance"),
    ] {
        let edge = g
            .edges
            .iter()
            .find(|e| e.from == from && e.to == to)
            .unwrap_or_else(|| panic!("edge {from} -> {to} exists"));
        assert_eq!(
            edge.attr("condition"),
            Some("outcome=success"),
            "edge {from} -> {to} must not fall through on failure"
        );
    }
}

#[test]
fn execute_dot_rejects_unpaired_double_fan_out_variant() {
    let src = r#"digraph execute {
        "start" [shape=Mdiamond];
        "fan_a" [handler="parallel.fan_out", reviewers="a"];
        "fan_b" [handler="parallel.fan_out", reviewers="b"];
        "agg"   [handler="parallel.fan_in", aggregator="majority_pass"];
        "done"  [shape=Msquare];
        "start" -> "fan_a";
        "start" -> "fan_b";
        "fan_a" -> "agg";
        "fan_b" -> "agg";
        "agg"   -> "done";
    }"#;
    let ast = parser::parse(src).expect("parse");
    let err = NodeGraph::from_ast(&ast)
        .expect_err("two unpaired fan_outs into one fan_in must be rejected");
    assert!(
        format!("{err:?}").contains("fan_out"),
        "violation should report the unpaired fan_out, got {err:?}"
    );
}

#[tokio::test]
async fn execute_dot_implement_prompt_receives_spec_and_plan_paths() {
    let g = load("execute.dot");
    let h = Harness::new();
    let allocator = StaticAllocator::new("/tmp/agentd-task-wt");
    let engine = h.engine_with_allocator(&g, &allocator);

    let parked = engine.execute(&h.run_id).await.expect("execute");

    assert!(
        matches!(park_reason(&parked), ParkReason::AgentOutcome { .. }),
        "execute.dot should park at implement, got {parked:?}"
    );
    let spawned = h.backend.spawned();
    let prompt = spawned
        .iter()
        .find(|req| req.agent_id.as_str() == "implementer")
        .and_then(|req| req.initial_prompt.as_deref())
        .expect("implementer prompt");
    assert!(
        prompt.contains("spec_path: .agentd/run/frozen.spec.md"),
        "implementer prompt must include the staged spec path: {prompt}"
    );
    assert!(
        prompt.contains("plan_path: .agentd/run/plan.md"),
        "implementer prompt must include the staged plan path: {prompt}"
    );
}

#[tokio::test]
async fn execute_dot_walks_to_done() {
    let g = load("execute.dot");
    let h = Harness::new();
    let allocator = StaticAllocator::new("/tmp/agentd-task-wt");
    let engine = h.engine_with_allocator(&g, &allocator);

    // start -> pull_frozen_spec, draft_plan (tools) -> implement (codergen) parks.
    let parked = engine.execute(&h.run_id).await.expect("execute");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park at implement, got {other:?}"),
    };
    let task_branch = format!("agentd/{}", task_run_id.as_str());
    let task_run_id_arg = task_run_id.as_str().to_string();

    // implement success -> verify_lifecycle (tool) -> review (fan_out) parks for 3 verdicts.
    let parked = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id: task_run_id.clone(),
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver implement outcome");
    let review_run_id = match park_reason(&parked) {
        ParkReason::ReviewVerdicts {
            review_run_id,
            expected,
            ..
        } => {
            assert_eq!(*expected, 3, "three reviewers");
            review_run_id.clone()
        }
        other => panic!("expected ReviewVerdicts park at review, got {other:?}"),
    };

    // Three pass verdicts -> aggregate (majority_pass) -> publish_branch ->
    // open_pr -> report_acceptance; both goal_gates (verify_lifecycle,
    // aggregate) met -> done.
    let mut last = None;
    for reviewer in ["claude-sec", "codex-perf", "gemini-readability"] {
        last = Some(
            engine
                .deliver_event(EngineEvent::ReviewVerdictSubmitted {
                    review_run_id: review_run_id.clone(),
                    reviewer_id: AgentId::parsed(reviewer),
                    verdict: VerdictValue::Pass,
                    findings: String::new(),
                })
                .await
                .expect("deliver verdict"),
        );
    }
    assert_eq!(
        last.expect("a final progress"),
        RunProgress::Finished {
            run_id: h.run_id.clone()
        }
    );

    let calls = h.runner.calls();
    let publish = calls
        .iter()
        .find(|c| {
            c.program == "bash"
                && c.args
                    .first()
                    .is_some_and(|a| a == "scripts/agentd_publish_worktree.sh")
        })
        .expect("publish_branch recorded a script call");
    assert_eq!(
        publish.args,
        vec![
            "scripts/agentd_publish_worktree.sh".to_string(),
            "/tmp/agentd-task-wt".to_string(),
            task_run_id_arg,
        ]
    );

    let gh = calls
        .iter()
        .find(|c| c.program == "gh")
        .expect("open_pr recorded a `gh` call");
    assert_eq!(
        gh.args,
        vec![
            "pr".to_string(),
            "create".to_string(),
            "--fill".to_string(),
            "--head".to_string(),
            task_branch,
        ]
    );
}

#[tokio::test]
async fn execute_dot_publish_failure_stops_before_open_pr() {
    let g = load("execute.dot");
    let h = Harness::new();
    let allocator = StaticAllocator::new("/tmp/agentd-task-wt");
    let engine = h.engine_with_allocator(&g, &allocator);
    h.push_ok(3);
    h.runner.push_output(Ok(CommandOutput {
        stdout: String::new(),
        stderr: "push failed".to_string(),
        status: 1,
    }));

    let parked = engine.execute(&h.run_id).await.expect("execute");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park at implement, got {other:?}"),
    };
    let parked = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id,
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver implement outcome");
    let review_run_id = match park_reason(&parked) {
        ParkReason::ReviewVerdicts { review_run_id, .. } => review_run_id.clone(),
        other => panic!("expected ReviewVerdicts park at review, got {other:?}"),
    };

    let mut last = None;
    for reviewer in ["claude-sec", "codex-perf", "gemini-readability"] {
        last = Some(
            engine
                .deliver_event(EngineEvent::ReviewVerdictSubmitted {
                    review_run_id: review_run_id.clone(),
                    reviewer_id: AgentId::parsed(reviewer),
                    verdict: VerdictValue::Pass,
                    findings: String::new(),
                })
                .await
                .expect("deliver verdict"),
        );
    }
    let progress = last.expect("a final progress");
    assert!(
        matches!(&progress, RunProgress::Failed { reason, .. } if reason.contains("publish_branch")),
        "publish failure must fail the run before open_pr, got {progress:?}"
    );

    let calls = h.runner.calls();
    assert!(
        calls.iter().any(|c| c.program == "bash"
            && c.args
                .first()
                .is_some_and(|a| a == "scripts/agentd_publish_worktree.sh")),
        "publish_branch should have run before failing"
    );
    assert!(
        calls
            .iter()
            .all(|c| !(c.program == "gh" && c.args.first().is_some_and(|a| a == "pr"))),
        "open_pr must not run after publish_branch failure: {calls:?}"
    );
}

#[tokio::test]
async fn execute_dot_open_pr_failure_stops_before_report_acceptance() {
    let g = load("execute.dot");
    let h = Harness::new();
    let allocator = StaticAllocator::new("/tmp/agentd-task-wt");
    let engine = h.engine_with_allocator(&g, &allocator);
    h.push_ok(4);
    h.runner.push_output(Ok(CommandOutput {
        stdout: String::new(),
        stderr: "pr create failed".to_string(),
        status: 1,
    }));

    let parked = engine.execute(&h.run_id).await.expect("execute");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park at implement, got {other:?}"),
    };
    let parked = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id,
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver implement outcome");
    let review_run_id = match park_reason(&parked) {
        ParkReason::ReviewVerdicts { review_run_id, .. } => review_run_id.clone(),
        other => panic!("expected ReviewVerdicts park at review, got {other:?}"),
    };

    let mut last = None;
    for reviewer in ["claude-sec", "codex-perf", "gemini-readability"] {
        last = Some(
            engine
                .deliver_event(EngineEvent::ReviewVerdictSubmitted {
                    review_run_id: review_run_id.clone(),
                    reviewer_id: AgentId::parsed(reviewer),
                    verdict: VerdictValue::Pass,
                    findings: String::new(),
                })
                .await
                .expect("deliver verdict"),
        );
    }
    let progress = last.expect("a final progress");
    assert!(
        matches!(&progress, RunProgress::Failed { reason, .. } if reason.contains("open_pr")),
        "open_pr failure must fail the run before report_acceptance, got {progress:?}"
    );

    let calls = h.runner.calls();
    assert!(
        calls
            .iter()
            .any(|c| c.program == "gh" && c.args.first().is_some_and(|a| a == "pr")),
        "open_pr should have run before failing"
    );
    assert!(
        calls.iter().all(|c| {
            !(c.program == "cat" && c.args.first().is_some_and(|a| a == ".agentd/run/report.md"))
        }),
        "report_acceptance must not run after open_pr failure: {calls:?}"
    );
}

#[tokio::test]
async fn execute_dot_goal_gate_unmet_routes_to_recovery_not_stuck() {
    let g = load("execute.dot");
    let h = Harness::new();
    let allocator = StaticAllocator::new("/tmp/agentd-task-wt");
    let engine = h.engine_with_allocator(&g, &allocator);
    // pull_frozen_spec(ok), draft_plan(ok), then verify_lifecycle FAILS (exit 1) — so
    // its goal_gate is permanently unmet. open_pr/report_acceptance exhaust -> success.
    h.push_ok(2);
    h.runner.push_output(Ok(CommandOutput {
        stdout: String::new(),
        stderr: "lifecycle failed".to_string(),
        status: 1,
    }));

    // execute -> implement parks.
    let parked = engine.execute(&h.run_id).await.expect("execute");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park at implement, got {other:?}"),
    };
    // implement success -> verify_lifecycle FAILS -> (unconditional) review parks.
    let parked = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id,
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver implement outcome");
    let review_run_id = match park_reason(&parked) {
        ParkReason::ReviewVerdicts { review_run_id, .. } => review_run_id.clone(),
        other => panic!("expected ReviewVerdicts park at review, got {other:?}"),
    };

    // 3 pass verdicts -> aggregate -> open_pr -> report_acceptance -> done transition:
    // the GLOBAL goal_gate is unmet (verify_lifecycle Fail), so the engine discards the
    // terminal transition, synthesizes goal_gate_unmet, and re-selects the recovery edge
    // report_acceptance -> implement — which re-parks (codergen). NOT Stuck/Failed.
    let mut last = None;
    for reviewer in ["claude-sec", "codex-perf", "gemini-readability"] {
        last = Some(
            engine
                .deliver_event(EngineEvent::ReviewVerdictSubmitted {
                    review_run_id: review_run_id.clone(),
                    reviewer_id: AgentId::parsed(reviewer),
                    verdict: VerdictValue::Pass,
                    findings: String::new(),
                })
                .await
                .expect("deliver verdict"),
        );
    }
    let final_progress = last.expect("a final progress");
    assert!(
        matches!(
            &final_progress,
            RunProgress::Parked {
                reason: ParkReason::AgentOutcome { .. },
                ..
            }
        ),
        "an unmet goal_gate must route to the recovery edge (re-park at implement), \
         not Stuck/Failed/Finished — got {final_progress:?}"
    );
}

#[tokio::test]
async fn execute_dot_reviewers_receive_independent_worktrees() {
    let g = load("execute.dot");
    let h = Harness::new();
    let allocator = SnapshotAllocator::new("/tmp/agentd-task-wt");
    let engine = h.engine_with_allocator(&g, &allocator);

    let parked = engine.execute(&h.run_id).await.expect("execute");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park at implement, got {other:?}"),
    };
    let parked = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id,
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver implement outcome");
    assert!(
        matches!(park_reason(&parked), ParkReason::ReviewVerdicts { .. }),
        "implement success should park at review, got {parked:?}"
    );

    let spawned = h.backend.spawned();
    assert_eq!(spawned.len(), 4, "implementer + 3 reviewers spawned");
    let implementer_worktree = PathBuf::from("/tmp/agentd-task-wt");
    assert_eq!(
        spawned
            .iter()
            .find(|req| req.agent_id.as_str() == "implementer")
            .expect("implementer spawned")
            .worktree,
        implementer_worktree,
        "implementer still runs in the allocated task worktree"
    );

    let verify = h
        .runner
        .calls()
        .into_iter()
        .find(|c| c.program == "agent-spec" && c.args.iter().any(|a| a == "lifecycle"))
        .expect("verify_lifecycle tool ran");
    assert!(
        verify
            .args
            .contains(&implementer_worktree.to_string_lossy().into_owned()),
        "tool context must keep ${{worktree}} as the implementer worktree: {:?}",
        verify.args
    );

    let reviewer_worktrees: Vec<_> = spawned
        .iter()
        .filter(|req| req.agent_id.as_str() != "implementer")
        .map(|req| req.worktree.clone())
        .collect();
    assert_eq!(reviewer_worktrees.len(), 3, "three reviewer spawns");
    assert!(
        reviewer_worktrees
            .iter()
            .all(|path| path != &implementer_worktree),
        "reviewers should not share the implementer worktree: {reviewer_worktrees:?}"
    );
    let unique: std::collections::HashSet<_> = reviewer_worktrees.iter().collect();
    assert_eq!(
        unique.len(),
        reviewer_worktrees.len(),
        "each reviewer gets its own snapshot worktree: {reviewer_worktrees:?}"
    );

    let snapshots = allocator.snapshots();
    assert_eq!(snapshots.len(), 3, "one snapshot allocation per reviewer");
    assert!(
        snapshots.iter().all(
            |(key, source, _)| key.starts_with("review-rr_") && source == &implementer_worktree
        ),
        "snapshot keys are review-run scoped and all copy from implementer: {snapshots:?}"
    );
}

#[tokio::test]
async fn draft_dot_parks_at_propose_spec_then_finishes() {
    let g = load("draft.dot");
    let h = Harness::new();
    let engine = h.engine(&g);
    // Three tool nodes run across the walk: fetch_issue_context, lint_spec, push_draft.
    h.push_ok(3);

    // start -> fetch_issue_context (tool) -> propose_spec (codergen) parks for the agent.
    let parked = engine.execute(&h.run_id).await.expect("execute");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park at propose_spec, got {other:?}"),
    };

    // The spec-writer's success -> lint_spec -> push_draft -> done.
    let progress = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id,
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver agent outcome");
    assert_eq!(
        progress,
        RunProgress::Finished {
            run_id: h.run_id.clone()
        }
    );
}

#[test]
fn draft_dot_validates() {
    // `load` panics on any parse/validation failure; reaching here is success.
    let g = load("draft.dot");
    assert!(!g.nodes.is_empty(), "draft.dot has nodes");
}

#[test]
fn draft_dot_single_start_single_terminal() {
    let g = load("draft.dot");
    assert_eq!(g.starts().len(), 1, "exactly one start (Mdiamond)");
    assert_eq!(g.terminals().len(), 1, "exactly one terminal (Msquare)");
}

#[test]
fn draft_dot_rejects_unknown_handler_variant() {
    let src = r#"digraph draft {
        "start"        [shape=Mdiamond];
        "propose_spec" [handler="stack.manager_loop"];
        "done"         [shape=Msquare];
        "start"        -> "propose_spec";
        "propose_spec" -> "done";
    }"#;
    let ast = parser::parse(src).expect("parse");
    let err = NodeGraph::from_ast(&ast).expect_err("unknown handler must be rejected");
    assert!(
        format!("{err:?}").contains("unknown handler"),
        "violation should name the unknown handler, got {err:?}"
    );
}

#[test]
fn draft_dot_rejects_missing_terminal_variant() {
    let src = r#"digraph draft {
        "start"        [shape=Mdiamond];
        "propose_spec" [handler="codergen"];
        "start"        -> "propose_spec";
    }"#;
    let ast = parser::parse(src).expect("parse");
    let err = NodeGraph::from_ast(&ast).expect_err("a graph with no terminal must be rejected");
    assert!(
        format!("{err:?}").contains("terminal"),
        "violation should report the missing terminal, got {err:?}"
    );
}

// ─── §7.3 P1.7: spike / docs-only / bugfix-rapid / refactor-only ─────────────

// Drive a SINGLE-agent-park workflow (one codergen park, then tool nodes) to
// Finished: execute to the park, submit the agent's success, expect done. Tool
// nodes default to exit-0 on an empty runner queue, so any goal_gate they carry
// is met — covers spike, docs-only, and bugfix-rapid.
async fn walk_single_park_to_done(file: &str) {
    let g = load(file);
    let h = Harness::new();
    let engine = h.engine(&g);
    let parked = engine.execute(&h.run_id).await.expect("execute");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park, got {other:?}"),
    };
    let progress = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id,
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver agent outcome");
    assert_eq!(
        progress,
        RunProgress::Finished {
            run_id: h.run_id.clone()
        },
        "{file} should reach Finished"
    );
}

// Assert a workflow has exactly one goal_gate_unmet recovery edge routing to a
// non-terminal (the global-gate discipline; mirrors execute.dot's check).
fn assert_single_recovery_to_nonterminal(file: &str) {
    let g = load(file);
    let recovery: Vec<_> = g
        .edges
        .iter()
        .filter(|e| e.attr("label") == Some("goal_gate_unmet"))
        .collect();
    assert_eq!(
        recovery.len(),
        1,
        "{file}: one goal_gate_unmet recovery edge"
    );
    let target = recovery[0].to.as_str();
    let terminals: Vec<&str> = g.terminals().iter().map(|n| n.id.as_str()).collect();
    assert!(
        !terminals.contains(&target),
        "{file}: recovery edge must route to a non-terminal, got '{target}'"
    );
}

fn assert_publish_branch_before_pr(file: &str, report_node: &str) {
    let g = load(file);
    let publish = g
        .node("publish_branch")
        .unwrap_or_else(|| panic!("{file}: publish_branch node exists"));
    assert_eq!(
        publish.attr("cmd"),
        Some("bash scripts/agentd_publish_worktree.sh ${worktree} ${task_run_id}"),
        "{file}: publish_branch must use the P100 helper"
    );

    let open_pr = g
        .node("open_pr")
        .unwrap_or_else(|| panic!("{file}: open_pr node exists"));
    assert_eq!(
        open_pr.attr("cmd"),
        Some("gh pr create --fill --head agentd/${task_run_id}"),
        "{file}: open_pr must target the task branch explicitly"
    );

    for (from, to) in [("publish_branch", "open_pr"), ("open_pr", report_node)] {
        let edge = g
            .edges
            .iter()
            .find(|e| e.from == from && e.to == to)
            .unwrap_or_else(|| panic!("{file}: edge {from} -> {to} exists"));
        assert_eq!(
            edge.attr("condition"),
            Some("outcome=success"),
            "{file}: edge {from} -> {to} must not fall through on failure"
        );
    }
}

fn assert_verify_lifecycle_uses_worktree(file: &str) {
    let g = load(file);
    let verify = g
        .node("verify_lifecycle")
        .unwrap_or_else(|| panic!("{file}: verify_lifecycle node exists"));
    let cmd = verify
        .attr("cmd")
        .unwrap_or_else(|| panic!("{file}: verify_lifecycle has cmd"));
    assert!(
        cmd.contains("--code ${worktree}"),
        "{file}: verify_lifecycle must check the allocated worktree, got {cmd:?}"
    );
}

fn assert_recorded_task_branch_publication(
    file: &str,
    task_run_id: &TaskRunId,
    calls: &[agentd_core::test_support::RecordedCall],
) {
    let task_branch = format!("agentd/{}", task_run_id.as_str());
    let publish_idx = calls
        .iter()
        .position(|c| {
            c.program == "bash"
                && c.args
                    .first()
                    .is_some_and(|a| a == "scripts/agentd_publish_worktree.sh")
        })
        .unwrap_or_else(|| panic!("{file}: publish_branch recorded a script call"));
    assert_eq!(
        calls[publish_idx].args,
        vec![
            "scripts/agentd_publish_worktree.sh".to_string(),
            "/tmp/agentd-task-wt".to_string(),
            task_run_id.as_str().to_string(),
        ],
        "{file}: publish_branch receives worktree and task_run_id"
    );

    let gh_idx = calls
        .iter()
        .position(|c| c.program == "gh" && c.args.first().is_some_and(|a| a == "pr"))
        .unwrap_or_else(|| panic!("{file}: open_pr recorded a gh call"));
    assert!(
        publish_idx < gh_idx,
        "{file}: publish_branch must run before open_pr: {calls:?}"
    );
    assert_eq!(
        calls[gh_idx].args,
        vec![
            "pr".to_string(),
            "create".to_string(),
            "--fill".to_string(),
            "--head".to_string(),
            task_branch,
        ],
        "{file}: open_pr targets the task branch"
    );
}

async fn walk_single_park_pr_workflow_to_done(
    file: &str,
) -> (TaskRunId, Vec<agentd_core::test_support::RecordedCall>) {
    let g = load(file);
    let h = Harness::new();
    let allocator = StaticAllocator::new("/tmp/agentd-task-wt");
    let engine = h.engine_with_allocator(&g, &allocator);
    let parked = engine.execute(&h.run_id).await.expect("execute");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park, got {other:?}"),
    };
    let progress = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id: task_run_id.clone(),
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver agent outcome");
    assert_eq!(
        progress,
        RunProgress::Finished {
            run_id: h.run_id.clone()
        },
        "{file} should reach Finished"
    );
    (task_run_id, h.runner.calls())
}

#[test]
fn spike_dot_validates() {
    assert!(!load("spike.dot").nodes.is_empty(), "spike.dot has nodes");
}

#[tokio::test]
async fn spike_dot_walks_to_done() {
    walk_single_park_to_done("spike.dot").await;
}

#[test]
fn docs_only_dot_validates() {
    assert!(
        !load("docs-only.dot").nodes.is_empty(),
        "docs-only.dot has nodes"
    );
}

#[tokio::test]
async fn docs_only_dot_walks_to_done() {
    let (task_run_id, calls) = walk_single_park_pr_workflow_to_done("docs-only.dot").await;
    assert_recorded_task_branch_publication("docs-only.dot", &task_run_id, &calls);
}

#[test]
fn docs_only_dot_publishes_worktree_before_pr() {
    assert_publish_branch_before_pr("docs-only.dot", "report");
    let g = load("docs-only.dot");
    let edge = g
        .edges
        .iter()
        .find(|e| e.from == "write_docs" && e.to == "publish_branch")
        .expect("docs-only.dot: write_docs -> publish_branch edge exists");
    assert_eq!(
        edge.attr("condition"),
        Some("outcome=success"),
        "docs-only.dot has no recovery path, so write_docs failure must not publish"
    );
}

#[tokio::test]
async fn docs_only_dot_publish_failure_stops_before_open_pr() {
    let g = load("docs-only.dot");
    let h = Harness::new();
    let allocator = StaticAllocator::new("/tmp/agentd-task-wt");
    let engine = h.engine_with_allocator(&g, &allocator);
    h.runner.push_output(Ok(CommandOutput {
        stdout: String::new(),
        stderr: "push failed".to_string(),
        status: 1,
    }));

    let parked = engine.execute(&h.run_id).await.expect("execute");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park, got {other:?}"),
    };
    let progress = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id,
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver agent outcome");

    assert!(
        matches!(&progress, RunProgress::Failed { reason, .. } if reason.contains("publish_branch")),
        "publish failure must fail docs-only before open_pr, got {progress:?}"
    );
    let calls = h.runner.calls();
    assert!(
        calls
            .iter()
            .all(|c| !(c.program == "gh" && c.args.first().is_some_and(|a| a == "pr"))),
        "open_pr must not run after publish_branch failure: {calls:?}"
    );
}

#[test]
fn bugfix_rapid_dot_validates() {
    assert!(
        !load("bugfix-rapid.dot").nodes.is_empty(),
        "bugfix-rapid.dot has nodes"
    );
}

#[test]
fn bugfix_rapid_dot_has_goal_gate_unmet_recovery_edge() {
    assert_single_recovery_to_nonterminal("bugfix-rapid.dot");
}

#[tokio::test]
async fn bugfix_rapid_dot_walks_to_done() {
    let (task_run_id, calls) = walk_single_park_pr_workflow_to_done("bugfix-rapid.dot").await;
    assert_recorded_task_branch_publication("bugfix-rapid.dot", &task_run_id, &calls);
}

#[test]
fn bugfix_rapid_dot_uses_worktree_and_publishes_before_pr() {
    assert_verify_lifecycle_uses_worktree("bugfix-rapid.dot");
    assert_publish_branch_before_pr("bugfix-rapid.dot", "report");
}

#[test]
fn refactor_only_dot_validates() {
    assert!(
        !load("refactor-only.dot").nodes.is_empty(),
        "refactor-only.dot has nodes"
    );
}

#[test]
fn refactor_only_dot_has_goal_gate_unmet_recovery_edge() {
    assert_single_recovery_to_nonterminal("refactor-only.dot");
}

#[tokio::test]
async fn refactor_only_dot_walks_to_done() {
    // Two parks: implement (agent) then review (fan_out, 3 verdicts).
    let g = load("refactor-only.dot");
    let h = Harness::new();
    let allocator = StaticAllocator::new("/tmp/agentd-task-wt");
    let engine = h.engine_with_allocator(&g, &allocator);

    let parked = engine.execute(&h.run_id).await.expect("execute");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park at implement, got {other:?}"),
    };
    // implement success -> verify_lifecycle (tool ok) -> review parks for 3.
    let parked = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id: task_run_id.clone(),
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver implement outcome");
    let review_run_id = match park_reason(&parked) {
        ParkReason::ReviewVerdicts {
            review_run_id,
            expected,
            ..
        } => {
            assert_eq!(*expected, 3, "three reviewers");
            review_run_id.clone()
        }
        other => panic!("expected ReviewVerdicts park at review, got {other:?}"),
    };
    // 3 pass verdicts -> aggregate -> open_pr -> report -> done (both gates met).
    let mut last = None;
    for reviewer in ["claude-sec", "codex-perf", "gemini-readability"] {
        last = Some(
            engine
                .deliver_event(EngineEvent::ReviewVerdictSubmitted {
                    review_run_id: review_run_id.clone(),
                    reviewer_id: AgentId::parsed(reviewer),
                    verdict: VerdictValue::Pass,
                    findings: String::new(),
                })
                .await
                .expect("deliver verdict"),
        );
    }
    assert_eq!(
        last.expect("a final progress"),
        RunProgress::Finished {
            run_id: h.run_id.clone()
        }
    );
    let calls = h.runner.calls();
    assert_recorded_task_branch_publication("refactor-only.dot", &task_run_id, &calls);
}

#[test]
fn refactor_only_dot_uses_worktree_and_publishes_before_pr() {
    assert_verify_lifecycle_uses_worktree("refactor-only.dot");
    assert_publish_branch_before_pr("refactor-only.dot", "report");
}

#[tokio::test]
async fn migrated_pr_workflows_walk_to_done_with_task_branch_publication() {
    for file in ["docs-only.dot", "bugfix-rapid.dot"] {
        let (task_run_id, calls) = walk_single_park_pr_workflow_to_done(file).await;
        assert_recorded_task_branch_publication(file, &task_run_id, &calls);
    }

    let g = load("refactor-only.dot");
    let h = Harness::new();
    let allocator = StaticAllocator::new("/tmp/agentd-task-wt");
    let engine = h.engine_with_allocator(&g, &allocator);
    let parked = engine.execute(&h.run_id).await.expect("execute");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome park at implement, got {other:?}"),
    };
    let parked = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id: task_run_id.clone(),
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver implement outcome");
    let review_run_id = match park_reason(&parked) {
        ParkReason::ReviewVerdicts { review_run_id, .. } => review_run_id.clone(),
        other => panic!("expected ReviewVerdicts park at review, got {other:?}"),
    };
    let mut last = None;
    for reviewer in ["claude-sec", "codex-perf", "gemini-readability"] {
        last = Some(
            engine
                .deliver_event(EngineEvent::ReviewVerdictSubmitted {
                    review_run_id: review_run_id.clone(),
                    reviewer_id: AgentId::parsed(reviewer),
                    verdict: VerdictValue::Pass,
                    findings: String::new(),
                })
                .await
                .expect("deliver verdict"),
        );
    }
    assert_eq!(
        last.expect("a final progress"),
        RunProgress::Finished {
            run_id: h.run_id.clone()
        }
    );
    let calls = h.runner.calls();
    assert_recorded_task_branch_publication("refactor-only.dot", &task_run_id, &calls);
}

// ─── §7.3 P1.5: bootstrap ────────────────────────────────────────────────────

#[test]
fn bootstrap_dot_validates() {
    assert!(
        !load("bootstrap.dot").nodes.is_empty(),
        "bootstrap.dot has nodes"
    );
}

#[test]
fn bootstrap_dot_uses_real_agent_spec_discover() {
    let g = load("bootstrap.dot");
    let discover = g
        .nodes
        .iter()
        .find(|n| n.id == "discover_spec")
        .expect("bootstrap.dot has a discover_spec node");
    assert_eq!(
        discover.attr("handler"),
        Some("tool"),
        "discover_spec should be a tool node"
    );
    assert_eq!(
        discover.attr("cmd"),
        Some(
            "agent-spec discover --from-codebase --code . --name bootstrap --out bootstrap.spec.md"
        )
    );
    assert!(
        g.nodes
            .iter()
            .all(|n| n.attr("handler") != Some("codergen")),
        "bootstrap.dot should not park a spec-writer agent"
    );
}

#[tokio::test]
async fn bootstrap_dot_walks_to_done_without_agent_park() {
    let g = load("bootstrap.dot");
    let h = Harness::new();
    h.push_ok(3);

    let progress = h.engine(&g).execute(&h.run_id).await.expect("execute");
    assert_eq!(
        progress,
        RunProgress::Finished {
            run_id: h.run_id.clone()
        },
        "bootstrap.dot should reach Finished without parking"
    );
    assert!(
        h.backend.spawned().is_empty(),
        "bootstrap.dot should not spawn an agent"
    );
    let calls = h.runner.calls();
    assert_eq!(calls.len(), 3, "discover, lint, report should run");
    assert_eq!(calls[0].program, "agent-spec");
    assert_eq!(
        calls[0].args,
        vec![
            "discover".to_string(),
            "--from-codebase".to_string(),
            "--code".to_string(),
            ".".to_string(),
            "--name".to_string(),
            "bootstrap".to_string(),
            "--out".to_string(),
            "bootstrap.spec.md".to_string(),
        ]
    );
}

#[tokio::test]
async fn bootstrap_discover_failure_stops_before_lint() {
    let g = load("bootstrap.dot");
    let h = Harness::new();
    h.runner.push_output(Ok(CommandOutput {
        stdout: String::new(),
        stderr: "discover failed".to_string(),
        status: 2,
    }));

    let progress = h.engine(&g).execute(&h.run_id).await.expect("execute");
    assert!(
        matches!(&progress, RunProgress::Failed { reason, .. } if reason.contains("discover_spec")),
        "discover failure should fail the run at discover_spec, got {progress:?}"
    );
    let calls = h.runner.calls();
    assert_eq!(
        calls.len(),
        1,
        "lint and report must not run after discover failure"
    );
    assert_eq!(calls[0].program, "agent-spec");
    assert_eq!(calls[0].args.first().map(String::as_str), Some("discover"));
}
