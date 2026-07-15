spec: task
name: "agent-chat Matrix service smoke harness"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p254]
---

## Intent

Move the Matrix bridge replacement path from preflight-only operator evidence
to an explicit bounded service smoke entrypoint. This slice adds a script that
operators can run against their agentd daemon and Matrix homeserver only when
they opt in, while default tests keep using fake binaries and never connect to
a real Matrix homeserver.

## Decisions

- Add `scripts/agentd_matrix_client_bridge_service_smoke.sh`.
- The script supports `--dry-run`, `--preflight-only`, and `--execute`.
- Real execution requires `AGENTD_REAL_MATRIX_SERVICE_SMOKE=1` plus a Matrix
  homeserver URL and exactly one Matrix SDK login mode: username/password or
  user-id/access-token.
- The execute path must build `agentd-bin` with `--features
  matrix-sdk-adapter` unless `--skip-build` is passed.
- The execute path must run `agentd matrix-client-bridge-preflight` before
  `agentd matrix-client-bridge-service`.
- The execute path captures `preflight.out`, `preflight.err`, `service.out`,
  `service.err`, and `summary.txt` under
  `.agentd/matrix-service-smoke/<run-id>` by default.
- Dry-run and summary output must redact Matrix password, access token,
  puppet password secret, and registration token values.
- The script must verify that a successful service command created the bridge
  cursor state file.
- Tests must use fake binaries only and must not contact a real Matrix
  homeserver or start a real daemon.
- Keep unbounded daemon supervision, service packaging, cutover, rollback,
  Matrix media, bot commands, dashboard rendering, and token rotation out of
  this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p254-agent-chat-matrix-service-smoke-harness.spec.md
- scripts/agentd_matrix_client_bridge_service_smoke.sh
- crates/agentd-bin/tests/matrix_service_smoke.rs
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
- Do not make this smoke harness run from `scripts/check.sh`.
- Do not enable `matrix-sdk-adapter` by default.
- Do not add new Rust dependencies.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not add unbounded daemon supervision, service packaging, cutover,
  rollback, media transfer, bot command handling, dashboard rendering, Matrix
  profile/avatar sync, or token rotation.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

Real Matrix homeserver execution in CI, real Matrix account registration,
long-running process supervision, systemd/launchd packaging, encrypted room
verification, DM/group room lifecycle creation, media transfer, bot commands,
operator cutover, rollback automation, dashboard rendering, Matrix
profile/avatar updates, token rotation, and remote relay service packaging.

## Completion Criteria

<!-- lint-ack: decision-coverage - p254 binds script modes, opt-in env, SDK login-mode validation, matrix-sdk-adapter build feature, preflight-before-service ordering, evidence files, secret redaction, cursor-state evidence, fake-only tests, and partial parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify stdout/stderr behavior, command arguments, fake binary invocation ordering, evidence files, cursor state creation, no state-dir side effects, and repository Markdown/source state. -->
<!-- lint-ack: error-path - p254 covers missing opt-in, missing SDK auth mode, and partial parity assertions. -->
<!-- lint-ack: boundary-entry-point - p254 touches one shell entry point and one test entry point; scenarios reference both. -->

Scenario: dry-run prints Matrix service smoke plan without side effects
  Test:
    Package: agentd-bin
    Filter: matrix_service_smoke_dry_run_prints_plan_without_side_effects
  Level: script unit
  Test Double: temporary filesystem
  Given `scripts/agentd_matrix_client_bridge_service_smoke.sh --dry-run` is invoked with homeserver, username, password, and access-token-like secrets
  When the script prints its plan
  Then stdout mentions `agentd matrix-client-bridge-preflight`
  And stdout mentions `agentd matrix-client-bridge-service`
  And stdout mentions `preflight.out`, `service.out`, and `summary.txt`
  And stdout redacts password, access token, puppet password secret, and registration token values
  And the state directory is not created

Scenario: execute refuses without explicit Matrix service opt-in
  Test:
    Package: agentd-bin
    Filter: matrix_service_smoke_execute_requires_explicit_opt_in
  Level: script unit
  Test Double: fake agentd binary and temporary filesystem
  Given `--execute` is passed without `AGENTD_REAL_MATRIX_SERVICE_SMOKE=1`
  When the service smoke script runs
  Then it exits non-zero
  And stderr names `AGENTD_REAL_MATRIX_SERVICE_SMOKE=1`
  And the state directory is not created

Scenario: preflight-only requires one Matrix SDK login mode
  Test:
    Package: agentd-bin
    Filter: matrix_service_smoke_preflight_only_requires_login_mode
  Level: script unit
  Test Double: fake agentd binary and temporary filesystem
  Given a Matrix homeserver URL is configured without username/password or user-id/access-token
  When the service smoke script runs in `--preflight-only`
  Then it exits non-zero
  And stderr names the username/password and user-id/access-token login modes
  And the state directory is not created

Scenario: execute runs preflight then bounded service and writes evidence
  Test:
    Package: agentd-bin
    Filter: matrix_service_smoke_execute_invokes_preflight_then_service_and_writes_evidence
  Level: script integration unit
  Test Double: fake agentd binary and temporary filesystem
  Given `AGENTD_REAL_MATRIX_SERVICE_SMOKE=1`
  And a fake agentd binary records invocations and creates the service cursor state file
  When `agentd_matrix_client_bridge_service_smoke.sh --execute --skip-build` runs with username/password credentials
  Then the fake agentd binary receives `matrix-client-bridge-preflight` before `matrix-client-bridge-service`
  And the service invocation includes `--features` only in the build command, not the agentd command
  And `preflight.out`, `preflight.err`, `service.out`, `service.err`, and `summary.txt` are written
  And `summary.txt` reports `result: finished`
  And `summary.txt` redacts the Matrix password
  And the bridge cursor state file exists

Scenario: parity docs record p254 service smoke harness without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p254_matrix_service_smoke_progress
  Level: artifact inspection
  Test Double: repository Markdown files and shell source text
  Given `crates/agentctl/tests/parity_cli.rs` inspects the agent-chat replacement parity map, roadmap, and Matrix service smoke script
  When p254 progress is inspected
  Then the Matrix bridge row mentions p254 and `agentd_matrix_client_bridge_service_smoke.sh`
  And the Matrix bridge row remains partial
  And the row still names service packaging, Matrix media, cutover, rollback, token rotation, and dashboard/operator visibility gaps
  And the roadmap mentions p254, `AGENTD_REAL_MATRIX_SERVICE_SMOKE`, and bounded service smoke
  And the script mentions `matrix-sdk-adapter`, `matrix-client-bridge-service`, `service.out`, and `password: set (redacted)`
