//! Tests for `agentd_core::graph::NodeGraph` validation. Names match the spec `Test:` selectors.

use agentd_core::dot::parser;
use agentd_core::graph::NodeGraph;

fn build(src: &str) -> Result<NodeGraph, agentd_core::CoreError> {
    let ast = parser::parse(src).expect("dot parse");
    NodeGraph::from_ast(&ast)
}

fn err_msg(src: &str) -> String {
    match build(src) {
        Ok(_) => panic!("expected validation error, got Ok"),
        Err(e) => format!("{e}"),
    }
}

#[test]
fn node_graph_rejects_no_start() {
    let src = r#"digraph m {
        "work" [handler=tool];
        "end" [shape=Msquare];
        "work" -> "end";
    }"#;
    let msg = err_msg(src);
    assert!(msg.contains("start"), "msg: {msg}");
}

#[test]
fn node_graph_rejects_no_terminal() {
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "work" [handler=tool];
        "start" -> "work";
    }"#;
    let msg = err_msg(src);
    assert!(msg.contains("terminal"), "msg: {msg}");
}

#[test]
fn node_graph_rejects_unknown_handler() {
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "work" [handler=stack.manager_loop];
        "end" [shape=Msquare];
        "start" -> "work";
        "work" -> "end";
    }"#;
    let msg = err_msg(src);
    assert!(
        msg.contains("handler") && msg.contains("stack.manager_loop"),
        "msg: {msg}"
    );
}

#[test]
fn node_graph_rejects_unreachable_node() {
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "end" [shape=Msquare];
        "orphan" [handler=tool];
        "start" -> "end";
    }"#;
    let msg = err_msg(src);
    assert!(
        msg.contains("unreachable") && msg.contains("orphan"),
        "msg: {msg}"
    );
}

#[test]
fn node_graph_rejects_goal_gate_not_on_any_path() {
    // gate is reachable from start but cannot reach any terminal (dead end).
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "gate" [handler=tool, goal_gate=true];
        "end" [shape=Msquare];
        "start" -> "end";
        "start" -> "gate";
    }"#;
    let msg = err_msg(src);
    assert!(
        msg.contains("goal_gate") && msg.contains("gate"),
        "msg: {msg}"
    );
}

#[test]
fn node_graph_rejects_unknown_pre_tool() {
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "work" [handler=codergen, pre_tools="frobnicate(x=1)"];
        "end" [shape=Msquare];
        "start" -> "work";
        "work" -> "end";
    }"#;
    let msg = err_msg(src);
    assert!(msg.contains("frobnicate"), "msg: {msg}");
}

#[test]
fn node_graph_accepts_retry_target_and_retry_policy_attrs() {
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "impl" [handler=codergen, retry_policy="max=3,backoff=exp"];
        "verify" [handler=tool, goal_gate=true];
        "end" [shape=Msquare];
        "start" -> "impl";
        "impl" -> "verify";
        "verify" -> "end" [condition="outcome=success"];
        "verify" -> "impl" [condition="outcome=fail", retry_target=true];
    }"#;
    build(src).expect("retry_target + retry_policy should validate");
}

#[test]
fn node_graph_rejects_delphi_visibility_in_p0() {
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "review" [handler=parallel.fan_out, visibility=delphi];
        "agg" [handler=parallel.fan_in];
        "end" [shape=Msquare];
        "start" -> "review";
        "review" -> "agg";
        "agg" -> "end";
    }"#;
    let msg = err_msg(src);
    assert!(msg.contains("delphi"), "msg: {msg}");
}

#[test]
fn node_graph_rejects_multi_fan_out_into_one_fan_in() {
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "ra" [handler=parallel.fan_out];
        "rb" [handler=parallel.fan_out];
        "agg" [handler=parallel.fan_in];
        "end" [shape=Msquare];
        "start" -> "ra";
        "start" -> "rb";
        "ra" -> "agg";
        "rb" -> "agg";
        "agg" -> "end";
    }"#;
    let msg = err_msg(src);
    assert!(msg.contains("fan_out"), "msg: {msg}");
}

#[test]
fn node_graph_accepts_minimal_valid_graph() {
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "work" [handler=tool];
        "end" [shape=Msquare];
        "start" -> "work";
        "work" -> "end";
    }"#;
    let g = build(src).expect("minimal valid graph");
    assert_eq!(g.nodes.len(), 3);
    assert_eq!(g.edges.len(), 2);
}

#[test]
fn node_graph_reports_all_violations_not_just_first() {
    // no terminal AND unknown handler — both must appear.
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "work" [handler=stack.manager_loop];
        "start" -> "work";
    }"#;
    let msg = err_msg(src);
    assert!(
        msg.contains("terminal"),
        "missing terminal violation: {msg}"
    );
    assert!(
        msg.contains("stack.manager_loop"),
        "missing unknown-handler violation: {msg}"
    );
}

#[test]
fn node_graph_rejects_multiple_starts() {
    // The engine drives a run from a single entry point, so more than one
    // Mdiamond start would silently execute only the first-declared component.
    let src = r#"digraph m {
        "s1" [shape=Mdiamond];
        "s2" [shape=Mdiamond];
        "work" [handler=tool];
        "end" [shape=Msquare];
        "s1" -> "work";
        "s2" -> "work";
        "work" -> "end";
    }"#;
    let msg = err_msg(src);
    assert!(
        msg.contains("start nodes"),
        "expected a multiple-start violation, got: {msg}"
    );
}
