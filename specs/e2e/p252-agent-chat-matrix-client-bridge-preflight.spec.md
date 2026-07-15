spec: task
name: "agent-chat Matrix client bridge preflight"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p252]
---

## Intent

Move agentd closer to replacing agent-chat by adding an operator preflight for
the SDK-facing Matrix client bridge. The preflight must validate the existing
service configuration shape, probe the configured Matrix homeserver through
standard-library HTTP, optionally validate an access token with whoami, and
return an operator-readable report without running bridge iterations or
mutating bridge state.

## Decisions

- Add an `agentd matrix-client-bridge-preflight` CLI command.
- Reuse the `MatrixClientBridgeServiceArgs` option surface through a preflight
  wrapper so operators validate the same agentd API, state path, Matrix SDK,
  transport trust, known-agent, ignored-sender, trusted-inviter, and optional
  puppet account configuration used by `matrix-client-bridge-service`.
- Add `MatrixClientBridgePreflightReport`,
  `MatrixHomeserverPreflightReport`, and
  `run_matrix_client_bridge_preflight` in `agentd-bin::matrix_bridge`.
- The preflight must build `MatrixClientBridgeServiceConfig` first so existing
  CLI/config validation remains shared with the service path.
- The preflight must require `--matrix-homeserver-url`, send a read-only GET to
  `/_matrix/client/versions`, and reject non-2xx or malformed JSON responses.
- When `--matrix-access-token` is present, the preflight must send a read-only
  GET to `/_matrix/client/v3/account/whoami` with `Authorization: Bearer ...`
  and include the returned `user_id` in the report.
- The preflight must not call Matrix SDK login/sync, must not poll agentd
  Matrix outbox, and must not create or update the bridge cursor or puppet
  token state files.
- Keep real Matrix homeserver smoke tests, registration, durable token-store
  expansion, unbounded daemon supervision, service packaging, cutover,
  rollback, Matrix media, bot commands, dashboard rendering, and token rotation
  out of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p252-agent-chat-matrix-client-bridge-preflight.spec.md
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/lib.rs
- crates/agentd-bin/src/main.rs
- crates/agentd-bin/src/matrix_bridge.rs
- crates/agentd-bin/tests/matrix_client_bridge_preflight.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not run Matrix SDK login or sync from preflight tests.
- Do not enable the `matrix-sdk-adapter` feature by default.
- Do not add a new persistence dependency or HTTP client dependency.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not add account registration, unbounded daemon supervision, service
  packaging, cutover, rollback, media transfer, bot command handling,
  dashboard rendering, Matrix profile/avatar sync, or token rotation.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

Real Matrix homeserver smoke tests, real Matrix account registration,
long-running process supervision, systemd/launchd packaging, encrypted room
verification, DM/group room lifecycle creation, media transfer, bot commands,
operator cutover, rollback automation, dashboard rendering, Matrix
profile/avatar updates, token rotation, and remote relay service packaging.

## Completion Criteria

<!-- lint-ack: decision-coverage - p252 binds CLI args, shared service config validation, homeserver versions probe, optional whoami probe, no state mutation, and partial parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify CLI parse output, fake HTTP request paths and authorization header, on-disk state absence, malformed response failure, and repository Markdown/source state. -->
<!-- lint-ack: error-path - p252 covers missing homeserver URL and malformed versions response while preserving no-side-effect behavior. -->

Scenario: CLI accepts Matrix client bridge preflight configuration
  Test:
    Package: agentd-bin
    Filter: agentd_cli_matrix_client_bridge_preflight_reuses_service_options
  Level: CLI parser unit
  Test Double: clap parser only
  Given `agentd matrix-client-bridge-preflight` is parsed with the same SDK, transport, trust, known-agent, ignored-sender, trusted-inviter, and puppet account options as the service command
  When the CLI parser returns `AgentdCommand::MatrixClientBridgePreflight`
  Then the wrapped `MatrixClientBridgeServiceArgs` contains the bounded iteration count
  And it contains the SDK credential fields
  And it contains the Matrix transport trust, known-agent, skip-agent, trusted-inviter, and ignored-sender fields
  And it contains the optional puppet account provisioning fields

Scenario: preflight probes homeserver versions and access-token whoami without bridge side effects
  Test:
    Package: agentd-bin
    Filter: agentd_bin_matrix_client_bridge_preflight_probes_versions_and_whoami_without_state_mutation
  Level: integration unit
  Test Double: local fake Matrix homeserver and temporary filesystem
  Given the fake Matrix homeserver returns Matrix client versions and a whoami user id
  And the preflight args include an access token, state path, and puppet token-state path
  When `run_matrix_client_bridge_preflight` executes
  Then the fake Matrix homeserver records GET `/_matrix/client/versions`
  And it records GET `/_matrix/client/v3/account/whoami` with the bearer token
  And the returned report contains the advertised versions and whoami user id
  And the bridge cursor state file is not created
  And the puppet token-state file is not created

Scenario: preflight rejects malformed homeserver versions without state mutation
  Test:
    Package: agentd-bin
    Filter: agentd_bin_matrix_client_bridge_preflight_rejects_malformed_versions_without_state_mutation
  Level: integration unit
  Test Double: local fake Matrix homeserver and temporary filesystem
  Given the fake Matrix homeserver returns malformed JSON for `/_matrix/client/versions`
  And the preflight args include state and puppet token-state paths
  When `run_matrix_client_bridge_preflight` executes
  Then it returns a `BridgeError::Transport`
  And the bridge cursor state file is not created
  And the puppet token-state file is not created

Scenario: preflight requires a Matrix homeserver URL before HTTP side effects
  Test:
    Package: agentd-bin
    Filter: agentd_bin_matrix_client_bridge_preflight_requires_homeserver_url
  Level: unit
  Test Double: local argument struct only
  Given the preflight args omit `--matrix-homeserver-url`
  When `run_matrix_client_bridge_preflight` executes
  Then it returns `BridgeError::InvalidConfig`
  And the bridge cursor state file is not created

Scenario: parity docs record p252 preflight without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p252_matrix_client_bridge_preflight_progress
  Level: artifact inspection
  Test Double: repository Markdown files and Rust source text
  Given the agent-chat replacement parity map, roadmap, and `agentd-bin` source
  When p252 progress is inspected
  Then the Matrix bridge row mentions p252, `MatrixClientBridgePreflightReport`, and `run_matrix_client_bridge_preflight`
  And the Matrix bridge row remains partial
  And the row still names service packaging, Matrix media, cutover, rollback, token rotation, and dashboard/operator visibility gaps
  And the source mentions `matrix-client-bridge-preflight`, `MatrixClientBridgePreflightArgs`, and `MatrixHomeserverPreflightReport`
