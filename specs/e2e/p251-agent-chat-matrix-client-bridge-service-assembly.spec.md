spec: task
name: "agent-chat Matrix client bridge service assembly"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p251]
---

## Intent

Move the Matrix bridge closer to replacing agent-chat by adding a daemon-side,
fake-testable bounded service assembly for the SDK-facing Matrix client bridge.
This slice wires daemon configuration, Matrix client transport configuration,
cursor persistence, optional puppet account provisioning, and an injected
`MatrixClientPort` into repeatable service iterations without starting a real
long-running supervisor or connecting tests to a real Matrix homeserver.

## Decisions

- Add `MatrixClientBridgeServiceArgs` to the `agentd` CLI configuration.
- Add `MatrixClientBridgeServiceConfig`, `MatrixClientBridgeServiceReport`, and
  `run_matrix_client_bridge_service` in `agentd-bin::matrix_bridge`.
- The service runner must execute a bounded positive number of iterations and
  reuse one mutable `MatrixClientPort` across iterations.
- The service runner must delegate each iteration to the existing
  `run_matrix_client_bridge_once` path so `AgentdHttpBackend`,
  `MatrixClientBridgeTransport`, `BridgeRuntime`, JSON cursor persistence, and
  optional `BridgeOncePuppetAccountConfig` remain the single bridge behavior.
- Add a feature-gated real SDK service entrypoint that builds `SdkMatrixClient`
  only when `agentd-bin/matrix-sdk-adapter` is enabled; default builds must stay
  SDK-free and fake-testable.
- `matrix-client-bridge-service` CLI options must cover agentd API URL, state
  path, bounded iteration count, Matrix SDK credentials, transport trust
  settings, known/skip agents, ignored senders, trusted inviters, and optional
  puppet account provisioning.
- The runner must stop on the first failed iteration and preserve the cursor
  from the last fully successful iteration.
- Keep real homeserver validation, unbounded daemon supervision, service
  packaging, cutover, rollback, Matrix media, bot commands, dashboard rendering,
  and token rotation out of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p251-agent-chat-matrix-client-bridge-service-assembly.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-bin/Cargo.toml
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/lib.rs
- crates/agentd-bin/src/main.rs
- crates/agentd-bin/src/matrix_bridge.rs
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
- Do not enable the `matrix-sdk-adapter` feature by default.
- Do not add a new persistence dependency or HTTP client dependency.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not add unbounded daemon supervision, service packaging, cutover,
  rollback, media transfer, bot command handling, dashboard rendering, Matrix
  profile/avatar sync, or token rotation.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

Real Matrix homeserver smoke tests, long-running process supervision,
systemd/launchd packaging, encrypted room verification, DM/group room lifecycle
creation, media transfer, bot commands, operator cutover, rollback automation,
dashboard rendering, Matrix profile/avatar updates, token rotation, and remote
relay service packaging.

## Completion Criteria

<!-- lint-ack: decision-coverage - p251 binds CLI args, service config/report, bounded iterations, mutable MatrixClientPort reuse, feature-gated SDK entrypoint, failure cursor preservation, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify CLI parse output, fake HTTP calls, fake Matrix client calls, on-disk cursor state, failure non-persistence, and repository Markdown/source state. -->
<!-- lint-ack: error-path - p251 covers zero-iteration config rejection, failed second-iteration cursor preservation, unsupported default feature behavior, and partial parity assertions. -->

Scenario: CLI accepts Matrix client bridge service configuration
  Test:
    Package: agentd-bin
    Filter: agentd_cli_matrix_client_bridge_service_accepts_sdk_transport_and_puppet_options
  Level: CLI parser unit
  Test Double: clap parser only
  Given `agentd matrix-client-bridge-service` is parsed with agentd API, state path, SDK credentials, transport trust settings, known agents, ignored senders, and puppet account options
  When the CLI parser returns `AgentdCommand::MatrixClientBridgeService`
  Then the parsed `MatrixClientBridgeServiceArgs` contains the bounded iteration count
  And it contains the SDK credential fields
  And it contains the Matrix transport trust, known-agent, skip-agent, trusted-inviter, and ignored-sender fields
  And it contains the optional puppet account provisioning fields

Scenario: bounded service runner reuses one Matrix client and advances cursor per iteration
  Test:
    Package: agentd-bin
    Filter: agentd_bin_matrix_client_bridge_service_runs_bounded_iterations_with_fake_client
  Level: integration unit
  Test Double: local fake agentd backend, fake Matrix client port, and temporary filesystem
  Given `MatrixClientBridgeServiceConfig` has "2" iterations and a cursor state path
  And the fake Matrix client exposes a trusted direct room on both syncs
  And the fake agentd backend returns one outbox event for sequence "21" and then one outbox event for sequence "22"
  When `run_matrix_client_bridge_service` executes with the same mutable fake Matrix client
  Then the fake Matrix client records two login/sync passes on one client instance
  And the fake agentd backend polls outbox first from sequence "0" and then from sequence "21"
  And the returned `MatrixClientBridgeServiceReport` contains two iteration reports
  And the cursor state file contains sequence "22"

Scenario: service runner stops on failed later iteration without advancing that cursor
  Test:
    Package: agentd-bin
    Filter: agentd_bin_matrix_client_bridge_service_preserves_cursor_when_later_iteration_send_fails
  Level: integration unit
  Test Double: local fake agentd backend, fake Matrix client port, and temporary filesystem
  Given `MatrixClientBridgeServiceConfig` has "2" iterations and a cursor state path
  And the first iteration sends outbox sequence "21" successfully
  And the fake Matrix client fails while sending sequence "22" in the second iteration
  When `run_matrix_client_bridge_service` executes
  Then it returns a `BridgeError::Transport`
  And the fake Matrix client records the first successful send and the failed second send attempt
  And the cursor state file remains at sequence "21"

Scenario: default build reports SDK service command as feature-gated
  Test:
    Package: agentd-bin
    Filter: agentd_bin_matrix_client_bridge_service_default_build_requires_sdk_feature
  Level: unit
  Test Double: default cargo feature set
  Given the default `agentd-bin` build does not enable `matrix-sdk-adapter`
  When the Matrix client bridge service command is dispatched through the default feature path
  Then the returned error names the `matrix-sdk-adapter` feature
  And it does not attempt Matrix login, Matrix sync, or agentd HTTP bridge calls

Scenario: parity docs record p251 service assembly without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p251_matrix_client_bridge_service_progress
  Level: artifact inspection
  Test Double: repository Markdown files and Rust source text
  Given the agent-chat replacement parity map, roadmap, and `agentd-bin` source
  When p251 progress is inspected
  Then the Matrix bridge row mentions p251, `MatrixClientBridgeServiceConfig`, and `run_matrix_client_bridge_service`
  And the Matrix bridge row remains partial
  And the row still names real homeserver validation, Matrix media, cutover, rollback, token rotation, and service packaging gaps
  And the source mentions `matrix-client-bridge-service`, `MatrixClientBridgeServiceArgs`, and `matrix-sdk-adapter`
