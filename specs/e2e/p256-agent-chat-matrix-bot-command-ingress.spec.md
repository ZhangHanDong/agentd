spec: task
name: "agent-chat Matrix bot command ingress classification"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, bot-commands, phase-g, p256]
---

## Intent

Connect the p255 Matrix bot command planner to the SDK-facing Matrix client
transport so command messages stop flowing into agentd as ordinary agent
messages. This slice creates a fake-tested ingress classification boundary:
operator commands are captured as command plans, normal human messages continue
to forward through the existing backend contract, and command execution remains
out of scope.

## Decisions

- Extend `MatrixClientTextMessage` with optional `formatted_body` so Matrix
  mention-pill command prefixes can be classified after SDK timeline parsing.
- Extend `MatrixClientTransportConfig` with a `MatrixBotCommandAcl` and wire
  `agentd matrix-client-bridge-service` / `matrix-client-bridge-preflight`
  CLI options for repeatable operator and admin MXIDs.
- During `MatrixClientBridgeTransport::ensure_synced`, first apply the existing
  loop/ignored-sender suppression, then classify the remaining text events with
  `plan_matrix_bot_command`.
- Bang commands and Matrix mention-pill commands are stored as bot command
  plans and omitted from `inbound_events()`, matching agent-chat's
  command-before-routing behavior.
- Normal non-command events in mapped group or agent DM rooms remain forwarded
  as `MatrixInboundEvent`; non-command bot-DM fallback execution stays out of
  scope until bot command execution exists.
- Provide a `bot_command_plans()` accessor on `MatrixClientBridgeTransport` so
  the future execution slice can consume planned commands without re-syncing.
- Keep command execution, backend mutation, Matrix replies, tmux/agentctl
  control, room lifecycle changes, media transfer, service packaging, cutover,
  rollback, and dashboard rendering out of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p256-agent-chat-matrix-bot-command-ingress.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/client_transport.rs
- crates/agentd-matrix/tests/client_bridge_once.rs
- crates/agentd-matrix/tests/sdk_adapter.rs
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/matrix_bridge.rs
- crates/agentd-bin/tests/matrix_client_bridge_preflight.rs
- crates/agentd-bin/tests/matrix_client_bridge_service.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not start a real agentd daemon in tests.
- Do not execute bot commands against the backend, tmux, Matrix rooms, or
  agent runtimes.
- Do not send Matrix command replies or fallback replies.
- Do not add new Rust dependencies.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not enable `matrix-sdk-adapter` by default.
- Do not add Matrix media transfer, room lifecycle execution, service
  packaging, cutover, rollback, dashboard rendering, token rotation, or Matrix
  profile/avatar sync.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

Bot command execution, Matrix command replies, non-command bot-DM fallback
execution, backend API mutation for commands, room creation/deletion, group
membership mutation, agent DM creation, identity writes, spy rooms, agentctl/tmux
control, real Matrix homeserver validation, media transfer, service packaging,
operator cutover, rollback automation, dashboard rendering, Matrix
profile/avatar updates, token rotation, and remote relay service packaging.

## Completion Criteria

<!-- lint-ack: decision-coverage - p256 binds formatted_body preservation, CLI ACL options, loop suppression before classification, command omission from inbound forwarding, command-plan access, normal message preservation, and partial parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify Rust return values, CLI parsed args/config fields, SDK raw-event parsing, and repository Markdown/source state without a Matrix homeserver or daemon. -->
<!-- lint-ack: error-path - p256 covers ACL denied command planning and loop-suppressed command omission paths. -->
<!-- lint-ack: boundary-entry-point - p256 touches library transport, SDK parsing, CLI args/config, and docs/tests; scenarios reference each entry point. -->

Scenario: client transport separates Matrix bot commands from normal inbound events
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_separates_bot_commands_from_inbound_events
  Level: unit
  Test Double: fake Matrix client
  Given a sync snapshot with a joined group room
  And one human `!status` command event from a configured operator
  And one normal `please review` event from the same sender
  When `MatrixClientBridgeTransport` syncs the fake client
  Then `inbound_events()` returns only the normal event
  And `bot_command_plans()` returns one command plan for `!status`
  And the command plan preserves room id, event id, sender MXID, group context, and operator authorization

Scenario: client transport strips Matrix mention command prefix during ingress classification
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_classifies_formatted_mention_commands
  Level: unit
  Test Double: fake Matrix client
  Given a sync snapshot with a plain body `Agent Bridge: !dm codex-worker`
  And a formatted body whose leading Matrix mention pill is followed by `: !dm codex-worker`
  When `MatrixClientBridgeTransport` classifies inbound events
  Then `inbound_events()` is empty
  And `bot_command_plans()` contains command `!dm` with argument `codex-worker`
  And the command tier is operator management

Scenario: client transport suppresses loop command events before planning
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_suppresses_loop_commands_before_planning
  Level: unit
  Test Double: fake Matrix client
  Given command-shaped text events from the Matrix bot, a known puppet user, an ignored sender, and a human non-operator
  When `MatrixClientBridgeTransport` syncs the fake client
  Then loop and ignored-sender events produce no inbound event and no bot command plan
  And the human command produces one bot command plan with `operator_required` authorization
  And no command-shaped event is forwarded as a normal inbound event

Scenario: SDK timeline parser preserves formatted bodies for command classification
  Test:
    Package: agentd-matrix
    Filter: sdk_timeline_parser_preserves_formatted_body_for_bot_command_ingress
  Level: unit
  Test Double: raw Matrix sync JSON
  Given a raw `m.room.message` timeline event with `format` set to `org.matrix.custom.html`
  And a `formatted_body` that starts with a Matrix mention pill followed by `: !status`
  When SDK timeline parsing normalizes the event
  Then the resulting `MatrixClientTextMessage` preserves the plain `body`
  And the resulting `MatrixClientTextMessage` preserves the `formatted_body`

Scenario: CLI service and preflight pass Matrix bot ACL options into transport config
  Test:
    Package: agentd-bin
    Filter: agentd_cli_matrix_client_bridge_service_accepts_bot_command_acl_options
  Level: CLI unit
  Test Double: clap parser and config builder
  Given `agentd matrix-client-bridge-service` is invoked with repeatable `--matrix-operator` and `--matrix-admin` MXIDs
  When CLI args are parsed and service config is built
  Then the service args preserve operator and admin MXIDs
  And the preflight command reuses the same args
  And the resulting `MatrixClientTransportConfig.bot_command_acl` contains those operator and admin MXIDs

Scenario: parity docs record p256 bot command ingress classification without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p256_matrix_bot_command_ingress_progress
  Level: artifact inspection
  Test Double: repository Markdown files and Rust source text
  Given `crates/agentctl/tests/parity_cli.rs` inspects the agent-chat replacement parity map, roadmap, and Matrix source files
  When p256 progress is inspected
  Then the Matrix bridge row mentions p256 and bot command ingress classification
  And the Matrix bridge row remains partial
  And the row still names bot command execution, Matrix media, service packaging, cutover, rollback, token rotation, bridge operations, and dashboard/operator visibility gaps
  And the roadmap mentions p256, `--matrix-operator`, `--matrix-admin`, and command omission from inbound forwarding
  And Matrix source mentions `bot_command_plans`, `formatted_body`, `MatrixBotCommandAcl`, and `matrix_operator_mxids`
