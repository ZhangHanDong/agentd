spec: task
name: "agent-chat Matrix puppet HTTP account port"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p246]
---

## Intent

Move the Matrix bridge one slice closer to agent-chat replacement by wiring the
p245 puppet account port to the Matrix Client-Server HTTP account endpoints,
tested only against a local fake HTTP server. This slice gives the account
executor a real request/response adapter for whoami, password login, and
agent-chat-compatible registration UIA flow while still avoiding a real
homeserver in tests.

## Decisions

- Add `MatrixPuppetHttpAccountPort` in `agentd-matrix` implementing
  `MatrixPuppetAccountPort`.
- Reuse the existing standard-library HTTP request path and `HttpEndpoint`
  parsing style; do not add a new HTTP client dependency.
- Add `MatrixPuppetHttpAccountConfig` with a homeserver base URL and optional
  registration token.
- `whoami` must send `GET /_matrix/client/v3/account/whoami` with
  `Authorization: Bearer <token>` and return the response `user_id`.
- `login` must send `POST /_matrix/client/v3/login` with Matrix password-login
  JSON: `type=m.login.password`, `identifier.type=m.id.user`,
  `identifier.user=<localpart>`, and `password=<password>`.
- `register` must mirror agent-chat's two-step UIA behavior: first probe
  `POST /_matrix/client/v3/register` with username/password, return immediately
  if the probe includes `access_token`, otherwise complete the returned session
  with registration-token auth when configured or dummy auth when the probe
  offers a single-stage `m.login.dummy` flow.
- Accept an UIA registration probe response with a session and flows even when
  the HTTP status is 401, because Matrix homeservers commonly use 401 for
  incomplete UIA.
- Map non-success login/whoami/completion statuses, missing JSON fields, no
  usable registration flow, and malformed JSON into `BridgeError::Transport`
  for account port callers.
- Keep long-running bridge service wiring, SDK account integration, token-store
  persistence backends, token rotation, display-name/avatar sync, real
  homeserver tests, cutover, and rollback out of this slice.

## Boundaries

### Allowed Changes

- specs/e2e/p246-agent-chat-matrix-puppet-http-account-port.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/puppet_http_account.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not add a new HTTP client dependency.
- Do not persist Matrix passwords or access tokens outside local test doubles.
- Do not upload or download Matrix media.
- Do not add Matrix bot command handling.
- Do not add long-running bridge service packaging, cutover, rollback, token
  rotation, invite polling, display-name sync, or avatar sync.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

SDK-backed account provisioning, durable token storage backends, token rotation,
bridge service installation, encrypted-room verification, DM/group room
lifecycle, media transfer, bot commands, operator cutover, rollback automation,
dashboard rendering, and Matrix profile/avatar updates.

## Completion Criteria

<!-- lint-ack: decision-coverage - p246 binds HTTP whoami, login JSON, registration probe including 401 UIA, token/dummy UIA completion, error mapping, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify HTTP method/path/headers/body, returned session fields, error variants, local fake-server isolation, and repository Markdown state. -->
<!-- lint-ack: error-path - p246 includes non-success/malformed responses, missing fields, no usable registration flow, and partial parity assertions. -->

Scenario: HTTP account port validates existing tokens with whoami
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_http_account_port_whoami_sends_bearer_and_reads_user_id
  Level: integration unit
  Test Double: local fake HTTP server
  Given `MatrixPuppetHttpAccountPort` configured with a local fake homeserver URL
  When `whoami` is called with an access token
  Then the fake server receives `GET /_matrix/client/v3/account/whoami`
  And the request includes `Authorization: Bearer existing-token`
  And the port returns `MatrixPuppetWhoami` with the response `user_id`

Scenario: HTTP account port logs in using Matrix password-login JSON
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_http_account_port_login_posts_password_identifier_json
  Level: integration unit
  Test Double: local fake HTTP server
  Given `MatrixPuppetHttpAccountPort` configured with a local fake homeserver URL
  When `login` is called for localpart `ac_codex-worker`
  Then the fake server receives `POST /_matrix/client/v3/login`
  And the JSON body includes `type`, `identifier.type`, `identifier.user`, and `password`
  And the port returns `MatrixPuppetAccountSession` with `user_id` and `access_token`

Scenario: HTTP account port returns probe registration sessions without UIA
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_http_account_port_register_accepts_access_token_probe
  Level: integration unit
  Test Double: local fake HTTP server
  Given the fake homeserver returns `access_token` and `user_id` to the registration probe
  When `register` is called for localpart `ac_codex-worker`
  Then the port sends exactly one `POST /_matrix/client/v3/register`
  And the probe body contains `username` and `password`
  And the port returns the probe session without sending an UIA completion request

Scenario: HTTP account port completes registration with token or dummy UIA
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_http_account_port_register_completes_token_or_dummy_uia
  Level: integration unit
  Test Double: local fake HTTP server
  Given one fake homeserver probe returns HTTP 401 with a session and dummy support
  And another fake homeserver probe returns HTTP 401 with a session while the port has a registration token
  When `register` is called against each fake homeserver
  Then the dummy path sends auth kind `m.login.dummy` with the probe session
  And the token path sends auth kind `m.login.registration_token`, token, and the probe session
  And both calls return `MatrixPuppetAccountSession`

Scenario: HTTP account port reports malformed or unusable account responses
  Test:
    Package: agentd-matrix
    Filter: matrix_puppet_http_account_port_reports_status_malformed_and_no_uia_errors
  Level: integration unit
  Test Double: local fake HTTP server
  Given local fake homeservers that return a non-success login status, malformed JSON, missing login fields, and no usable registration flow
  When `whoami`, `login`, and `register` are called against those fake homeservers
  Then each call returns `BridgeError::Transport`
  And no test connects to a real Matrix homeserver

Scenario: parity docs record p246 HTTP account port progress without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p246_matrix_puppet_http_account_port_progress
  Level: artifact inspection
  Test Double: repository Markdown files and Rust source text
  Given the agent-chat replacement parity map, roadmap, and `agentd-matrix` source
  When p246 progress is inspected
  Then the Matrix bridge row mentions p246 and `MatrixPuppetHttpAccountPort`
  And the Matrix bridge row remains partial
  And the row still names Matrix media, cutover, rollback, token rotation, and service packaging gaps
  And the source mentions `MatrixPuppetHttpAccountConfig` and `MatrixPuppetHttpAccountPort`
