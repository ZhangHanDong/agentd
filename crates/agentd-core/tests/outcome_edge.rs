//! Tests for `agentd_core::graph::edge_select`. Names match the spec `Test:` selectors.

use std::collections::HashMap;

use agentd_core::dot::parser;
use agentd_core::graph::{NodeGraph, edge_select::select_next_edge};
use agentd_core::types::{Outcome, RunContext};

fn graph(src: &str) -> NodeGraph {
    let ast = parser::parse(src).expect("dot parse");
    // Build directly without full validation so tests can use tiny fragments.
    NodeGraph::from_ast_unvalidated(&ast)
}

#[test]
fn edge_select_condition_first() {
    let g = graph(
        r#"digraph m {
        "n" -> "yes" [condition="outcome=success"];
        "n" -> "plain";
    }"#,
    );
    let out = Outcome::success();
    let e = select_next_edge(&g, "n", &out, &RunContext::new(), &HashMap::new()).expect("edge");
    assert_eq!(e.to, "yes");
}

#[test]
fn edge_select_falls_back_to_preferred_label() {
    let g = graph(
        r#"digraph m {
        "n" -> "a" [label="approve"];
        "n" -> "r" [label="reject"];
    }"#,
    );
    let mut out = Outcome::success();
    out.preferred_label = Some("approve".to_string());
    let e = select_next_edge(&g, "n", &out, &RunContext::new(), &HashMap::new()).expect("edge");
    assert_eq!(e.to, "a");
}

#[test]
fn edge_select_prefers_retry_target_on_fail_when_under_attempt_ceiling() {
    let g = graph(
        r#"digraph m {
        "impl" [retry_policy="max=3"];
        "verify" -> "impl" [retry_target=true];
        "verify" -> "end";
    }"#,
    );
    let out = Outcome::fail();
    let mut attempts = HashMap::new();
    attempts.insert("impl".to_string(), 1_u32);
    let e = select_next_edge(&g, "verify", &out, &RunContext::new(), &attempts).expect("edge");
    assert_eq!(e.to, "impl");
}

#[test]
fn edge_select_skips_retry_target_when_attempt_ceiling_reached() {
    let g = graph(
        r#"digraph m {
        "impl" [retry_policy="max=2"];
        "verify" -> "impl" [retry_target=true];
        "verify" -> "dead";
    }"#,
    );
    let out = Outcome::fail();
    let mut attempts = HashMap::new();
    attempts.insert("impl".to_string(), 2_u32);
    let e = select_next_edge(&g, "verify", &out, &RunContext::new(), &attempts).expect("edge");
    assert_eq!(e.to, "dead", "should fall through to the non-retry edge");
}

#[test]
fn edge_select_uses_weight_when_no_label() {
    let g = graph(
        r#"digraph m {
        "n" -> "low" [weight=1];
        "n" -> "high" [weight=5];
    }"#,
    );
    let out = Outcome::success();
    let e = select_next_edge(&g, "n", &out, &RunContext::new(), &HashMap::new()).expect("edge");
    assert_eq!(e.to, "high");
}

#[test]
fn edge_select_lex_tiebreak_and_none_when_no_match() {
    let g = graph(
        r#"digraph m {
        "n" -> "beta";
        "n" -> "alpha";
    }"#,
    );
    let out = Outcome::success();
    let e = select_next_edge(&g, "n", &out, &RunContext::new(), &HashMap::new()).expect("edge");
    assert_eq!(e.to, "alpha", "lexical tiebreak picks alpha");

    // a node with no outgoing edges yields None
    assert!(select_next_edge(&g, "alpha", &out, &RunContext::new(), &HashMap::new()).is_none());
}

#[test]
fn edge_select_answer_condition_matches_context() {
    let g = graph(
        r#"digraph m {
        "n" -> "approved" [condition="answer=approve"];
        "n" -> "other";
    }"#,
    );
    let out = Outcome::success();
    let mut ctx = RunContext::new();
    ctx.set("answer", serde_json::Value::String("approve".to_string()));
    let e = select_next_edge(&g, "n", &out, &ctx, &HashMap::new()).expect("edge");
    assert_eq!(e.to, "approved");
}
