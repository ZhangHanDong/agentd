spec: task
name: "Bootstrap workflow — derive a spec from an existing codebase (§7.3 P1.5)"
tags: [workflow, dot, p1, bootstrap, agent-spec]
---

## Intent

Ship a `bootstrap.dot` template: the brownfield entry point — point agentd at an
existing codebase and get a starter spec out, so a repo with no specs can adopt
the agent-spec/agentd loop. P112 supersedes the original P85 fallback now that
the installed `agent-spec` CLI ships `discover --from-codebase`: bootstrap uses
the real discover command instead of parking a spec-writer agent.

`start → discover_spec (agent-spec discover --from-codebase) → lint
(agent-spec lint) → report → done`. It no longer parks.

## Decisions

- The graph conforms to the frozen DOT grammar (passes `agentctl flow validate`,
  so check.sh's dot-validate covers it) and composes only the existing `tool`
  handler — no new grammar, no core change (D1).
- `discover_spec` is `tool` running `agent-spec discover --from-codebase --code .
  --name bootstrap --out bootstrap.spec.md`; `lint` is `tool` running
  `agent-spec lint bootstrap.spec.md --min-score 0.7`.
- LINEAR (no `goal_gate`, like draft.dot): `lint` is ADVISORY — a low-quality
  bootstrap still completes and surfaces, rather than blocking the terminal; a
  human iterates. Discovery itself must succeed before lint/report run. No
  recovery edge.
- `cmd=` strings are STATIC whitespace-split argv over the standalone convention
  (the frozen tool handler does no `${...}`/cwd/env); `discover`/`lint` operate
  on `bootstrap.spec.md` in the run cwd, `report` on `.agentd/run/report.md`.
- WIRED to be launchable (not an inert file): the agentctl `Flow` value-enum
  gains a `Bootstrap` variant and the daemon's `flow_to_file` gains a `bootstrap`
  arm, sharing one wire string — the same flow-triple the §7.3 P1.7 round-trip
  tests (`flow_to_file_resolves_every_shipped_flow`,
  `cli_flow_variants_map_to_existing_files`) guard, now extended to cover it.
- Proven by a walk-test on the REAL `Engine` over in-memory fakes: the all-tool
  run reaches `Finished` without an agent park.

## Boundaries

### Allowed Changes

- workflows/**
- crates/agentctl/** (the `Flow` enum + its tests)
- crates/agentd-bin/** (the `flow_to_file` mapping + its test)
- crates/agentctl/tests/**
- specs/workflow/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not add new handlers/grammar — compose only the existing vocabulary.
- Do not park a spec-writer agent for bootstrap discovery now that the CLI
  command exists.

## Out of Scope

- Real agent/tmux execution (deployment).

## Completion Criteria

Scenario: bootstrap.dot validates
  Test: bootstrap_dot_validates
  Given the authored workflows/bootstrap.dot
  When it is parsed and built with NodeGraph::from_ast
  Then it returns Ok with no validation violations

Scenario: bootstrap.dot walks to done
  Test: bootstrap_dot_walks_to_done_without_agent_park
  Given the bootstrap.dot graph on the real Engine with in-memory fake ports
  When discover_spec, lint, and report succeed
  Then the run reaches Finished

Scenario: the bootstrap flow is wired and launchable
  Test: flow_to_file_resolves_every_shipped_flow
  Given the daemon's flow_to_file mapping including the bootstrap flow
  When flow_to_file is called with the "bootstrap" flow name
  Then it resolves to bootstrap.dot, which exists under workflows
