//! P0.8 workflow-authoring: the standalone Path-B workflows conform to the
//! frozen DOT grammar and walk on the real `Engine`. Test names match
//! `specs/workflow/p80-draft-dot.spec.md` (and p81 for execute.dot).
//!
//! The walk-tests (added with the walk-test tasks) construct the real
//! `agentd_core::Engine` over the `test-support` fakes — NOT `FakeRunHost`,
//! which scripts `RunProgress` and exercises only the MCP tool layer.

use std::path::PathBuf;

use agentd_core::dot::parser;
use agentd_core::engine::{Engine, EngineEvent, ParkReason, RunProgress};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{HandlerRegistry, Ports};
use agentd_core::ports::CommandOutput;
use agentd_core::test_support::{
    FakeBackend, FixedClock, InMemoryStore, MempalStub, RecordingCommandRunner,
};
use agentd_core::types::{Outcome, RunId, TaskRunId};

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
