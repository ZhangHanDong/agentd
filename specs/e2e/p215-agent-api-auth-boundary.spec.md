spec: task
name: "Agent API auth boundary baseline"
tags: [agent-chat-replacement, auth, registry, lifecycle, phase-c, p215]
---

## Intent

Advance Phase C by adding an agent-chat compatible authentication boundary for
agent registry, launch, and runtime endpoints. p213 and p214 made the local
agent API useful, but those routes are still open by default; this slice adds a
configured bearer-token and per-agent-token guard while preserving explicit
development compatibility when no token is configured.

## Decisions

- `agentd_surface::http` keeps the existing open `router(AppState)` for tests
  and local development, and adds an authenticated router variant that applies
  an `AuthConfig`.
- Operator routes require `Authorization: Bearer <api-token>` when an API token
  is configured: `GET /api/agents`, `GET /api/agents/:name`,
  `GET /api/agents/:name/launch-env`, and `POST /api/agents/:name/start`.
- `GET /api/agents/:name/launch-env` and `POST /api/agents/:name/start` are
  local-only when auth is configured. Requests carrying a non-local forwarded
  address are rejected with HTTP 403 even if the bearer token is correct.
- Agent-owned routes require `X-Agent-Token` when an expected token exists for
  the agent and token mode is `hard`: `POST /api/agents`,
  `POST /api/agents/:name/heartbeat`, `POST /api/agents/:name/runtime`, and
  `POST /api/agents/:name/offline`.
- Agent-token `audit` mode records compatibility intent by allowing missing or
  wrong agent tokens instead of blocking; this preserves the staged migration
  behavior of agent-chat token rollout.
- `agentd` accepts an operator token and repeated agent-token assignments in
  daemon config, and the production router is built with auth enabled when that
  material is configured.
- `agentctl agent` commands can attach `Authorization` and `X-Agent-Token`
  headers through explicit flags or `AGENTD_API_TOKEN` /
  `AGENTD_AGENT_TOKEN` environment variables, without adding new dependencies.
- The parity map moves `api_auth_boundary` from `missing` to `partial`; full auth
  parity still requires dashboard/browser auth, bridge secrets, remote relay
  credentials, import-time token provisioning, and secret rotation.

## Boundaries

### Allowed Changes

- specs/e2e/p215-agent-api-auth-boundary.spec.md
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/tests/http.rs
- crates/agentd-surface/tests/runs_overview.rs
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/daemon.rs
- crates/agentd-bin/tests/agent_mcp_context.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentctl/src/agent.rs
- crates/agentctl/src/cli.rs
- crates/agentctl/tests/agent_cli.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not add new Cargo dependencies.
- Do not store plaintext API or agent tokens in SQLite.
- Do not implement dashboard browser sessions, Matrix bridge secrets, remote
  relay auth, token rotation, JSON import, messaging, scheduler, or cutover.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not claim that agentd can fully replace agent-chat after this slice.

## Out of Scope

- Loading agent tokens from existing agent-chat home directories.
- Generating or rotating per-agent tokens.
- Browser login/session auth for dashboard use.
- Matrix bridge and remote relay credentials.
- Secret redaction in logs beyond not printing configured tokens.

## Completion Criteria

<!-- lint-ack: decision-coverage - the bound daemon HTTP, surface HTTP, CLI, config, and parity tests cover this auth baseline. -->
<!-- lint-ack: observable-decision-coverage - this slice binds HTTP status codes, request headers, CLI request bytes, and docs parity status. -->
<!-- lint-ack: boundary-entry-point - daemon and CLI entry points are verified through `agentd-bin` and `agentctl` test selectors listed below. -->

Scenario: default router keeps development agent routes open
  Test:
    Package: agentd-surface
    Filter: http_router_default_keeps_agent_routes_open_without_auth_config
  Level: HTTP integration
  Test Double: FakeRunHost
  Given the existing `router(AppState)` without auth config
  When a client posts `/api/agents` without any auth headers
  Then the response is HTTP 200
  And the agent is registered through the fake host

Scenario: operator routes require configured bearer token
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_operator_routes_require_bearer_when_configured
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake ports
  Given a daemon router with API token "operator-secret"
  When a client gets `/api/agents` with no bearer, a wrong bearer, and the
  correct bearer
  Then the responses are HTTP 401, HTTP 401, and HTTP 200 respectively
  And the success response remains the normal agent list JSON

Scenario: launch-env and start are local-only operator routes
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_start_and_launch_env_reject_remote_operator_requests
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given a daemon router with API token "operator-secret" and a registered Codex
  agent
  When a request for launch-env or start includes the correct bearer but a
  non-local forwarded address
  Then the response is HTTP 403
  And the fake backend sees no spawn request for the rejected start

Scenario: agent-owned routes require configured agent token in hard mode
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_owned_routes_require_agent_token_in_hard_mode
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake ports
  Given a daemon router with agent token "codex-worker=agent-secret" in hard
  mode
  When clients call register, heartbeat, runtime, and offline without
  `X-Agent-Token`
  Then each response is HTTP 403
  When the same calls include `X-Agent-Token: agent-secret`
  Then each call succeeds

Scenario: audit mode allows missing agent tokens
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_token_audit_mode_allows_missing_tokens
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake ports
  Given a daemon router with agent token "codex-worker=agent-secret" in audit
  mode
  When a client sends a heartbeat for "codex-worker" without `X-Agent-Token`
  Then the response is HTTP 200
  And the agent is still marked online

Scenario: daemon config builds authenticated router
  Test:
    Package: agentd-bin
    Filter: agentd_cli_accepts_agent_api_auth_options
  Level: CLI config
  Test Double: clap parser
  Given the `agentd` CLI
  When it is parsed with `--api-token`, `--agent-token`, and
  `--agent-token-mode hard`
  Then the daemon config contains the operator token, agent token assignment,
  and hard token mode

Scenario: agentctl attaches auth headers from flags and env
  Test:
    Package: agentctl
    Filter: agent_cli_auth_headers_use_flags_and_env
  Level: CLI
  Test Double: one-shot local TCP daemon
  Given a local fake HTTP daemon that records raw requests
  When `agentctl agent ls --api-token operator-secret` runs
  Then the request contains `Authorization: Bearer operator-secret`
  When `AGENTD_API_TOKEN` is set for `agentctl agent launch-env`
  Then the request contains that bearer token
  When `agentctl agent runtime --agent-token agent-secret` runs
  Then the request contains `X-Agent-Token: agent-secret`

Scenario: parity map records p215 auth progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p215_auth_boundary_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the `api_auth_boundary` row is inspected
  Then its status is "partial"
  And its decision mentions p215 bearer, agent token, local-only, and remaining
  dashboard, bridge, relay, import, and rotation gaps
