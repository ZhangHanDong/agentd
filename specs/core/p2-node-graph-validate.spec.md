spec: task
name: "NodeGraph build + validation pass"
tags: [core, mvp, p0, workflow]
---

## Intent

Turn a parsed `dot::ast::Graph` into a typed, validated `graph::NodeGraph`.
The validation pass implements design §2.7: it rejects structurally or
semantically broken workflows up front (missing start/terminal, unknown
handler, unreachable nodes, an unsatisfiable goal_gate, unknown pre_tools /
post_action tools, malformed Delphi review contracts, and unsupported
multi-fan_out fan-in) and reports ALL violations at once rather than failing on
the first.

## Decisions

- `NodeGraph::from_ast(&dot::ast::Graph) -> Result<NodeGraph, CoreError>`
- Node shape: `Mdiamond` = start, `Msquare` = terminal, anything else = regular
- Known handlers (P0): codergen, conditional, tool, wait.human, parallel.fan_out, parallel.fan_in. `stack.manager_loop` is P1+ and counts as an unknown handler in P0.
- A `handler=` attribute, when present, must be a known handler; start/terminal nodes may omit it
- Known tool names for pre_tools / post_action (the §4.12 closed set): assign_task, submit_review, check_inbox, submit_outcome, query_run, mempal_search, mempal_ingest, mempal_kg, mempal_fact_check, mempal_peek_partner, mempal_cowork_push, matrix.post, github.status_push
- pre_tools / post_action are comma-separated token lists; commas inside `(...)` are NOT separators. Each token's tool name is the leading identifier before `(` or whitespace.
- `retry_target` (edge, bool) and `retry_policy` (node, `max=N,...`) are recognized attributes (D8b/D8c) and never cause a validation error
- DEVIATION from the plan delta: unknown attributes are NOT a hard error. P0.1 validates only the attributes the checks above need and treats all other attributes as forward-compatible (ignored). Hard-rejecting unknown attributes would break the design's own Appendix C DOT (which uses many attributes outside the §2.2 table) and is not exercised by any scenario.
- P109 enables `visibility=delphi` only for a well-formed P1.4 contract: `max_rounds >= 2`, exactly one reachable fan_in partner, and `aggregator=converge_or_<fallback>` where the fallback is one of `any_fail`, `majority_pass`, `unanimous_pass`, or `first_blocker`
- P113 supports `convergence=verdict_stable` and `convergence=findings_diff<N>` for Delphi, where `N` is a finite `0.0..=1.0` threshold
- A non-Delphi fan_out may keep `max_rounds` unset or `1`; `max_rounds >= 2` requires `visibility=delphi`
- Multi-fan_out into one fan_in: a `parallel.fan_in` reachable from ≥2 `parallel.fan_out` nodes (none carrying `pair_with`) is rejected in P0.1
- Exactly one start node: zero starts and more than one start are both rejected (the engine drives from a single entry point)
- Every edge endpoint must resolve to a declared node: the parser tolerates an implicit edge-referenced id (DOT semantics), but validation rejects it so the engine never steps into a node missing from the graph
- Duplicate node ids are rejected: a repeated declaration would corrupt shape/handler classification (e.g. the same id as both start and terminal)
- Violations accumulate; `from_ast` returns `Err(CoreError::GraphValidate(joined))` listing every violation

## Boundaries

### Allowed Changes

- crates/agentd-core/src/graph/**
- crates/agentd-core/src/lib.rs
- crates/agentd-core/tests/node_graph.rs
- crates/agentctl/** (Task 10: the `flow validate` CLI surface over `from_ast`)
- workflows/*.dot (Task 10: shipped sample workflow)

### Forbidden

- Do not fail on the first violation; collect and report all.
- Do not accept malformed Delphi contracts or unknown `converge_or_*` fallbacks.
- Do not treat `stack.manager_loop` as a known handler in P0.

## Completion Criteria

Scenario: A graph with no start node is rejected
  Test: node_graph_rejects_no_start
  Given a parsed graph with a terminal but no Mdiamond start node
  When NodeGraph::from_ast runs
  Then it returns an error mentioning a missing start node

Scenario: A graph with more than one start node is rejected
  Test: node_graph_rejects_multiple_starts
  Given a parsed graph with two Mdiamond start nodes
  When NodeGraph::from_ast runs
  Then it returns an error mentioning multiple start nodes (the engine drives from one entry point)

Scenario: An edge referencing an undeclared node is rejected
  Test: node_graph_rejects_undeclared_edge_endpoint
  Given a parsed graph whose edge names an endpoint that has no node declaration
  When NodeGraph::from_ast runs
  Then it returns an error naming the undeclared edge endpoint

Scenario: A duplicate node id is rejected
  Test: node_graph_rejects_duplicate_node_id
  Given a parsed graph that declares the same node id twice
  When NodeGraph::from_ast runs
  Then it returns an error naming the duplicate node id

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

Scenario: A well-formed Delphi fan_out/fan_in pair validates
  Test: node_graph_accepts_delphi_visibility_with_converge_aggregator
  Given a parsed graph whose fan_out declares visibility=delphi, max_rounds=3, and a paired fan_in aggregator=converge_or_majority_pass
  When NodeGraph::from_ast runs
  Then it succeeds

Scenario: delphi visibility without max_rounds is rejected
  Test: node_graph_rejects_delphi_visibility_without_max_rounds
  Given a parsed graph whose fan_out node declares visibility=delphi without max_rounds
  When NodeGraph::from_ast runs
  Then it returns an error mentioning max_rounds

Scenario: delphi visibility with a one-shot aggregator is rejected
  Test: node_graph_rejects_delphi_visibility_with_non_converge_aggregator
  Given a parsed graph whose Delphi fan_out reaches fan_in aggregator=majority_pass
  When NodeGraph::from_ast runs
  Then it returns an error mentioning converge_or

Scenario: Unknown converge fallback is rejected
  Test: node_graph_rejects_unknown_converge_fallback
  Given a parsed graph whose fan_in declares aggregator=converge_or_sideways
  When NodeGraph::from_ast runs
  Then it returns an error mentioning sideways

Scenario: max_rounds greater than one requires Delphi visibility
  Test: node_graph_rejects_non_delphi_max_rounds_above_one
  Given a parsed graph whose fan_out declares visibility=blind and max_rounds=3
  When NodeGraph::from_ast runs
  Then it returns an error mentioning visibility=delphi

Scenario: findings_diff convergence is accepted for Delphi
  Test: node_graph_accepts_delphi_findings_diff_convergence
  Given a parsed graph whose Delphi fan_out declares convergence=findings_diff<0.1
  When NodeGraph::from_ast runs
  Then it returns Ok

Scenario: malformed findings_diff convergence is rejected
  Test: node_graph_rejects_malformed_delphi_findings_diff_convergence
  Given a parsed graph whose Delphi fan_out declares convergence=findings_diff<sideways>
  When NodeGraph::from_ast runs
  Then it returns an error mentioning findings_diff

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

Scenario: agentctl flow validate succeeds on a valid .dot
  Test: agentctl_flow_validate_succeeds_on_valid_dot
  Given the built agentctl binary and a valid workflow .dot file
  When it is invoked with flow validate on that file
  Then the exit code is zero

Scenario: agentctl flow validate fails on an invalid .dot with exit code 2
  Test: agentctl_flow_validate_fails_on_invalid_dot_with_exit_2
  Given the built agentctl binary and a .dot file that fails validation
  When it is invoked with flow validate on that file
  Then the exit code is 2

Scenario: agentctl flow validate lists all violations on stderr
  Test: agentctl_flow_validate_lists_all_violations_in_stderr
  Given a .dot file with multiple validation violations
  When agentctl flow validate runs on it
  Then standard error mentions each violation
