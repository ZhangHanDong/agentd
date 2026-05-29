spec: task
name: "DOT parser for .agentflow files"
tags: [core, mvp, p0, workflow]
---

## Intent

Parse `.agentflow/*.dot` files into a typed `dot::ast::Graph` containing
nodes (with attribute maps) and directed edges (with attribute maps).
The parser handles the subset of DOT we use: digraph keyword, named
nodes with `[attr=value, ...]` blocks, edges `"a" -> "b" [attr=...]`,
quoted and unquoted attribute values, `//` line comments, and trailing
commas/semicolons. Anything outside this subset is a parse error.

## Decisions

- Hand-written recursive parser, no external grammar deps
- Identifier characters: `a-z A-Z 0-9 _ - .`
- Attribute values are either bare identifiers, **non-negative** integers, booleans
  (true/false), or double-quoted strings (with `\"` / `\n` / `\t` / `\r` / `\\`
  escapes). P0.1 LIMITATION: a leading `-` is reserved for the `->` arrow, so a
  bare negative integer (`k=-5`) is not lexable — quote it (`k="-5"`) if needed.
- P0.1 LEXING CONVENTION: node ids must be quoted, or unquoted but **separated
  from `->` by whitespace** (`a -> b`, not `a->b`) — `-` is an identifier char,
  so an unquoted id glued to the arrow mis-lexes. All shipped/sample workflows
  quote ids. (Both this and the negative-integer gap are deferred cosmetic lexer
  edges with no P0.1-reachable impact — a tracked relaxation, not a bug.)
- Comments are `// ... end-of-line` only; no `/* ... */`
- Node statements may appear standalone or implicitly through being referenced in an edge (the parser tolerates an implicit endpoint; `NodeGraph` validation then requires every edge endpoint to resolve to a declared node — see p2)
- subgraph keyword reserved but rejected in v0 (error message points to roadmap)
- Order preservation: nodes and edges retain source order for deterministic iteration
- The lexer offers one-token lookahead (`peek_tok`); the token consumer is `next_tok`

## Boundaries

### Allowed Changes

- crates/agentd-core/src/dot/**
- crates/agentd-core/src/lib.rs
- crates/agentd-core/tests/dot_parser.rs

### Forbidden

- Do not depend on external DOT/graphviz crates.
- Do not silently accept tokens outside the documented subset.
- Do not normalize or lowercase identifiers.

## Completion Criteria

Scenario: Parse a minimal digraph with two nodes and an edge
  Test: dot_parser_minimal_digraph
  Given a DOT source declaring nodes "a" and "b" plus an edge "a" -> "b"
  When the parser runs
  Then the result has 2 nodes with ids "a" and "b"
  And node "a" has attribute shape=Mdiamond
  And node "b" has attribute shape=Msquare
  And there is one edge from "a" to "b" with no attributes

Scenario: Node attributes accept quoted strings with metacharacters
  Test: dot_parser_quoted_attributes
  Given a DOT source declaring a node with a prompt attribute containing an escaped newline and quote
  When the parser runs
  Then the node's prompt attribute equals the literal string with the embedded newline and quote

Scenario: Comments are stripped
  Test: dot_parser_strips_line_comments
  Given a DOT source with line comments mixed in
  When the parser runs
  Then the parsed graph is identical to the comment-free version

Scenario: Trailing commas and semicolons tolerated
  Test: dot_parser_trailing_punct
  Given a DOT source where every node and edge ends with a semicolon
  And one node has a trailing comma after its last attribute
  When the parser runs
  Then parsing succeeds

Scenario: Unknown subgraph keyword is rejected with a helpful error
  Test: dot_parser_subgraph_rejected
  Given a DOT source containing a subgraph block
  When the parser runs
  Then it returns an error
  And the error message contains "subgraph not supported"

Scenario: Edge with attributes preserves order and key/value
  Test: dot_parser_edge_attributes
  Given a DOT source with an edge carrying condition and label attributes
  When the parser runs
  Then the edge has condition="outcome=success" and label="approve"

Scenario: Empty graph yields zero nodes and zero edges
  Test: dot_parser_empty_graph
  Given a DOT source with an empty digraph named empty
  When the parser runs
  Then there are 0 nodes and 0 edges
  And the graph name is "empty"
