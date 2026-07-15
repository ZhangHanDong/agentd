spec: task
name: "agent-chat Matrix bridge process scaffold"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p237]
---

## Intent

Advance the agent-chat replacement path from a backend-facing Matrix HTTP
contract to a testable Matrix bridge runtime boundary. This slice gives
`agentd-matrix` the local process scaffold that a future Matrix SDK adapter can
plug into: it registers Matrix rooms, forwards inbound Matrix events to the
p236 backend contract, polls backend outbox events, sends outbound Matrix
messages through a transport, and persists the outbox cursor only after
successful sends.

This is not the real homeserver bridge yet. The runtime is intentionally tested
with fake backend and fake Matrix transport implementations so the slice remains
deterministic and does not touch real Matrix, Claude, tmux, remote relay, or the
real execute smoke path.

## Decisions

- Add `agentd-matrix` runtime types for bridge configuration, state, room
  registration, inbound Matrix events, outbound Matrix events, and per-iteration
  reports.
- Add an `AgentdBridgeBackend` trait representing the p236 backend contract:
  room registration, Matrix inbound ingress, and Matrix outbox polling.
- Add a `MatrixBridgeTransport` trait representing the Matrix-side adapter:
  observed room registrations, inbound Matrix events, and outbound sends.
- Add `BridgeRuntime::run_once` as the deterministic process-loop unit. It
  registers rooms first, forwards inbound events second, then polls the backend
  outbox from the stored cursor and sends outbound Matrix events.
- Advance the outbox cursor after each successfully sent outbox event. If a send
  fails, the cursor remains at the last confirmed sequence so the failed event
  can be retried without replaying already confirmed sends.
- Validate bridge configuration by requiring a non-empty agentd API base URL,
  trimming trailing slashes, preserving optional bearer-token configuration, and
  defaulting bridge state to cursor `0`.
- Keep `matrix_bridge` partial after this slice because agentd still does not
  include a real Matrix SDK process, homeserver login, puppet accounts,
  join/invite handling, Matrix media transfer, Matrix bot commands, service
  cutover, rollback, token provisioning/rotation, or bridge operations.

## Boundaries

### Allowed Changes

- specs/e2e/p237-agent-chat-matrix-bridge-process-scaffold.spec.md
- crates/agentd-matrix/Cargo.toml
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/bridge_runtime.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not add a real Matrix SDK adapter, Matrix homeserver dependency, real
  homeserver network test, puppet registration, Matrix media upload, long-running
  bridge daemon, systemd/launchd service, remote relay process, tmux injection
  worker, service cutover, rollback automation, token provisioning, or token
  rotation.
- Do not claim that agentd can fully replace agent-chat after this slice.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

- Matrix client-server API calls, account login/registration, room join/invite,
  puppet account lifecycle, Matrix media transfer, and real transport packaging.
- Bot command parsing through Matrix rooms.
- Full Matrix room membership reconciliation.
- Dashboard rendering for Matrix bridge rooms, runtime state, or outbox cursor.
- Real SSE soak tests, remote relay integration, launchd/systemd installation,
  and operator cutover automation.

## Completion Criteria

<!-- lint-ack: decision-coverage - p237 binds runtime configuration, backend/transport traits, run_once ordering, cursor advancement, failure retry semantics, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify fake backend calls, fake Matrix sends, cursor values, config output, and repository Markdown without a real Matrix homeserver or bridge process. -->
<!-- lint-ack: boundary-entry-point - agentd-matrix runtime and agentctl parity artifact entry points are verified through bound package filters. -->

Scenario: Matrix bridge runtime forwards room registrations and inbound events
  Test:
    Package: agentd-matrix
    Filter: matrix_bridge_runtime_forwards_room_registrations_and_inbound_events
  Level: library integration
  Test Double: fake backend and fake Matrix transport
  Given a `MatrixBridgeTransport` fake reports one trusted group room and two inbound events
  When `BridgeRuntime::run_once` runs one iteration
  Then the backend receives the room registration before both inbound events
  And the report records one registered room and two forwarded inbound events

Scenario: Matrix bridge runtime sends outbox events and advances cursor
  Test:
    Package: agentd-matrix
    Filter: matrix_bridge_runtime_sends_outbox_events_and_advances_cursor
  Level: library integration
  Test Double: fake backend and fake Matrix transport
  Given backend outbox polling from cursor 0 returns two outbound Matrix events
  When `BridgeRuntime::run_once` runs one iteration
  Then the Matrix transport sends both events in sequence order
  And the bridge state cursor advances to sequence 2
  And the report records two outbound sends

Scenario: Matrix bridge runtime keeps retry cursor on send failure
  Test:
    Package: agentd-matrix
    Filter: matrix_bridge_runtime_keeps_retry_cursor_on_send_failure
  Level: library integration
  Test Double: fake backend and fake Matrix transport
  Given backend outbox polling from cursor 0 returns two outbound Matrix events
  And the `MatrixBridgeTransport` fake fails while sending sequence 2
  When `BridgeRuntime::run_once` runs one iteration
  Then the operation returns a transport error
  And the bridge state cursor remains at sequence 1
  And only the first outbound event is recorded as sent

Scenario: Matrix bridge configuration validates API base and default state
  Test:
    Package: agentd-matrix
    Filter: matrix_bridge_config_validates_agentd_api_and_defaults_cursor
  Level: unit
  Test Double: none
  Given an agentd API URL with trailing slashes and an operator token
  When the bridge configuration is constructed
  Then the API URL is normalized without trailing slashes
  And the token is preserved
  And default bridge state starts at cursor 0
  And an empty API URL is rejected

Scenario: parity docs record p237 Matrix bridge scaffold progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p237_matrix_bridge_scaffold_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the Matrix bridge row and Phase G roadmap are inspected
  Then the Matrix bridge row mentions p237 and the `agentd-matrix` bridge
  runtime scaffold
  And the Matrix bridge row remains partial
  And the row still names the real Matrix SDK process, puppet, media, cutover,
  rollback, and token gaps that prevent full replacement
