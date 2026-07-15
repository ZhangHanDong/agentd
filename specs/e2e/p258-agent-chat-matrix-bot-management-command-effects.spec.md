spec: task
name: "agent-chat Matrix bot management command effects"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, bot-commands, phase-g, p258]
---

## Intent

Continue the agent-chat replacement Matrix bridge work after p257 by executing
the first bounded management-command slice for `!dm` and `!identity`. This
creates the explicit side-effect ports needed for agent-chat-compatible bot
management behavior while keeping real Matrix room creation, daemon identity
persistence semantics, tmux control, agent launch, and real homeserver evidence
out of scope for this slice.

## Decisions

- Add a deterministic management-command execution path in `agentd-matrix` for
  authorized `!dm <agent>` and `!identity [agent] <text>` commands.
- Keep the p257 side-effect-free `execute_matrix_bot_command` behavior intact
  for read-only commands and unsupported management/admin commands.
- Execute `!dm` through effect ports: first verify the target agent exists, then
  request a human-agent DM room for the command sender localpart, and then send
  an agent-chat-compatible reply for invited, already-joined, invite-failed, and
  no-room outcomes.
- Execute `!identity` through effect ports: infer the target agent from direct
  room context when the first argument is not a known agent name, otherwise use
  the explicit first argument; update the selected agent identity text through
  the backend effect port; and reply with success or `Failed: <error>`.
- Surface side effects through `MatrixBotCommandExecution.side_effects`:
  `!dm` declares `ChangesMatrixRooms`, and successful or failed `!identity`
  update attempts declare `MutatesBackend`.
- Wire SDK-facing `MatrixClientBridgeTransport` command reply execution through
  the management effect ports while still omitting command-shaped events from
  ordinary `inbound_events()`.
- Extend `AgentdHttpBackend` with the HTTP request shape needed by the
  management backend port: `GET /api/agents/:name` for lookup and
  `PATCH /api/agents/:name` with `{ "identity": ... }` for identity update.
- Keep default tests fake/local only; do not use real Claude, real Matrix
  homeservers, real daemon supervision, or real execute smoke.

## Boundaries

### Allowed Changes
- specs/e2e/p258-agent-chat-matrix-bot-management-command-effects.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/bot_commands.rs
- crates/agentd-matrix/tests/client_transport.rs
- crates/agentd-matrix/tests/client_bridge_once.rs
- crates/agentd-matrix/tests/http_backend.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden
- Do not implement full Matrix room lifecycle parity or real SDK room creation
  in this slice.
- Do not implement or execute `!mkgroup`, `!addmember`, `!rmember`,
  `!joingroup`, `!rmgroup`, `!spy`, `!agentctl`, or `!ctl` side effects.
- Do not add new cargo dependencies.
- Do not contact a real Matrix homeserver or start real Claude/agent runtimes in
  tests.
- Do not forward command-shaped Matrix events as ordinary inbound messages.
- Do not hide remaining Matrix replacement gaps in parity documentation.

## Completion Criteria

Scenario: executor runs `!dm` through effect ports
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_with_effects_executes_dm_invite_request
  Given an authorized `!dm codex-worker` command from `@alex:matrix.test`
  And a fake effect port that finds `codex-worker` and returns an invited DM room
  When the management command executor runs
  Then the effect port records an agent lookup for `codex-worker`
  And it records an `ensure_human_dm_room` request for agent `codex-worker` and human `alex`
  And the reply tells the operator that the DM room is ready and the invite was sent
  And the execution declares the `ChangesMatrixRooms` side effect

Scenario: executor runs `!identity` through direct-room and explicit-agent forms
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_with_effects_updates_identity_from_context_or_args
  Given a command snapshot containing `codex-worker` and `codex-reviewer`
  And a direct-room context for `codex-worker`
  When authorized `!identity Be concise` and `!identity codex-reviewer Review carefully` commands are executed
  Then the first command updates identity text for `codex-worker`
  And the second command updates identity text for `codex-reviewer`
  And both replies contain `Identity set for`
  And both executions declare the `MutatesBackend` side effect

Scenario: executor rejects malformed or failed management commands without extra effects
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_with_effects_handles_management_errors
  Given authorized management commands with missing arguments, unknown agents, and backend failure results
  When the management command executor runs
  Then `!dm` without an agent replies with `Usage: !dm <agent>` and performs no lookup
  And `!dm ghost` replies with `Agent not found: ghost` without requesting a room
  And `!identity` without enough context replies with the agent-chat-compatible usage text
  And an identity backend failure replies with `Failed:`

Scenario: legacy read-only executor remains side-effect free for unsupported management commands
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_answers_unknown_and_unsupported_commands_without_mutation
  Given the p257 side-effect-free executor
  When an authorized management command such as `!dm` is executed through that legacy entry point
  Then the reply still states that the command is not implemented in the agentd Matrix bridge yet
  And the execution declares no side effects

Scenario: SDK client transport executes management commands through effect ports
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_executes_management_commands_through_effect_ports
  Given a fake Matrix sync with `!dm codex-worker`, `!identity Be concise`, and one normal text event
  And a fake backend effect port for agent lookup and identity update
  When `MatrixClientBridgeTransport` executes bot command replies through management effects
  Then the fake Matrix client records an `ensure_human_dm_room` request
  And the fake backend records the identity update
  And the fake Matrix client sends two command replies
  And `inbound_events()` still returns only the normal text event

Scenario: HTTP backend exposes management command effect request shapes
  Test:
    Package: agentd-matrix
    Filter: agentd_http_backend_executes_bot_management_effect_requests
  Level: adapter integration
  Test Double: fake agentd HTTP server
  Given a fake agentd HTTP server for agent lookup and identity update
  When `AgentdHttpBackend` runs the management backend effect methods
  Then it sends authenticated `GET /api/agents/codex-worker`
  And it sends authenticated `PATCH /api/agents/codex-worker` with an `identity` JSON field
  And it maps a missing agent response to `None` instead of a hard command failure

Scenario: matrix client bridge one-shot executes management command effects
  Test:
    Package: agentd-matrix
    Filter: matrix_client_bridge_once_executes_management_command_effects_and_reports_count
  Level: bridge assembly integration
  Test Double: fake SDK client and fake agentd HTTP server
  Given a fake SDK client sync with one `!dm codex-worker` command
  And a fake agentd HTTP backend for snapshot, agent lookup, room registration, and outbox polling
  When `run_matrix_client_bridge_once` runs
  Then it requests the management command effects before normal inbound forwarding
  And it sends one bot command reply through the fake Matrix client
  And `BridgeRunReport.bot_command_replies_sent` is `1`

Scenario: parity docs record p258 without declaring full Matrix replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p258_matrix_bot_management_effects_progress
  Given the agent-chat replacement parity map and roadmap
  When p258 progress is inspected
  Then the Matrix bridge row mentions p258 management command effects for `!dm` and `!identity`
  And the row remains partial
  And the row still names full room lifecycle parity, Matrix media, real homeserver evidence, service packaging, cutover, rollback, token rotation, bridge operations, and dashboard/operator visibility gaps
