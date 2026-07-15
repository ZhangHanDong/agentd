spec: task
name: "agent-chat Matrix SDK adapter"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p241]
---

## Intent

Move the agent-chat Matrix replacement path from an SDK-facing port boundary to
a feature-gated real Matrix SDK adapter in `agentd-matrix`. This slice must
prove the SDK dependency path compiles and that the adapter exposes build,
login/session, sync, join, leave, and send operations through the existing
`MatrixClientPort` contract without requiring a real homeserver or real Claude
in tests.

## Decisions

- Add a crate feature named `matrix-sdk-adapter`; default builds keep the real
  `matrix-sdk` dependency disabled.
- Add `SdkMatrixClientConfig` with homeserver URL, optional username/password
  login, optional access-token session restore, optional device id, sync timeout
  milliseconds, and optional SQLite store path.
- Add `SdkMatrixClient` behind `matrix-sdk-adapter`; it owns a
  `matrix_sdk::Client` and a current-thread Tokio runtime so the existing
  synchronous `MatrixClientPort` trait can stay unchanged.
- `SdkMatrixClient::build` uses `Client::builder().homeserver_url(...)` and
  must not use server-name discovery for direct homeserver URLs.
- `ensure_logged_in` returns the current SDK user id when a session already
  exists, restores an access-token session when configured, otherwise performs
  username/password login when configured.
- `sync_once` calls `matrix_sdk::Client::sync_once` with the configured timeout,
  returns normalized joined and invited room ids from SDK room state, and leaves
  timeline text-event parsing for a later slice.
- `join_room`, `leave_room`, and `send_text_message` parse Matrix room ids,
  return `BridgeError::Transport` for invalid or unknown room ids, and use SDK
  room join/leave/send operations for valid rooms.
- Keep `matrix_bridge` partial after this slice because agentd still lacks
  puppet account provisioning, full Matrix timeline text parsing, media,
  bot commands, long-running service packaging, cutover, rollback, token
  provisioning/rotation, bridge operations, and dashboard visibility.

## Boundaries

### Allowed Changes

- specs/e2e/p241-agent-chat-matrix-sdk-adapter.spec.md
- crates/agentd-matrix/Cargo.toml
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/sdk_adapter.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- Cargo.lock

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not register Matrix bot or puppet accounts.
- Do not upload Matrix media or add bot command handling.
- Do not add long-running bridge service packaging, cutover, rollback, token
  provisioning, or token rotation.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

- Real homeserver integration tests, account registration, puppet account
  lifecycle, full event timeline normalization, encrypted-room verification,
  Matrix media transfer, Matrix bot command parsing, service installation,
  operator cutover, rollback automation, and dashboard rendering.

## Completion Criteria

<!-- lint-ack: decision-coverage - p241 binds matrix-sdk-adapter feature, config defaults/validation, SDK compile path, login/session mode selection, SDK operation error mapping, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify Cargo metadata, feature-gated compile checks, public API availability, invalid input errors, and repository Markdown with local-only tests. -->
<!-- lint-ack: error-path - p241 includes invalid homeserver, conflicting credentials, invalid room id, unknown room id, and not-covered parity scenarios. -->

Scenario: Matrix SDK adapter feature is opt-in and default builds stay SDK-free
  Test:
    Package: agentd-matrix
    Filter: sdk_adapter_feature_is_opt_in_and_default_build_stays_sdk_free
  Level: artifact inspection
  Test Double: Cargo manifest text
  Given the `agentd-matrix` manifest
  When feature and dependency declarations are inspected
  Then `matrix-sdk-adapter` exists as a feature
  And `matrix-sdk` is an optional dependency
  And the default feature set does not include `matrix-sdk-adapter`

Scenario: SDK adapter config validates homeserver and credential modes
  Test:
    Package: agentd-matrix
    Filter: sdk_matrix_client_config_validates_homeserver_and_credentials
  Level: library unit
  Test Double: local config values only
  Given direct homeserver URLs and credential combinations
  When `SdkMatrixClientConfig::validate` runs
  Then valid password-login and token-restore configurations pass
  And an empty homeserver URL fails
  And a config that mixes password login with token restore fails

Scenario: SDK adapter builds local clients from direct homeserver URLs
  Test:
    Package: agentd-matrix
    Filter: sdk_matrix_client_builds_local_client_from_direct_homeserver_url
  Level: library unit
  Test Double: locally built SDK client only
  Given a direct homeserver URL configuration
  When `SdkMatrixClient::build` runs with `matrix-sdk-adapter` enabled
  Then the adapter builds a local SDK client without server-name discovery
  And the adapter exposes the configured homeserver URL for inspection

Scenario: SDK adapter sync path is source-bound to the Matrix SDK
  Test:
    Package: agentd-matrix
    Filter: sdk_matrix_client_sync_path_is_source_bound_to_matrix_sdk
  Level: artifact inspection
  Test Double: Rust source text
  Given the `agentd-matrix` library source
  When the `SdkMatrixClient::sync_once` implementation is inspected
  Then it calls `matrix_sdk::Client::sync_once`
  And it reads SDK `joined_rooms` and `invited_rooms`
  And it returns an empty `text_events` list until full timeline parsing is implemented later

Scenario: SDK adapter maps invalid and unknown Matrix room ids to transport errors
  Test:
    Package: agentd-matrix
    Filter: sdk_matrix_client_maps_room_lookup_errors_without_network
  Level: library unit
  Test Double: locally built SDK client with no synced rooms
  Given a locally built `SdkMatrixClient` with no synced rooms
  When `leave_room` receives an invalid Matrix room id
  Then it returns a `BridgeError::Transport` mentioning the invalid room id
  And when `send_text_message` receives a valid but unknown Matrix room id
  Then it returns a `BridgeError::Transport` mentioning the unknown room id

Scenario: SDK adapter feature path compiles with matrix-sdk enabled
  Test:
    Package: agentd-matrix
    Filter: sdk_adapter_feature_path_compiles_with_matrix_sdk_enabled
  Level: compile check
  Test Double: cargo check only, no homeserver
  Given the `matrix-sdk-adapter` feature is enabled
  When `cargo check -p agentd-matrix --features matrix-sdk-adapter --tests` runs
  Then the crate compiles without connecting to a Matrix homeserver

Scenario: parity docs record p241 SDK adapter progress without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p241_matrix_sdk_adapter_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the Matrix bridge row and Phase G roadmap are inspected
  Then the Matrix bridge row mentions p241, `matrix-sdk-adapter`, and `SdkMatrixClient`
  And the Matrix bridge row remains partial
  And the row still names puppet accounts, full timeline parsing, media, cutover, rollback, and token gaps
