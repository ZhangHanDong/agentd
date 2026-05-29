//! Runs a small workflow end-to-end against the in-memory fakes, printing the
//! progress after each step. Demonstrates the park/resume cycle without a real
//! database, tmux server, or agent CLI.
//!
//! Run with: `cargo run -p agentd-core --example minimal_engine_run`

use agentd_core::dot::parser;
use agentd_core::engine::{Engine, EngineEvent, ParkReason, RunProgress};
use agentd_core::graph::NodeGraph;
use agentd_core::handler::{HandlerRegistry, Ports};
use agentd_core::test_support::{
    FakeBackend, FixedClock, InMemoryStore, MempalStub, RecordingCommandRunner,
};
use agentd_core::types::{AgentId, RunId, VerdictValue};

const FLOW: &str = r#"digraph demo {
    "start"     [shape=Mdiamond];
    "spec"      [handler="wait.human", prompt="approve the spec?"];
    "review"    [handler="parallel.fan_out", reviewers="claude-sec,codex-perf"];
    "aggregate" [handler="parallel.fan_in", aggregator="majority_pass", goal_gate=true];
    "done"      [shape=Msquare];
    "start"     -> "spec";
    "spec"      -> "review"    [condition="answer=approve"];
    "review"    -> "aggregate";
    "aggregate" -> "done";
}"#;

#[tokio::main]
async fn main() {
    let ast = parser::parse(FLOW).expect("parse demo flow");
    let graph = NodeGraph::from_ast(&ast).expect("validate demo flow");

    let backend = FakeBackend::new();
    let runner = RecordingCommandRunner::new();
    let store = InMemoryStore::new();
    let mempal = MempalStub::new();
    let clock = FixedClock::new(0);
    let registry = HandlerRegistry::with_builtins();
    let ports = Ports {
        backend: &backend,
        runner: &runner,
        store: &store,
        mempal: &mempal,
        clock: &clock,
    };
    let engine = Engine::new(&graph, &registry, ports, "demo-sha");
    let run_id = RunId::from_string("demo-run");

    // 1. Start the run — it parks at the wait.human spec node.
    let progress = engine.execute(&run_id).await.expect("execute");
    println!("after execute: {progress:?}");
    let wait_id = expect_human(&progress);

    // 2. Approve the spec — the run advances to fan_out and parks for verdicts.
    let progress = engine
        .deliver_event(EngineEvent::HumanAnswered {
            wait_id,
            answer: "approve".to_string(),
            feedback: None,
        })
        .await
        .expect("deliver approve");
    println!("after approve:  {progress:?}");
    let review_run_id = expect_review(&progress);

    // 3. Two reviewers pass — fan_in aggregates, the goal_gate is satisfied, and
    //    the run reaches the terminal.
    for reviewer in ["claude-sec", "codex-perf"] {
        let progress = engine
            .deliver_event(EngineEvent::ReviewVerdictSubmitted {
                review_run_id: review_run_id.clone(),
                reviewer_id: AgentId::parsed(reviewer),
                verdict: VerdictValue::Pass,
            })
            .await
            .expect("deliver verdict");
        println!("after {reviewer}: {progress:?}");
    }
}

fn expect_human(progress: &RunProgress) -> String {
    match progress {
        RunProgress::Parked {
            reason: ParkReason::HumanAnswer { wait_id },
            ..
        } => wait_id.clone(),
        other => panic!("expected a human-answer park, got {other:?}"),
    }
}

fn expect_review(progress: &RunProgress) -> agentd_core::types::ReviewRunId {
    match progress {
        RunProgress::Parked {
            reason: ParkReason::ReviewVerdicts { review_run_id, .. },
            ..
        } => review_run_id.clone(),
        other => panic!("expected a review-verdicts park, got {other:?}"),
    }
}
