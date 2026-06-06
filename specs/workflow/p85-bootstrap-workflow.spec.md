spec: task
name: "Bootstrap workflow — derive a spec from an existing codebase (§7.3 P1.5)"
tags: [workflow, dot, p1, bootstrap, agent-spec]
---

## Intent

Ship a `bootstrap.dot` template: the brownfield entry point — point agentd at an
existing codebase and get a starter spec out, so a repo with no specs can adopt
the agent-spec/agentd loop. §7.3 P1.5 named `agent-spec discover --from-codebase`
for this, but that subcommand does NOT exist in the shipped agent-spec (0.2.7) —
it is a "Phase 9" future feature. Rather than ship a workflow whose tool node
fails today (`unrecognized subcommand`), the AGENT performs the from-codebase
discovery — an LLM reading the code and writing the spec, which is more faithful
than a CLI heuristic anyway — scaffolded and quality-checked by the agent-spec
commands that DO exist (`init`, `lint`).

`start → scaffold (agent-spec init) → discover (agent reads code, writes the
spec) → lint (agent-spec lint) → report → done`. Parks once, at `discover`.

## Decisions

- The graph conforms to the frozen DOT grammar (passes `agentctl flow validate`,
  so check.sh's dot-validate covers it) and composes only existing handlers
  (`tool` / `codergen`) — no new grammar, no core change (D1).
- `scaffold` is `tool` running `agent-spec init --level task --name bootstrap`,
  which CREATES `bootstrap.spec.md` (verified — `init` writes a file, not
  stdout). `discover` is `codergen` (`role="spec-writer"`): the agent reads the
  codebase and fills that spec — this is the from-codebase discovery. `lint` is
  `tool` running `agent-spec lint bootstrap.spec.md --min-score 0.7`.
- LINEAR (no `goal_gate`, like draft.dot): `lint` is ADVISORY — a low-quality
  bootstrap still completes and surfaces, rather than blocking the terminal; a
  human iterates. No recovery edge.
- `cmd=` strings are STATIC whitespace-split argv over the standalone convention
  (the frozen tool handler does no `${...}`/cwd/env); `init`/`lint` operate on
  `bootstrap.spec.md` in the run cwd, `report` on `.agentd/run/report.md`.
- WIRED to be launchable (not an inert file): the agentctl `Flow` value-enum
  gains a `Bootstrap` variant and the daemon's `flow_to_file` gains a `bootstrap`
  arm, sharing one wire string — the same flow-triple the §7.3 P1.7 round-trip
  tests (`flow_to_file_resolves_every_shipped_flow`,
  `cli_flow_variants_map_to_existing_files`) guard, now extended to cover it.
- Proven by a walk-test on the REAL `Engine` over in-memory fakes (one codergen
  park at `discover`; the tool nodes default to exit-0): the run reaches
  `Finished`.

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
- Do not reference `agent-spec discover` (unshipped) in the shipped workflow.

## Out of Scope

- The literal `agent-spec discover --from-codebase` integration — revisit when
  agent-spec ships it (Phase 9); the agent-driven discovery stands in until then.
- Seeding the agent's codebase context (`initial_prompt_includes`) — the MVP
  real-env gap shared with draft/execute; the walk-test uses fake ports.
- Real agent/tmux execution (deployment).

## Completion Criteria

Scenario: bootstrap.dot validates
  Test: bootstrap_dot_validates
  Given the authored workflows/bootstrap.dot
  When it is parsed and built with NodeGraph::from_ast
  Then it returns Ok with no validation violations

Scenario: bootstrap.dot walks to done
  Test: bootstrap_dot_walks_to_done
  Given the bootstrap.dot graph on the real Engine with in-memory fake ports
  When scaffold succeeds, the discover agent submits success, and lint/report succeed
  Then the run reaches Finished

Scenario: the bootstrap flow is wired and launchable
  Test: flow_to_file_resolves_every_shipped_flow
  Given the daemon's flow_to_file mapping including the bootstrap flow
  When flow_to_file is called with the "bootstrap" flow name
  Then it resolves to bootstrap.dot, which exists under workflows
