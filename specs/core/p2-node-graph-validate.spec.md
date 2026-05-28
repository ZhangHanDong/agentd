spec: task
name: "NodeGraph build + validation pass"
tags: [core, mvp, p0, workflow]
---

## Intent

Turn a parsed `dot::ast::Graph` into a typed, validated `graph::NodeGraph`.
The validation pass implements design §2.7: it rejects structurally or
semantically broken workflows up front (missing start/terminal, unknown
handler, unreachable nodes, an unsatisfiable goal_gate, unknown pre_tools /
post_action tools, P1-only delphi mode, and unsupported multi-fan_out
fan-in) and reports ALL violations at once rather than failing on the first.

## Decisions

- `NodeGraph::from_ast(&dot::ast::Graph) -> Result<NodeGraph, CoreError>`
- Node shape: `Mdiamond` = start, `Msquare` = terminal, anything else = regular
- Known handlers (P0): codergen, conditional, tool, wait.human, parallel.fan_out, parallel.fan_in. `stack.manager_loop` is P1+ and counts as an unknown handler in P0.
- A `handler=` attribute, when present, must be a known handler; start/terminal nodes may omit it
- Known tool names for pre_tools / post_action (the §4.12 closed set): assign_task, submit_review, check_inbox, submit_outcome, query_run, mempal_search, mempal_ingest, mempal_kg, mempal_fact_check, mempal_peek_partner, mempal_cowork_push, matrix.post, github.status_push
- pre_tools / post_action are comma-separated token lists; commas inside `(...)` are NOT separators. Each token's tool name is the leading identifier before `(` or whitespace.
- `retry_target` (edge, bool) and `retry_policy` (node, `max=N,...`) are recognized attributes (D8b/D8c) and never cause a validation error
- DEVIATION from the plan delta: unknown attributes are NOT a hard error. P0.1 validates only the attributes the checks above need and treats all other attributes as forward-compatible (ignored). Hard-rejecting unknown attributes would break the design's own Appendix C DOT (which uses many attributes outside the §2.2 table) and is not exercised by any scenario.
- `visibility=delphi` and any `converge_or_*` aggregator are rejected (design §2.5.1 — reserved for P1+)
- Multi-fan_out into one fan_in: a `parallel.fan_in` reachable from ≥2 `parallel.fan_out` nodes (none carrying `pair_with`) is rejected in P0.1
- Violations accumulate; `from_ast` returns `Err(CoreError::GraphValidate(joined))` listing every violation

## Boundaries

### Allowed Changes

- crates/agentd-core/src/graph/**
- crates/agentd-core/src/lib.rs
- crates/agentd-core/tests/node_graph.rs

### Forbidden

- Do not fail on the first violation; collect and report all.
- Do not accept `visibility=delphi` or `converge_or_*` in P0.1.
- Do not treat `stack.manager_loop` as a known handler in P0.

## Completion Criteria

Scenario: A graph with no start node is rejected
  Test: node_graph_rejects_no_start
  Given a parsed graph with a terminal but no Mdiamond start node
  When NodeGraph::from_ast runs
  Then it returns an error mentioning a missing start node

Scenario: A graph with no terminal node is rejected
  Test: node_graph_rejects_no_terminal
  Given a parsed graph with a start but no Msquare terminal node
  When NodeGraph::from_ast runs
  Then it returns an error mentioning a missing terminal node

Scenario: An unknown handler is rejected
  Test: node_graph_rejects_unknown_handler
  Given a parsed graph whose regular node declares handler=stack.manager_loop
  When NodeGraph::from_ast runs
  Then it returns an error mentioning the unknown handler

Scenario: An unreachable node is rejected
  Test: node_graph_rejects_unreachable_node
  Given a parsed graph with a node that no edge from the start can reach
  When NodeGraph::from_ast runs
  Then it returns an error mentioning the unreachable node

Scenario: A goal_gate node that cannot reach a terminal is rejected
  Test: node_graph_rejects_goal_gate_not_on_any_path
  Given a parsed graph with a goal_gate=true node that has no path to any terminal
  When NodeGraph::from_ast runs
  Then it returns an error mentioning the goal_gate node

Scenario: An unknown pre_tools tool is rejected
  Test: node_graph_rejects_unknown_pre_tool
  Given a parsed graph whose node declares pre_tools with an unrecognized tool name
  When NodeGraph::from_ast runs
  Then it returns an error mentioning the unknown tool

Scenario: retry_target and retry_policy attributes are accepted
  Test: node_graph_accepts_retry_target_and_retry_policy_attrs
  Given a valid parsed graph using retry_target on an edge and retry_policy on a node
  When NodeGraph::from_ast runs
  Then it succeeds

Scenario: delphi visibility is rejected in P0
  Test: node_graph_rejects_delphi_visibility_in_p0
  Given a parsed graph whose fan_out node declares visibility=delphi
  When NodeGraph::from_ast runs
  Then it returns an error mentioning delphi

Scenario: Multiple fan_out into one fan_in is rejected
  Test: node_graph_rejects_multi_fan_out_into_one_fan_in
  Given a parsed graph with two parallel.fan_out nodes both reaching one parallel.fan_in
  When NodeGraph::from_ast runs
  Then it returns an error mentioning fan_out pairing

Scenario: A minimal valid graph is accepted
  Test: node_graph_accepts_minimal_valid_graph
  Given a parsed graph with a start, one tool node, and a terminal, all connected
  When NodeGraph::from_ast runs
  Then it succeeds with the nodes and edges preserved

Scenario: All violations are reported, not just the first
  Test: node_graph_reports_all_violations_not_just_first
  Given a parsed graph that simultaneously lacks a terminal and uses an unknown handler
  When NodeGraph::from_ast runs
  Then the error message mentions both the missing terminal and the unknown handler
