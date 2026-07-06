spec: task
name: "Dashboard production event-kind alignment"
tags: [surface, http, p1, dashboard]
---

## Intent

P114 added a static read-only dashboard shell, but the shell must observe the
same named SSE events that the production daemon emits. This task aligns the
dashboard `EventSource` listeners with production `run_parked`, `run_finished`,
`run_failed`, and `state_resync` events, while keeping the shell local and
read-only.

## Decisions

- The dashboard shell registers `EventSource.addEventListener` handlers for
  production events `run_parked`, `run_finished`, `run_failed`, and
  `state_resync`.
- The existing `node.parked` listener is preserved as a compatibility listener
  for older fake/replay events in tests.
- `agentd-bin` daemon route coverage must exercise `daemon::build_router` over
  `ProductionRunHost` with a real `SqliteStore` and fake backends, not only the
  surface crate's fake host.
- The dashboard remains a static read-only local operator/debug console: no
  `POST /runs`, no `method: "POST"`, no MCP `tools/call`, and no Specify
  endpoint references.

## Boundaries

### Allowed Changes

- specs/surface/p115-dashboard-production-events.spec.md
- crates/agentd-surface/src/dashboard.html
- crates/agentd-surface/tests/http.rs
- crates/agentd-bin/tests/daemon_http.rs

### Forbidden

- Do not modify crates/agentd-core/**.
- Do not modify crates/agentd-store/**.
- Do not modify crates/agentd-bin/src/**.
- Do not add dependencies or a frontend build toolchain.
- Do not add write-capable dashboard actions.

## Out of Scope

- Authentication, authorization, or public internet exposure.
- Starting, canceling, retrying, or mutating runs from the dashboard.
- Persisted dashboard preferences.
- Specify integration or semantic event mapping.

## Completion Criteria

Scenario: dashboard shell listens to production SSE event kinds
  Test: dashboard_shell_listens_to_production_event_kinds
  Given the embedded dashboard HTML shell
  When its `EventSource` listener registrations are inspected
  Then it registers listeners for "run_parked", "run_finished", "run_failed", and "state_resync"
  And it preserves the compatibility "node.parked" listener

Scenario: production daemon router serves the dashboard shell
  Test: daemon_router_serves_dashboard_shell
  Given `daemon::build_router` is built over `ProductionRunHost` with a real store and fake backends
  When GET /dashboard is requested
  Then the response is 200 with the embedded dashboard HTML
  And the body contains the dashboard root and production "run_parked" listener

Scenario: dashboard shell remains read-only after production event alignment
  Test: dashboard_shell_remains_read_only_after_production_event_alignment
  Given the embedded dashboard HTML shell
  When its endpoint references are inspected after event listener alignment
  Then it still does not contain "POST /runs", `method: "POST"`, "tools/call", or "Specify"
