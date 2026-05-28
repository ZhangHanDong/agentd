//! Tests for `agentd_core::engine::goal_gate`. Names match the spec `Test:` selectors.

use std::collections::BTreeMap;

use agentd_core::dot::parser;
use agentd_core::engine::goal_gate::{GoalGateStatus, evaluate};
use agentd_core::graph::NodeGraph;
use agentd_core::types::{NodeId, Outcome, Status};

fn graph(src: &str) -> NodeGraph {
    let ast = parser::parse(src).expect("dot parse");
    NodeGraph::from_ast_unvalidated(&ast)
}

/// A graph with two gate nodes (`build`, `review`) plus a non-gate node.
fn two_gate_graph() -> NodeGraph {
    graph(
        r#"digraph m {
            "start" [shape=Mdiamond];
            "build" [handler=tool, goal_gate=true];
            "review" [handler=tool, goal_gate=true];
            "note" [handler=tool];
            "end" [shape=Msquare];
            "start" -> "build";
            "build" -> "review";
            "review" -> "end";
        }"#,
    )
}

fn outcome(status: Status) -> Outcome {
    Outcome {
        status,
        ..Outcome::success()
    }
}

#[test]
fn goal_gate_evaluate_met_when_all_gates_success_or_partial() {
    let g = two_gate_graph();
    let mut outcomes = BTreeMap::new();
    outcomes.insert(NodeId::parsed("build"), outcome(Status::Success));
    outcomes.insert(NodeId::parsed("review"), outcome(Status::PartialSuccess));
    let status = evaluate(&g, &outcomes);
    assert_eq!(
        status,
        GoalGateStatus {
            met: true,
            missing: vec![]
        }
    );
}

#[test]
fn goal_gate_evaluate_unmet_lists_missing_gate_nodes() {
    let g = two_gate_graph();
    let mut outcomes = BTreeMap::new();
    outcomes.insert(NodeId::parsed("build"), outcome(Status::Success));
    outcomes.insert(NodeId::parsed("review"), outcome(Status::Fail));
    let status = evaluate(&g, &outcomes);
    assert!(!status.met);
    assert_eq!(status.missing, vec![NodeId::parsed("review")]);
}

#[test]
fn goal_gate_evaluate_unmet_when_a_gate_has_no_outcome() {
    let g = two_gate_graph();
    let mut outcomes = BTreeMap::new();
    // Only `build` ran; `review` has no recorded outcome at all.
    outcomes.insert(NodeId::parsed("build"), outcome(Status::Success));
    let status = evaluate(&g, &outcomes);
    assert!(!status.met);
    assert_eq!(status.missing, vec![NodeId::parsed("review")]);
}

#[test]
fn goal_gate_evaluate_partial_success_counts_as_met() {
    let g = graph(
        r#"digraph m {
            "start" [shape=Mdiamond];
            "build" [handler=tool, goal_gate=true];
            "end" [shape=Msquare];
            "start" -> "build";
            "build" -> "end";
        }"#,
    );
    let mut outcomes = BTreeMap::new();
    outcomes.insert(NodeId::parsed("build"), outcome(Status::PartialSuccess));
    let status = evaluate(&g, &outcomes);
    assert_eq!(
        status,
        GoalGateStatus {
            met: true,
            missing: vec![]
        }
    );
}
