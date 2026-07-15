spec: task
name: "agent-chat Matrix preflight smoke harness"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p253]
---

## Intent

Move the Matrix bridge replacement path from fake-tested operator preflight to
an explicit real-environment smoke entrypoint. This slice adds a script harness
that operators can run against their Matrix homeserver only when they opt in,
while default tests keep using fake binaries and never connect to a real
homeserver.

## Decisions

- Add `scripts/agentd_matrix_client_bridge_preflight_smoke.sh`.
- The script supports `--dry-run`, `--preflight-only`, and `--execute`.
- Real execution requires `AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE=1` plus a Matrix
  homeserver URL from `--matrix-homeserver-url` or
  `AGENTD_MATRIX_HOMESERVER_URL`.
- The execute path runs `agentd matrix-client-bridge-preflight` and captures
  `preflight.out`, `preflight.err`, and `summary.txt` under
  `.agentd/matrix-preflight-smoke/<run-id>` by default.
- The script supports optional `--matrix-access-token`, `--matrix-user-id`, and
  `--matrix-device-id` flags or matching `AGENTD_MATRIX_*` environment values
  so operators can validate whoami when they have a token.
- Dry-run and summary output must redact the access token value.
- The script must verify that the bridge cursor state file was not created by
  the preflight command.
- Tests must use fake binaries only and must not contact a real Matrix
  homeserver.
- Keep Matrix account registration, bridge service execution, daemon
  supervision, service packaging, cutover, rollback, Matrix media, bot
  commands, dashboard rendering, and token rotation out of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p253-agent-chat-matrix-preflight-smoke-harness.spec.md
- scripts/agentd_matrix_client_bridge_preflight_smoke.sh
- crates/agentd-bin/tests/matrix_preflight_smoke.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not make this smoke harness run from `scripts/check.sh`.
- Do not enable `matrix-sdk-adapter` by default.
- Do not add a new persistence dependency or HTTP client dependency.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not add Matrix account registration, daemon supervision, service
  packaging, cutover, rollback, media transfer, bot command handling,
  dashboard rendering, Matrix profile/avatar sync, or token rotation.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

Real CI execution against a Matrix homeserver, real Matrix account
registration, long-running bridge service execution, systemd/launchd
packaging, encrypted room verification, DM/group room lifecycle creation,
media transfer, bot commands, operator cutover, rollback automation, dashboard
rendering, Matrix profile/avatar updates, token rotation, and remote relay
service packaging.

## Completion Criteria

<!-- lint-ack: decision-coverage - p253 binds script modes, opt-in env, homeserver URL requirement, evidence files, optional token/user/device values, token redaction, no state mutation, fake-only tests, and partial parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify stdout, stderr, exit status, env/flag handling, fake binary invocation, evidence files, state-file absence, and repository Markdown/source state. -->
<!-- lint-ack: error-path - p253 covers missing opt-in and missing homeserver URL error paths in addition to dry-run and execute success paths. -->

Scenario: dry-run prints Matrix preflight smoke plan without side effects
  Test:
    Package: agentd-bin
    Filter: matrix_preflight_smoke_dry_run_prints_plan_without_side_effects
  Level: script integration
  Test Double: temporary filesystem only
  Given `scripts/agentd_matrix_client_bridge_preflight_smoke.sh --dry-run` receives a Matrix homeserver URL and access token
  When the script prints its plan
  Then stdout mentions `agentd matrix-client-bridge-preflight`
  And stdout mentions `preflight.out`, `preflight.err`, and `summary.txt`
  And stdout reports the access token as redacted without printing the token value
  And the state directory is not created

Scenario: execute mode requires explicit real-smoke opt-in
  Test:
    Package: agentd-bin
    Filter: matrix_preflight_smoke_execute_requires_explicit_opt_in
  Level: script integration
  Test Double: temporary filesystem and fake agentd binary path
  Given `--execute` is passed without `AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE=1`
  When the script starts execution
  Then it exits non-zero
  And stderr names `AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE=1`
  And the state directory is not created

Scenario: preflight-only requires homeserver URL without starting anything
  Test:
    Package: agentd-bin
    Filter: matrix_preflight_smoke_preflight_only_requires_homeserver_url
  Level: script integration
  Test Double: temporary filesystem and fake agentd binary path
  Given `--preflight-only` omits both `--matrix-homeserver-url` and `AGENTD_MATRIX_HOMESERVER_URL`
  When the script validates local inputs
  Then it exits non-zero
  And stderr names `AGENTD_MATRIX_HOMESERVER_URL`
  And the state directory is not created

Scenario: execute mode invokes agentd preflight and writes redacted evidence
  Test:
    Package: agentd-bin
    Filter: matrix_preflight_smoke_execute_invokes_agentd_preflight_and_writes_evidence
  Level: script integration
  Test Double: fake agentd binary and temporary filesystem
  Given `AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE=1`
  And a fake `agentd` binary records its arguments and prints a successful preflight report
  And the script receives homeserver URL, access token, user id, device id, and `--skip-build`
  When `--execute` runs
  Then the fake binary is invoked with `matrix-client-bridge-preflight`
  And the captured stdout is written to `preflight.out`
  And `summary.txt` records `result: finished`
  And `summary.txt` redacts the access token value
  And the bridge cursor state file is not created
  And no `daemon.log` is created

Scenario: parity docs record p253 smoke harness without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p253_matrix_preflight_smoke_progress
  Level: artifact inspection
  Test Double: repository Markdown files and script source text
  Given the agent-chat replacement parity map, roadmap, and smoke script source
  When p253 progress is inspected
  Then the Matrix bridge row mentions p253 and `agentd_matrix_client_bridge_preflight_smoke.sh`
  And the Matrix bridge row remains partial
  And the row still names service packaging, Matrix media, cutover, rollback, token rotation, and dashboard/operator visibility gaps
  And the script mentions `AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE`, `matrix-client-bridge-preflight`, and `preflight.out`
