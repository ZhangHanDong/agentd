spec: task
name: "Production resume gates changed workflow sha"
tags: [e2e, p1, recovery, checkpoint, daemon]
---

## Intent

The P0.9 deployment checklist still notes that `--accept-workflow-change` must
be wired into the real resume path after boot-resume lands. Core already has the
`Checkpoint::resume_guard` policy, but production event delivery rebuilds the
current graph from disk and resumes a checkpointed run without enforcing that
policy. This slice wires the existing sha guard into production resume: changed
workflow content is rejected by default, and only an operator-started daemon with
an explicit flag may resume across the change.

## Decisions

- `Engine::deliver_event` enforces `Checkpoint::resume_guard` before invoking
  the parked handler.
- The default production behavior is conservative: changed workflow sha returns
  `WorkflowShaChanged` and does not record a node outcome or emit a terminal
  event.
- `agentd --accept-workflow-change` is the operator gate for allowing resume
  across changed workflow content.
- `ProductionRunHost` carries the operator policy into every per-call `Engine`,
  including the HTTP daemon and `mcp-stdio` construction paths.
- Keep the existing checkpoint/run/event storage shapes unchanged.
- Update the deployment checklist so the workflow-change resume flag is no
  longer described as an unwired future task after P140.

## Boundaries

### Allowed Changes

- specs/e2e/p140-resume-workflow-change-gate.spec.md
- docs/p0.9-deployment-checklist.md
- crates/agentd-core/src/engine/execute.rs
- crates/agentd-core/tests/engine_execute.rs
- crates/agentd-bin/src/agent_mcp_context.rs
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/daemon.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/agent_mcp_context.rs
- crates/agentd-bin/tests/recovery.rs
- crates/agentd-bin/tests/deployment_checklist.rs

### Forbidden

- Do not add, remove, or rename database columns or tables.
- Do not change HTTP, MCP, `submit_outcome`, `submit_review`, or `assign_task`
  JSON shapes.
- Do not change workflow `.dot` files.
- Do not start real tmux, Claude, GitHub, or external-agent smoke runs.

## Out of Scope

- A real SIGKILL drill or real agent smoke execution.
- Adding an HTTP request field or MCP tool argument for per-event acceptance.
- Changing `Checkpoint::resume_guard` semantics.
- Changing how workflow sha is computed.

## Completion Criteria

Scenario: production deliver rejects changed workflow sha by default
  Test:
    Package: agentd-bin
    Filter: production_deliver_rejects_changed_workflow_sha_by_default
  Level: production host contract
  Test Double: real SqliteStore on tempfile with fake ports
  Given a run parked with a checkpoint written from one workflow sha
  And the workflow file content changes before the parked event is delivered
  When the event is delivered through `ProductionRunHost`
  Then delivery returns `WorkflowShaChanged`
  And the parked node outcome is not recorded
  And no terminal event is emitted

Scenario: production deliver allows changed workflow sha with operator flag
  Test:
    Package: agentd-bin
    Filter: production_deliver_allows_changed_workflow_sha_with_operator_accept
  Level: production host contract
  Test Double: real SqliteStore on tempfile with fake ports
  Given the same changed workflow sha situation
  When the host is built with the operator accept policy enabled
  Then the parked event resumes and the run reaches Finished
  And the parked node outcome is recorded once

Scenario: daemon CLI exposes operator accept flag
  Test:
    Package: agentd-bin
    Filter: agentd_cli_accepts_accept_workflow_change_flag
  Level: CLI contract
  Test Double: clap parser
  Given `agentd --accept-workflow-change`
  When CLI args are parsed
  Then `DaemonConfig.accept_workflow_change` is true
  And the default daemon config keeps it false

Scenario: mcp-stdio command inherits operator accept flag
  Test:
    Package: agentd-bin
    Filter: mcp_stdio_command_includes_accept_workflow_change_when_enabled
  Level: spawned-agent context contract
  Test Double: pure command renderer
  Given a daemon config with `accept_workflow_change=true`
  When the spawned-agent MCP stdio command is rendered
  Then the command includes `--accept-workflow-change` before `mcp-stdio`
  And a default daemon config omits the flag

Scenario: deployment checklist marks workflow-change gate wired
  Test:
    Package: agentd-bin
    Filter: deployment_checklist_marks_p140_workflow_change_gate_wired
  Level: docs regression
  Test Double: source inspection
  Given docs/p0.9-deployment-checklist.md and the P140 spec
  When the kill-9 workflow-change item is inspected
  Then the checklist names P140 as the wired operator gate
  And it no longer says to wire `--accept-workflow-change` when boot-resume lands
