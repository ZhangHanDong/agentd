//! End-to-end tests for `agentd_core::engine::Engine` — the run loop and
//! `deliver_event`. Names match the spec `Test:` selectors. Requires `test-support`.

use agentd_core::dot::parser;
use agentd_core::engine::{Engine, EngineEvent, ParkReason, RunProgress};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{HandlerRegistry, Ports};
use agentd_core::ports::{CommandOutput, RunStatus, Store};
use agentd_core::test_support::{
    FakeBackend, FixedClock, InMemoryStore, MempalStub, RecordingCommandRunner,
};
use agentd_core::types::{NodeId, Outcome, ReviewRunId, RunId, TaskRunId, VerdictValue};

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
}

fn graph(src: &str) -> NodeGraph {
    let ast = parser::parse(src).expect("dot parse");
    NodeGraph::from_ast_unvalidated(&ast)
}

fn finished(run_id: &RunId) -> RunProgress {
    RunProgress::Finished {
        run_id: run_id.clone(),
    }
}

fn park_reason(progress: &RunProgress) -> &ParkReason {
    match progress {
        RunProgress::Parked { reason, .. } => reason,
        other => panic!("expected Parked, got {other:?}"),
    }
}

const MINIMAL: &str = r#"digraph m {
    "start" [shape=Mdiamond];
    "decide" [handler=conditional];
    "work" [handler=tool, cmd="echo hi"];
    "end" [shape=Msquare];
    "start" -> "decide";
    "decide" -> "work";
    "work" -> "end";
}"#;

#[tokio::test]
async fn engine_executes_minimal_three_node_graph_to_terminal() {
    let h = Harness::new();
    let g = graph(MINIMAL);
    h.runner.push_output(Ok(CommandOutput {
        stdout: "hi\n".to_string(),
        stderr: String::new(),
        status: 0,
    }));
    let progress = h.engine(&g).execute(&h.run_id).await.expect("execute");
    assert_eq!(progress, finished(&h.run_id));
}

#[tokio::test]
async fn engine_persists_outcome_after_each_done_node() {
    let h = Harness::new();
    let g = graph(MINIMAL);
    h.engine(&g).execute(&h.run_id).await.expect("execute");
    assert!(
        h.store
            .latest_outcome(&h.run_id, &NodeId::parsed("decide"))
            .await
            .expect("latest decide")
            .is_some(),
        "conditional node outcome persisted"
    );
    assert!(
        h.store
            .latest_outcome(&h.run_id, &NodeId::parsed("work"))
            .await
            .expect("latest work")
            .is_some(),
        "tool node outcome persisted"
    );
}

const WAIT_GRAPH: &str = r#"digraph m {
    "start" [shape=Mdiamond];
    "ask" [handler="wait.human", prompt="approve?"];
    "end" [shape=Msquare];
    "abandoned" [shape=Msquare];
    "start" -> "ask";
    "ask" -> "end" [condition="answer=approve"];
    "ask" -> "abandoned" [condition="answer=abandon"];
}"#;

#[tokio::test]
async fn engine_writes_checkpoint_after_each_node_including_parks() {
    let h = Harness::new();
    let g = graph(WAIT_GRAPH);
    let progress = h.engine(&g).execute(&h.run_id).await.expect("execute");
    assert!(matches!(progress, RunProgress::Parked { .. }));
    let checkpoint = h
        .store
        .load_checkpoint(&h.run_id)
        .await
        .expect("load")
        .expect("a checkpoint exists after the park");
    assert_eq!(checkpoint.current_node, NodeId::parsed("ask"));
}

#[tokio::test]
async fn engine_parks_on_wait_human_then_resumes_to_finish_via_deliver_event() {
    let h = Harness::new();
    let g = graph(WAIT_GRAPH);
    let engine = h.engine(&g);
    let parked = engine.execute(&h.run_id).await.expect("execute");
    let wait_id = match park_reason(&parked) {
        ParkReason::HumanAnswer { wait_id } => wait_id.clone(),
        other => panic!("expected HumanAnswer, got {other:?}"),
    };
    let done = engine
        .deliver_event(EngineEvent::HumanAnswered {
            wait_id,
            answer: "approve".to_string(),
            feedback: None,
        })
        .await
        .expect("deliver");
    assert_eq!(done, finished(&h.run_id));
}

const GOAL_GATE_GRAPH: &str = r#"digraph m {
    "start" [shape=Mdiamond];
    "gate" [handler=tool, cmd="check", goal_gate=true];
    "end" [shape=Msquare];
    "start" -> "gate";
    "gate" -> "end";
}"#;

#[tokio::test]
async fn engine_goal_gate_blocks_terminal_until_satisfied() {
    // Gate tool fails (non-zero exit) → goal_gate unmet → run cannot reach terminal.
    let failing = Harness::new();
    let g = graph(GOAL_GATE_GRAPH);
    failing.runner.push_output(Ok(CommandOutput {
        stdout: String::new(),
        stderr: "nope".to_string(),
        status: 1,
    }));
    let blocked = failing
        .engine(&g)
        .execute(&failing.run_id)
        .await
        .expect("execute fail");
    assert!(
        matches!(blocked, RunProgress::Failed { .. }),
        "an unmet goal_gate blocks the terminal, got {blocked:?}"
    );

    // Gate tool succeeds → goal_gate met → run finishes.
    let passing = Harness::new();
    passing.runner.push_output(Ok(CommandOutput {
        stdout: "ok\n".to_string(),
        stderr: String::new(),
        status: 0,
    }));
    let done = passing
        .engine(&g)
        .execute(&passing.run_id)
        .await
        .expect("execute ok");
    assert_eq!(done, finished(&passing.run_id));
}

const CANONICAL: &str = r#"digraph m {
    "start" [shape=Mdiamond];
    "spec" [handler="wait.human", prompt="approve spec?"];
    "impl" [handler="codergen", role="implementer"];
    "review" [handler="parallel.fan_out", reviewers="claude-sec,codex-perf,gemini-readability"];
    "aggregate" [handler="parallel.fan_in", aggregator="majority_pass", goal_gate=true];
    "end" [shape=Msquare];
    "start" -> "spec";
    "spec" -> "impl" [condition="answer=approve"];
    "impl" -> "review";
    "review" -> "aggregate";
    "aggregate" -> "end";
}"#;

#[tokio::test]
async fn engine_full_canonical_dot_runs_to_completion_with_fakes() {
    let h = Harness::new();
    let g = graph(CANONICAL);
    let engine = h.engine(&g);

    // start -> spec (wait.human) parks.
    let parked = engine.execute(&h.run_id).await.expect("execute");
    let wait_id = match park_reason(&parked) {
        ParkReason::HumanAnswer { wait_id } => wait_id.clone(),
        other => panic!("expected HumanAnswer, got {other:?}"),
    };

    // approve -> impl (codergen) parks awaiting the agent outcome.
    let parked = engine
        .deliver_event(EngineEvent::HumanAnswered {
            wait_id,
            answer: "approve".to_string(),
            feedback: None,
        })
        .await
        .expect("deliver approve");
    let task_run_id: TaskRunId = match park_reason(&parked) {
        ParkReason::AgentOutcome { task_run_id } => task_run_id.clone(),
        other => panic!("expected AgentOutcome, got {other:?}"),
    };

    // agent outcome -> review (fan_out) parks awaiting verdicts.
    let parked = engine
        .deliver_event(EngineEvent::AgentOutcomeSubmitted {
            task_run_id,
            outcome: Outcome::success(),
        })
        .await
        .expect("deliver agent outcome");
    let review_run_id: ReviewRunId = match park_reason(&parked) {
        ParkReason::ReviewVerdicts {
            review_run_id,
            expected,
        } => {
            assert_eq!(*expected, 3);
            review_run_id.clone()
        }
        other => panic!("expected ReviewVerdicts, got {other:?}"),
    };

    // First two verdicts re-park; the third completes fan_out, runs fan_in, and
    // (goal_gate satisfied by the majority_pass Success) reaches the terminal.
    let reviewers = ["claude-sec", "codex-perf", "gemini-readability"];
    let mut last = None;
    for reviewer in reviewers {
        last = Some(
            engine
                .deliver_event(EngineEvent::ReviewVerdictSubmitted {
                    review_run_id: review_run_id.clone(),
                    reviewer_id: agentd_core::types::AgentId::parsed(reviewer),
                    verdict: VerdictValue::Pass,
                })
                .await
                .expect("deliver verdict"),
        );
    }
    assert_eq!(last.expect("a final progress"), finished(&h.run_id));
}

// ─── STEP_CEILING backstop (regression for the engine review) ────────
//
// gate runs ONCE upstream (one queued failing output), records Fail, and routes
// to the queue-free `spin` conditional. At spin, the lexical unconditional pick
// is spin->end (terminal); the unmet goal_gate synthesizes Fail+label and the
// non-attempt-gated `preferred_label` tier re-picks spin->spin, looping to the
// 10,000-node ceiling — the spec-sanctioned backstop for a pathological graph.
// The fix: the ceiling exit marks the run Failed (not orphaned Running).
const CEILING_LOOP_GRAPH: &str = r#"digraph m {
    "start" [shape=Mdiamond];
    "gate" [handler=tool, cmd="check", goal_gate=true];
    "spin" [handler=conditional];
    "end" [shape=Msquare];
    "start" -> "gate";
    "gate" -> "spin";
    "spin" -> "end";
    "spin" -> "spin" [label="goal_gate_unmet"];
}"#;

#[tokio::test]
async fn ceiling_loop_graph_passes_validation() {
    // The scenario requires a *validated* graph (single start): confirm from_ast accepts it.
    let ast = parser::parse(CEILING_LOOP_GRAPH).expect("parse");
    let result = NodeGraph::from_ast(&ast);
    assert!(
        result.is_ok(),
        "ceiling-loop graph should pass from_ast validation, got {result:?}"
    );
}

#[tokio::test]
async fn step_ceiling_exhaustion_marks_run_failed() {
    let h = Harness::new();
    let g = graph(CEILING_LOOP_GRAPH);
    // Exactly one failing output: the gate runs once (records Fail -> goal_gate
    // permanently unmet). The loop thereafter spins in the queue-free conditional.
    h.runner.push_output(Ok(CommandOutput {
        stdout: String::new(),
        stderr: "nope".to_string(),
        status: 1,
    }));
    let result = h.engine(&g).execute(&h.run_id).await;
    // The run terminates via the STEP_CEILING invariant Err (not a Stuck Fail).
    let err = result.expect_err("run must hit the step ceiling");
    assert!(
        format!("{err:?}").contains("step ceiling"),
        "expected step-ceiling invariant error, got {err:?}"
    );
    // The fix: the store row is marked Failed (never left orphaned as Running),
    // consistent with every other run-ending path.
    assert_eq!(
        h.store.run_status(&h.run_id),
        Some(RunStatus::Failed),
        "ceiling exit must mark the run Failed, not leave it Running"
    );
}
