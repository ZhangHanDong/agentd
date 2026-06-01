//! P0.8 workflow-authoring: the standalone Path-B workflows conform to the
//! frozen DOT grammar and walk on the real `Engine`. Test names match
//! `specs/workflow/p80-draft-dot.spec.md` (and p81 for execute.dot).
//!
//! The walk-tests (added with the walk-test tasks) construct the real
//! `agentd_core::Engine` over the `test-support` fakes — NOT `FakeRunHost`,
//! which scripts `RunProgress` and exercises only the MCP tool layer.

use std::path::PathBuf;

use agentd_core::dot::parser;
use agentd_core::graph::NodeGraph;

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
