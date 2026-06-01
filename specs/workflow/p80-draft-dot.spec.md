spec: task
name: "draft.dot — the standalone Path-B draft workflow"
tags: [workflow, dot, mvp, p0, path-b]
---

## Intent

Author `workflows/draft.dot`, the first of the two standalone Path-B workflows
(boundary Δ1): turn a local issue into a spec draft (issue → propose_spec →
lint → push draft). It must conform to agentd-core's frozen DOT grammar so
`agentctl flow validate` accepts it, and it parks exactly once — at the
`propose_spec` codergen node (the spec-writer agent). This task lands the file
and its validation/structure guarantees; the engine walk is p80's later
scenario (added with the walk-test task).

## Decisions

- `draft.dot` is a linear graph: `start`(Mdiamond) → `fetch_issue_context`(tool) → `propose_spec`(codergen, role=spec-writer) → `lint_spec`(tool) → `push_draft`(tool) → `done`(Msquare).
- Handler values are exactly the frozen `HandlerKind::parse` strings (`tool`, `codergen`); shapes are `Mdiamond` (the single start) and `Msquare` (the single terminal).
- Every `tool` `cmd=` is a static whitespace-split argv (no `${...}` substitution — the frozen `tool` handler does not implement it); standalone file paths are fixed conventions.
- The graph has exactly one start and one terminal and passes `NodeGraph::from_ast` with zero violations.

## Boundaries

### Allowed Changes

- workflows/draft.dot
- crates/agentctl/tests/**
- specs/workflow/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1 — the DOT grammar is frozen).
- Do not use `${...}` / `$run_dir` substitution in any `cmd=` (the frozen tool handler does whitespace-split argv only).

## Out of Scope

- Live execution / a production RunHost / a real agent (P0.9). The walk-test drives the real engine over in-memory fakes in-process; it does not spawn a real agent or socket.

## Completion Criteria

Scenario: draft.dot parses and validates with zero violations
  Test: draft_dot_validates
  Given the authored workflows/draft.dot
  When it is parsed and built with NodeGraph::from_ast
  Then it returns Ok with no validation violations

Scenario: draft.dot has exactly one start and one terminal
  Test: draft_dot_single_start_single_terminal
  Given the validated draft.dot graph
  When its start and terminal nodes are counted
  Then there is exactly one start node and exactly one terminal node

Scenario: a draft graph with an unknown handler is rejected
  Test: draft_dot_rejects_unknown_handler_variant
  Given a draft-shaped graph whose propose_spec handler is "stack.manager_loop"
  When it is built with NodeGraph::from_ast
  Then it returns Err naming the unknown handler

Scenario: a draft graph with no terminal is rejected
  Test: draft_dot_rejects_missing_terminal_variant
  Given a draft-shaped graph with no Msquare terminal node
  When it is built with NodeGraph::from_ast
  Then it returns Err reporting the missing terminal

Scenario: draft.dot parks once at propose_spec then walks to done
  Test: draft_dot_parks_at_propose_spec_then_finishes
  Given the draft.dot graph on the real Engine with in-memory fake ports
  When execute runs and the spec-writer's success outcome is delivered
  Then the run parks at propose_spec awaiting an agent outcome and then reaches Finished
