spec: task
name: "agent-chat Matrix SDK bridge-once assembly"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p250]
---

## Intent

Move the Matrix bridge closer to replacing agent-chat by adding a fake-testable
SDK-facing one-shot bridge runner. This slice assembles the existing
`MatrixClientPort`/`MatrixClientBridgeTransport` path with the HTTP agentd
backend, durable cursor state, and optional p249 puppet account provisioning,
without starting a daemon service loop or connecting to a real Matrix
homeserver.

## Decisions

- Add `MatrixClientBridgeOnceConfig` in `agentd-matrix`.
- Add `run_matrix_client_bridge_once` in `agentd-matrix`.
- The runner must compose `AgentdHttpBackend`,
  `MatrixClientBridgeTransport`, `BridgeRuntime`, and JSON `BridgeState`
  persistence.
- The runner must accept any `MatrixClientPort`, so tests use fake clients and
  default builds do not require a real `SdkMatrixClient`.
- If optional `BridgeOncePuppetAccountConfig` is present, the runner must
  provision puppet accounts before Matrix client login/sync.
- The runner must save the cursor only after a full successful bridge pass.
- Keep daemon long-running service wiring, real SDK login/sync smoke, real
  homeserver validation, service packaging, cutover, rollback, media transfer,
  bot commands, profile sync, and token rotation out of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p250-agent-chat-matrix-sdk-bridge-once-assembly.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/client_bridge_once.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not enable the `matrix-sdk-adapter` feature by default.
- Do not add a new persistence dependency or HTTP client dependency.
- Do not add daemon long-running Matrix service management.
- Do not add service packaging, cutover, rollback, media transfer, bot command
  handling, Matrix profile/avatar sync, or token rotation.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

Daemon service lifecycle, CLI command wiring for a real SDK bridge service,
real SDK credential management, real Matrix homeserver smoke tests, encrypted
room verification, DM/group room lifecycle creation, media transfer, bot
commands, operator cutover, rollback automation, dashboard rendering, and
Matrix profile/avatar updates.

## Completion Criteria

<!-- lint-ack: decision-coverage - p250 binds SDK-facing bridge-once config, generic MatrixClientPort use, HTTP backend composition, puppet-before-sync ordering, cursor persistence, send-failure behavior, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify fake HTTP calls, fake Matrix client calls, on-disk cursor state, puppet token file side effects, failure non-persistence, and repository Markdown/source state. -->
<!-- lint-ack: error-path - p250 covers Matrix send failure and partial parity assertions. -->

Scenario: SDK-facing bridge-once runner syncs Matrix client and persists cursor
  Test:
    Package: agentd-matrix
    Filter: matrix_client_bridge_once_runner_syncs_client_posts_backend_sends_outbox_saves_cursor
  Level: integration unit
  Test Double: local fake agentd backend, fake Matrix client port, and temporary filesystem
  Given `MatrixClientBridgeOnceConfig` has an agentd HTTP backend URL and a cursor state path
  And a fake Matrix client exposes one trusted direct room and one inbound text event
  And the fake agentd backend returns one outbox event for that direct room
  When `run_matrix_client_bridge_once` executes with the fake Matrix client
  Then the fake Matrix client logs in and syncs through `MatrixClientPort`
  And the fake agentd backend receives room registration, inbound event, and outbox polling calls
  And the fake Matrix client sends the backend outbox text to the resolved room
  And the returned `BridgeOnceReport` contains the runtime counts and saved cursor
  And the cursor state file contains the confirmed outbox sequence

Scenario: SDK-facing bridge-once runner provisions puppets before Matrix sync
  Test:
    Package: agentd-matrix
    Filter: matrix_client_bridge_once_runner_provisions_puppets_before_matrix_sync
  Level: integration unit
  Test Double: local fake Matrix homeserver, local fake agentd backend, fake Matrix client port, and temporary filesystem
  Given `MatrixClientBridgeOnceConfig` includes `BridgeOncePuppetAccountConfig`
  And the fake Matrix client refuses login unless the puppet token file already contains the configured agent token
  When `run_matrix_client_bridge_once` executes
  Then the puppet provisioning report contains a logged-in outcome
  And the fake Matrix client still logs in and syncs successfully
  And the puppet token file contains the new token before bridge runtime uses the client

Scenario: SDK-facing bridge-once runner does not persist cursor on Matrix send failure
  Test:
    Package: agentd-matrix
    Filter: matrix_client_bridge_once_runner_preserves_cursor_on_matrix_send_failure
  Level: integration unit
  Test Double: local fake agentd backend, fake Matrix client port, and temporary filesystem
  Given the cursor state file starts at sequence "20"
  And the fake agentd backend returns two outbox events
  And the fake Matrix client fails while sending the second event
  When `run_matrix_client_bridge_once` executes
  Then it returns a `BridgeError::Transport`
  And the fake Matrix client records only the first successful send
  And the cursor state file remains at sequence "20"

Scenario: parity docs record p250 SDK bridge-once assembly without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p250_matrix_sdk_bridge_once_assembly_progress
  Level: artifact inspection
  Test Double: repository Markdown files and Rust source text
  Given the agent-chat replacement parity map, roadmap, and `agentd-matrix` source
  When p250 progress is inspected
  Then the Matrix bridge row mentions p250, `MatrixClientBridgeOnceConfig`, and `run_matrix_client_bridge_once`
  And the Matrix bridge row remains partial
  And the row still names daemon service assembly, Matrix media, cutover, rollback, token rotation, and service packaging gaps
  And the source mentions `MatrixClientBridgeTransport`, `AgentdHttpBackend`, and `BridgeRuntime`
