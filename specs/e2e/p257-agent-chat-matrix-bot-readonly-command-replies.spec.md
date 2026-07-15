spec: task
name: "agent-chat Matrix bot read-only command replies"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, bot-commands, phase-g, p257]
---

## Intent

Continue the agent-chat replacement Matrix bridge work after p256 by executing the
safe read-only subset of Matrix bot commands and sending Matrix replies. The
first executable slice covers command replies that do not mutate agentd state,
create rooms, control tmux, or launch agents, so operators can use the bridge for
basic visibility before management commands are implemented.

## Decisions

- Implement a deterministic Matrix bot command executor in `agentd-matrix` for
  `!help`, `!status`, `!agents`, `!agents all`, `!groups`, denied commands, and
  unknown commands.
- Use agent-chat-compatible plain-text reply headings where practical:
  `=== Agent Bridge Bot Commands ===`, `=== System Status ===`,
  `=== Online Agents ===`, `=== All Agents ===`, and `=== Groups ===`.
- Add a read-only `AgentdHttpBackend` snapshot path that fetches `/api/agents`
  and `/api/groups` and maps their current JSON arrays into command summaries.
- Wire SDK-facing `MatrixClientBridgeTransport` command plans into Matrix replies
  through `MatrixClientPort::send_text_message`, while keeping command events out
  of normal `inbound_events()`.
- Count sent command replies in `BridgeRunReport.bot_command_replies_sent` and
  expose the aggregate in matrix client bridge service reporting.
- Keep unsupported management/admin commands side-effect-free in p257: no
  backend mutation, no Matrix room lifecycle changes, no tmux/agentctl control,
  no agent launch, and no real homeserver requirement.
- Keep default tests fake/local only; do not use real Claude, real Matrix
  homeservers, real daemon supervision, or real execute smoke.

## Boundaries

### Allowed Changes
- specs/e2e/p257-agent-chat-matrix-bot-readonly-command-replies.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/bot_commands.rs
- crates/agentd-matrix/tests/client_transport.rs
- crates/agentd-matrix/tests/client_bridge_once.rs
- crates/agentd-matrix/tests/http_backend.rs
- crates/agentd-bin/src/main.rs
- crates/agentd-bin/src/matrix_bridge.rs
- crates/agentd-bin/tests/matrix_client_bridge_service.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden
- Do not execute or partially simulate `!dm`, `!mkgroup`, `!addmember`,
  `!rmember`, `!joingroup`, `!identity`, `!rmgroup`, `!spy`, `!agentctl`, or
  `!ctl` side effects.
- Do not add new cargo dependencies.
- Do not contact a real Matrix homeserver or start real Claude/agent runtimes in
  tests.
- Do not forward command-shaped Matrix events as ordinary inbound messages.
- Do not hide remaining Matrix replacement gaps in parity documentation.

## Completion Criteria

Scenario: executor renders read-only command replies from a snapshot
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_replies_to_help_status_agents_and_groups
  Given a command snapshot with two agents and one group
  When authorized `!help`, `!status`, `!agents`, `!agents all`, and `!groups` commands are executed
  Then each execution returns one Matrix reply for the command room
  And the replies include agent-chat-compatible headings for help, status, online agents, all agents, and groups
  And the status reply reports the agent count, group count, unavailable tmux sessions, and running bridge

Scenario: executor rejects unauthorized commands without side effects
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_rejects_unauthorized_commands_without_side_effects
  Given a planned `!status` command whose authorization is `operator_required`
  When the command executor runs
  Then it returns one reply that says operator privileges are required
  And no backend snapshot mutation, room lifecycle change, tmux control, or agent launch is requested

Scenario: executor answers unknown and unsupported commands without mutation
  Test:
    Package: agentd-matrix
    Filter: matrix_bot_command_executor_answers_unknown_and_unsupported_commands_without_mutation
  Given an authorized unknown command and an authorized management command such as `!dm`
  When the command executor runs
  Then the unknown command reply contains `Unknown command` and `Send !help for available commands.`
  And the management command reply states that the command is not implemented in the agentd Matrix bridge yet
  And neither command asks the backend to mutate state or asks Matrix to create rooms

Scenario: SDK client transport sends command replies and keeps commands out of inbound forwarding
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_sends_bot_command_replies_without_forwarding_inbound
  Given a fake Matrix sync with one `!status` command event and one normal text event
  And a command snapshot with one online agent and one group
  When `MatrixClientBridgeTransport` executes bot command replies
  Then the fake Matrix client receives one `send_text_message` call in the command room
  And `bot_command_plans()` still exposes the command plan
  And `inbound_events()` still returns only the normal text event

Scenario: HTTP backend fetches the read-only command snapshot
  Test:
    Package: agentd-matrix
    Filter: agentd_http_backend_reads_bot_command_snapshot_from_agents_and_groups
  Given a fake agentd HTTP server that returns `/api/agents` and `/api/groups` JSON arrays
  When `AgentdHttpBackend` reads the Matrix bot command snapshot
  Then it sends authenticated `GET /api/agents` and `GET /api/groups` requests
  And it maps agent names, statuses, roles, capabilities, runtimes, and group members into the snapshot

Scenario: matrix client bridge one-shot executes command replies and reports the count
  Test:
    Package: agentd-matrix
    Filter: matrix_client_bridge_once_executes_bot_command_replies_and_reports_count
  Given a fake SDK client sync with one `!help` command
  And a fake agentd HTTP backend for agents, groups, room registration, and outbox polling
  When `run_matrix_client_bridge_once` runs
  Then it sends the help reply through the fake Matrix client
  And the command is not posted to `/api/matrix/inbound`
  And `BridgeRunReport.bot_command_replies_sent` is `1`

Scenario: service assembly aggregates command reply counts
  Test:
    Package: agentd-bin
    Filter: agentd_bin_matrix_client_bridge_service_reports_bot_command_reply_counts
  Given a bounded fake matrix client bridge service run with a command reply in one iteration
  When the service report is produced
  Then the iteration report includes one bot command reply
  And the aggregate service report keeps registered room, inbound, outbound, cursor, and bot command reply counts visible

Scenario: parity docs record p257 without declaring full Matrix replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p257_matrix_bot_readonly_command_replies_progress
  Given the agent-chat replacement parity map and roadmap
  When p257 progress is inspected
  Then the Matrix bridge row mentions p257 read-only bot command replies
  And the row remains partial
  And the row still names management command execution, room lifecycle parity, Matrix media, real homeserver evidence, service packaging, cutover, rollback, token rotation, bridge operations, and dashboard/operator visibility gaps
