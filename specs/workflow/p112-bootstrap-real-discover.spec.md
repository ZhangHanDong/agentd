spec: task
name: "Bootstrap workflow uses real agent-spec discover"
tags: [workflow, p1, bootstrap, agent-spec]
---

## Intent

Upgrade `bootstrap.dot` now that the local `agent-spec` CLI ships the
`discover` subcommand. The bootstrap flow should no longer park a spec-writer
agent to simulate from-codebase discovery; it should run the real
`agent-spec discover --from-codebase` tool and then lint the generated spec.

## Decisions

- `bootstrap.dot` is a linear all-tool workflow:
  `start -> discover_spec -> lint_spec -> report -> done`.
- `discover_spec` runs
  `agent-spec discover --from-codebase --code . --name bootstrap --out bootstrap.spec.md`.
- `lint_spec` keeps `agent-spec lint bootstrap.spec.md --min-score 0.7`.
- The `discover_spec -> lint_spec` edge is success-conditioned so a failed
  discovery does not run lint/report on a missing or stale generated spec.
- The flow remains launchable through the existing `bootstrap` `Flow` wiring and
  the existing daemon `flow_to_file` mapping.
- This replaces P85's agent-driven fallback because `agent-spec discover` is now
  available in the installed CLI.

## Boundaries

### Allowed Changes

- specs/workflow/p112-bootstrap-real-discover.spec.md
- specs/workflow/p85-bootstrap-workflow.spec.md
- workflows/bootstrap.dot
- crates/agentctl/tests/workflows.rs

### Forbidden

- Do not modify any file under crates/agentd-core/**.
- Do not add new handlers, graph syntax, store schema, or daemon routes.
- Do not reintroduce a `codergen`/spec-writer park in bootstrap.dot.

## Acceptance Criteria

Scenario: bootstrap.dot uses the real discover command
  Test: bootstrap_dot_uses_real_agent_spec_discover
  Level: workflow validation
  Test Double: parsed bootstrap.dot
  Given the authored workflows/bootstrap.dot
  When it is parsed and validated
  Then it contains a tool node whose command includes "agent-spec discover --from-codebase"
  And it contains no codergen node

Scenario: bootstrap.dot walks to done without an agent park
  Test: bootstrap_dot_walks_to_done_without_agent_park
  Level: core workflow integration
  Test Double: Engine + FakeBackend + RecordingCommandRunner + InMemoryStore
  Given bootstrap.dot on the real Engine with three successful tool outputs queued
  When the engine executes the workflow
  Then the run reaches Finished without parking
  And the first recorded command is "agent-spec discover --from-codebase --code . --name bootstrap --out bootstrap.spec.md"

Scenario: discover failure stops before lint
  Test: bootstrap_discover_failure_stops_before_lint
  Level: core workflow integration
  Test Double: Engine + RecordingCommandRunner + InMemoryStore
  Given bootstrap.dot on the real Engine with discover_spec returning exit status 2
  When the engine executes the workflow
  Then the run fails at discover_spec
  And lint_spec and report are not executed

Scenario: the bootstrap flow remains wired
  Test: flow_to_file_resolves_every_shipped_flow
  Level: daemon mapping
  Test Double: host flow mapping
  Given the daemon's flow_to_file mapping including the bootstrap flow
  When flow_to_file is called with the "bootstrap" flow name
  Then it resolves to bootstrap.dot, which exists under workflows
