//! Tests for `agentd_core::dot::parser`. Each test is named after the spec scenario's `Test:` selector.

use agentd_core::dot::parser;
use pretty_assertions::assert_eq;

#[test]
fn dot_parser_minimal_digraph() {
    let src = r#"
        digraph m {
          "a" [shape=Mdiamond];
          "b" [shape=Msquare];
          "a" -> "b";
        }
    "#;
    let g = parser::parse(src).expect("parse");
    assert_eq!(g.name, "m");
    assert_eq!(g.nodes.len(), 2);
    assert_eq!(g.nodes[0].id, "a");
    assert_eq!(g.nodes[0].attrs.get("shape"), Some(&"Mdiamond".to_string()));
    assert_eq!(g.nodes[1].id, "b");
    assert_eq!(g.nodes[1].attrs.get("shape"), Some(&"Msquare".to_string()));
    assert_eq!(g.edges.len(), 1);
    assert_eq!(g.edges[0].from, "a");
    assert_eq!(g.edges[0].to, "b");
    assert!(g.edges[0].attrs.is_empty());
}

#[test]
fn dot_parser_quoted_attributes() {
    let src = r#"digraph m { "p" [prompt="line 1\nline 2 with quote \""]; }"#;
    let g = parser::parse(src).expect("parse");
    let prompt = g.nodes[0].attrs.get("prompt").expect("prompt attr");
    assert_eq!(prompt, "line 1\nline 2 with quote \"");
}

#[test]
fn dot_parser_strips_line_comments() {
    let with_comments = r#"
        digraph m {
          // this node is the start
          "a" [shape=Mdiamond];
          // this edge goes from a to b
          "a" -> "b";
          "b" [shape=Msquare];
        }
    "#;
    let without = r#"digraph m { "a" [shape=Mdiamond]; "a" -> "b"; "b" [shape=Msquare]; }"#;
    let g1 = parser::parse(with_comments).expect("parse with comments");
    let g2 = parser::parse(without).expect("parse without comments");
    assert_eq!(g1, g2);
}

#[test]
fn dot_parser_trailing_punct() {
    let src = r#"digraph m { "a" [shape=Mdiamond, ]; "a" -> "b"; "b"; }"#;
    parser::parse(src).expect("parse should tolerate trailing punctuation");
}

#[test]
fn dot_parser_subgraph_rejected() {
    let src = r#"digraph m { subgraph cluster_0 { "a"; } }"#;
    let err = parser::parse(src).expect_err("expected subgraph rejection");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("subgraph not supported"),
        "error did not mention subgraph: {msg}"
    );
}

#[test]
fn dot_parser_edge_attributes() {
    let src = r#"digraph m { "a" -> "b" [condition="outcome=success", label="approve"]; }"#;
    let g = parser::parse(src).expect("parse");
    let edge = &g.edges[0];
    assert_eq!(
        edge.attrs.get("condition"),
        Some(&"outcome=success".to_string())
    );
    assert_eq!(edge.attrs.get("label"), Some(&"approve".to_string()));
}

#[test]
fn dot_parser_empty_graph() {
    let src = "digraph empty { }";
    let g = parser::parse(src).expect("parse");
    assert_eq!(g.name, "empty");
    assert!(g.nodes.is_empty());
    assert!(g.edges.is_empty());
}
