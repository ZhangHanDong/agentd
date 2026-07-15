spec: task
name: "agent-chat Matrix client adapter boundary"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p240]
---

## Intent

Advance the agent-chat replacement path from a file-backed Matrix replay shell to
an SDK-facing Matrix client adapter boundary. This slice defines the normalized
client operations a future real Matrix SDK process must satisfy: login readiness,
invite join/leave, sync snapshots, inbound text normalization, outbound text
sends, trust gating, and loop suppression.

## Decisions

- Add a `MatrixClientPort` trait in `agentd-matrix` instead of adding a real
  Matrix SDK dependency in this slice.
- Add `MatrixClientBridgeTransport<C>` as the `MatrixBridgeTransport` adapter
  that adapts `MatrixClientPort` into the existing bridge runtime contract.
- `MatrixClientBridgeTransport` calls `ensure_logged_in` before the first sync,
  caches one sync snapshot per transport instance, and reuses that snapshot for
  `room_registrations`, `inbound_events`, and outbound target resolution.
- Keep the agent-chat trust-mode behavior: `audit` joins untrusted invites but
  marks the resulting room registration as `trusted=false`; `enforce` leaves an
  untrusted invite and emits no room registration for it.
- Trusted invite registrations use trust reason `trusted_inviter`; untrusted
  audit-mode invite registrations use trust reason `untrusted_inviter`.
- Inbound Matrix text events suppress loopback from the bot user, Matrix agent
  puppet users, configured ignored senders, and bodies containing
  `[AGENTIGNORE]`.
- Outbound Matrix sends use existing `MatrixRoomDirectory` target-to-room
  resolution and call `MatrixClientPort::send_text_message` with the resolved
  room id and plain body.
- Keep `matrix_bridge` partial after this slice because agentd still does not
  include a real Matrix SDK crate, homeserver network login, puppet account
  lifecycle, Matrix media transfer, bot commands, long-running service
  packaging, service cutover, rollback, token provisioning/rotation, bridge
  operations, or dashboard/operator visibility.

## Boundaries

### Allowed Changes

- specs/e2e/p240-agent-chat-matrix-client-adapter-boundary.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/client_transport.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not add a real Matrix SDK dependency, Matrix homeserver dependency, real
  homeserver network test, puppet registration, Matrix media upload,
  long-running bridge daemon, systemd/launchd service, remote relay process,
  tmux injection worker, service cutover, rollback automation, token
  provisioning, or token rotation.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

- Matrix client-server API calls, account registration, token storage, actual
  SDK login, room history pagination, encrypted rooms, Matrix media transfer,
  Matrix bot command parsing, and bridge service packaging.
- Dashboard rendering for Matrix bridge rooms, runtime state, or outbox cursor.
- Remote relay integration and operator cutover automation.

## Completion Criteria

<!-- lint-ack: decision-coverage - p240 binds MatrixClientPort, MatrixClientBridgeTransport, login-before-sync, trust-mode invite behavior, loop suppression, outbound send resolution, and partial parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify fake client calls, registration fields, inbound event filters, outbound send calls, send-failure cursor behavior, and repository Markdown with local doubles only. -->
<!-- lint-ack: boundary-entry-point - agentd-matrix adapter tests and agentctl parity artifact tests cover the listed entry points. -->
<!-- lint-ack: error-path - p240 includes untrusted enforce, loop suppression, send failure, and parity-not-covered scenarios as exception coverage. -->

Scenario: Matrix client transport logs in, joins trusted invites, and registers synced rooms
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_logs_in_joins_trusted_invites_and_registers_rooms
  Level: library unit
  Test Double: in-memory fake Matrix client
  Given a fake Matrix client with one trusted invite and one joined room
  When `MatrixClientBridgeTransport::room_registrations` runs
  Then the fake client records `ensure_logged_in`, `sync_once`, and `join_room`
  And the returned registrations include the joined room and trusted invite room
  And the trusted invite registration records `trusted=true`, `trust_reason=trusted_inviter`, and the inviter MXID

Scenario: Matrix client transport enforces untrusted invite policy
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_enforces_untrusted_invite_policy
  Level: library unit
  Test Double: in-memory fake Matrix clients
  Given one untrusted invite in audit mode
  When room registrations are loaded
  Then the fake client joins the room and returns a registration with `trusted=false` and `trust_reason=untrusted_inviter`
  And given the same untrusted invite in enforce mode
  When room registrations are loaded
  Then the fake client leaves the room and returns no registration for it

Scenario: Matrix client transport normalizes inbound text and suppresses loops
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_normalizes_inbound_text_and_suppresses_loops
  Level: library unit
  Test Double: in-memory fake Matrix client
  Given Matrix text events from a human, the bot user, an agent puppet user, an ignored sender, and a body containing `[AGENTIGNORE]`
  When `MatrixClientBridgeTransport::inbound_events` runs
  Then only the human event is returned as a `MatrixInboundEvent`
  And the event preserves event id, room id, sender MXID, body, mentions, and reply target

Scenario: Matrix client transport sends outbound text to resolved target rooms
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_sends_outbound_text_to_resolved_target_room
  Level: library unit
  Test Double: in-memory fake Matrix client
  Given a synced trusted room mapping for target `codex-worker`
  When the transport sends an outbound event whose target is `codex-worker`
  Then the fake client records one `send_text_message` call with the mapped room id and outbound body

Scenario: Matrix client transport send failure preserves runtime retry cursor
  Test:
    Package: agentd-matrix
    Filter: matrix_client_transport_send_failure_preserves_runtime_retry_cursor
  Level: library integration
  Test Double: in-memory fake backend and fake Matrix client
  Given a bridge runtime with two backend outbox events and a fake Matrix client that fails the second send
  When `BridgeRuntime::run_once` executes through `MatrixClientBridgeTransport`
  Then the runtime returns a Matrix transport error containing the failing room id
  And only the first send is recorded
  And the bridge cursor remains at the first confirmed sequence

Scenario: parity docs record p240 Matrix client adapter boundary progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p240_matrix_client_adapter_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the Matrix bridge row and Phase G roadmap are inspected
  Then the Matrix bridge row mentions p240, `MatrixClientPort`, `MatrixClientBridgeTransport`, trust-mode invite handling, and loop suppression
  And the Matrix bridge row remains partial
  And the row still names the real Matrix SDK, puppet, media, cutover, rollback, and token gaps
