spec: task
name: "agentd real Claude stdio smoke"
tags: [smoke, mcp]
---

## Intent

Prove that a real authenticated Claude Code process can drive the agentd MCP
stdio server named `agentd` and call `submit_outcome` for run
`real-claude-smoke-20260707064124`. The observable operator evidence is that
the run record leaves `current_node=propose_spec` after the agent submits an
outcome through `tools/call submit_outcome`.

## Decisions

- Transport: line-delimited JSON-RPC 2.0 over stdin/stdout via the `mcp-stdio` subcommand
- Tool under test: `submit_outcome` with arguments `run_id`, `node_id`, `attempt`, `status`
- Success status literal: `success`
- Run state is read from the run's SQLite database `agentd.db`

## Boundaries

### Allowed Changes
- .agentd/run/draft.spec.md

### Forbidden
- Do not modify the agentd daemon source under crates/**
- Do not edit the workflow definitions under workflows/**

## Out of Scope

- The execute.dot workflow and later lifecycle stages
- Human review and freeze of the draft spec
- MCP tools other than `submit_outcome`

## Completion Criteria

Scenario: submit_outcome advances the parked node
  Test: test_submit_outcome_success_advances_run
  Given the run "real-claude-smoke-20260707064124" is parked at node "propose_spec"
  When the agent calls `submit_outcome` over the `mcp-stdio` transport with status "success" and attempt 1
  Then the tool result reports the outcome as accepted
  And the run record in `agentd.db` no longer has current_node equal to "propose_spec"

Scenario: unknown run id is rejected
  Test: test_submit_outcome_rejects_unknown_run
  Given no run named "no-such-run" exists in `agentd.db`
  When the agent calls `submit_outcome` with run_id "no-such-run"
  Then the tool result is an error
  And the run record for "real-claude-smoke-20260707064124" is unchanged

Scenario: invalid status literal is rejected
  Test: test_submit_outcome_rejects_invalid_status
  Given the run "real-claude-smoke-20260707064124" is parked at node "propose_spec"
  When the agent calls `submit_outcome` with status "bogus"
  Then the tool result is an error
  And the run record in `agentd.db` keeps current_node equal to "propose_spec"
