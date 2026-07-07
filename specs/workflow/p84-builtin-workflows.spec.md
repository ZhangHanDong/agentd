spec: task
name: "More built-in workflows — spike / docs-only / bugfix-rapid / refactor-only (§7.3 P1.7)"
tags: [workflow, dot, p1, templates]
---

## Intent

Ship four more shipped `.dot` templates beyond draft/execute, covering the
common shapes a v1 user reaches for. Pure-additive: new workflow files + their
validation/walk coverage, reusing the frozen agentd-core handler vocabulary
(`tool` / `codergen` / `parallel.fan_out` / `parallel.fan_in`, `goal_gate`,
`Mdiamond`/`Msquare`). No core, engine, or spawn-path change — so this pack lands
cleanly under D1 (unlike the core-entangled packs).

The four differ by how much process each imposes:

- **spike** — exploratory throwaway; goal is learning, so NO gate, NO review, NO
  PR. `start → explore (agent) → report → done`.
- **docs-only** — a docs change; no code lifecycle gate, no adversarial review,
  so LINEAR like draft.dot. `start → write_docs (agent) → publish_branch →
  open_pr → report → done`. (No lint node — a docs change produces no spec draft
  to lint.)
- **bugfix-rapid** — a fast fix; KEEPS the `verify_lifecycle` gate but SKIPS the
  fan-out review. One `goal_gate` ⇒ a `goal_gate_unmet` recovery edge.
- **refactor-only** — behavior-preserving; no upstream spec drafting, but KEEPS
  both `verify_lifecycle` AND the fan-out `review` (drift is the risk). Two
  `goal_gate`s ⇒ a `goal_gate_unmet` recovery edge; the single fan_out pairs the
  single fan_in.

## Decisions

- Each workflow conforms to the frozen DOT grammar and passes `agentctl flow
  validate` (so check.sh's `dot-validate` step covers them automatically).
- `goal_gate` workflows (bugfix-rapid, refactor-only) carry exactly one
  `[label="goal_gate_unmet"]` recovery edge from the node that routes to `done`
  back to a NON-terminal (`implement`) — the same global-gate discipline as
  execute.dot (validate does NOT catch a missing recovery edge; the walk-test
  does, by reaching `done` rather than going Stuck).
- Non-gated workflows (spike, docs-only) route to the terminal unconditionally
  once their tool nodes succeed (linear, like draft.dot — no recovery edge).
- `cmd=` strings are whitespace-split argv over the standalone `.agentd/run/*`
  convention. As of P102, the shipped PR workflows (`docs-only`,
  `bugfix-rapid`, `refactor-only`) publish the allocated `${worktree}` via
  `scripts/agentd_publish_worktree.sh ${worktree} ${task_run_id}` before
  `open_pr`, and `open_pr` shells
  `scripts/agentd_open_pr.sh ${task_run_id}`. `gh` auth still comes from the
  operator's ambient environment (standalone, D6).
- Each is proven by a walk-test on the REAL `Engine` over in-memory fakes
  (mirroring p80/p81): the agent park(s) submit success, tool nodes succeed, and
  the run reaches `Finished`.
- The four are WIRED to be launchable (else they are inert files): the agentctl
  `Flow` value-enum gains a variant per workflow, and the daemon's
  `start_workflow` flow→file mapping gains a matching arm. The flow string is
  identical across the clap-derived kebab value, `Flow::name()`, and the
  `start_workflow` arm; a round-trip test guards that triple (a `name()` of
  `bugfix_rapid` vs an arm expecting `bugfix-rapid` is a runtime `unknown flow`
  the compiler never catches). `start_workflow`'s match is extracted to a pure
  `flow_to_file` so the mapping is unit-testable without starting a run.

## Boundaries

### Allowed Changes

- workflows/**
- crates/agentctl/** (the `Flow` enum + its tests)
- crates/agentd-bin/** (the `start_workflow` flow→file mapping + its tests)
- crates/agentctl/tests/**
- specs/workflow/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not add new handlers/grammar — compose only the existing vocabulary.

## Out of Scope

- Real agent/tmux/PR execution (deployment); the walk-tests use fake ports.

## Completion Criteria

Scenario: spike.dot validates
  Test: spike_dot_validates
  Given the authored workflows/spike.dot
  When it is parsed and built with NodeGraph::from_ast
  Then it returns Ok with no validation violations

Scenario: spike.dot walks to done
  Test: spike_dot_walks_to_done
  Given the spike.dot graph on the real Engine with in-memory fake ports
  When the explore agent submits success and the report tool succeeds
  Then the run reaches Finished

Scenario: docs-only.dot validates
  Test: docs_only_dot_validates
  Given the authored workflows/docs-only.dot
  When it is parsed and built with NodeGraph::from_ast
  Then it returns Ok with no validation violations

Scenario: docs-only.dot walks to done
  Test: docs_only_dot_walks_to_done
  Given the docs-only.dot graph on the real Engine with in-memory fake ports
  When the write_docs agent submits success and the publish/open_pr/report tools succeed
  Then the run reaches Finished

Scenario: bugfix-rapid.dot validates
  Test: bugfix_rapid_dot_validates
  Given the authored workflows/bugfix-rapid.dot
  When it is parsed and built with NodeGraph::from_ast
  Then it returns Ok with no validation violations

Scenario: bugfix-rapid.dot has a goal_gate_unmet recovery edge to a non-terminal
  Test: bugfix_rapid_dot_has_goal_gate_unmet_recovery_edge
  Given the validated bugfix-rapid.dot graph
  When the edge labelled "goal_gate_unmet" is located
  Then exactly one such edge exists and its target is not a terminal node

Scenario: bugfix-rapid.dot walks to done with the gate satisfied
  Test: bugfix_rapid_dot_walks_to_done
  Given the bugfix-rapid.dot graph on the real Engine with in-memory fake ports
  When implement succeeds and verify_lifecycle/publish/open_pr/report succeed so the goal_gate is met
  Then the run reaches Finished

Scenario: refactor-only.dot validates
  Test: refactor_only_dot_validates
  Given the authored workflows/refactor-only.dot
  When it is parsed and built with NodeGraph::from_ast
  Then it returns Ok with no validation violations

Scenario: refactor-only.dot has a goal_gate_unmet recovery edge to a non-terminal
  Test: refactor_only_dot_has_goal_gate_unmet_recovery_edge
  Given the validated refactor-only.dot graph
  When the edge labelled "goal_gate_unmet" is located
  Then exactly one such edge exists and its target is not a terminal node

Scenario: refactor-only.dot walks to done with both gates satisfied
  Test: refactor_only_dot_walks_to_done
  Given the refactor-only.dot graph on the real Engine with in-memory fake ports
  When implement succeeds, the three reviewers pass, and the tool nodes succeed
  Then the run reaches Finished

Scenario: every shipped flow name maps to an existing workflow file
  Test: flow_to_file_resolves_every_shipped_flow
  Given the daemon's flow_to_file mapping and the workflows directory
  When flow_to_file is called with each shipped flow name (draft, execute, spike, docs-only, bugfix-rapid, refactor-only)
  Then each resolves to a workflow file that exists, and an unknown flow resolves to none

Scenario: every CLI Flow variant names a real file with a consistent wire name
  Test: cli_flow_variants_map_to_existing_files
  Given the agentctl Flow value-enum
  When each variant's file_name and name are checked
  Then file_name names a file that exists in workflows and name equals the file stem
