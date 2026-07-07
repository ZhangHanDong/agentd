spec: task
name: "Local dashboard shell over existing read-only HTTP surface"
tags: [surface, http, p1, dashboard]
---

## Intent

P1.1 asks for a local operator/debug console, not a second command authority.
The HTTP surface already exposes the data a read-only dashboard needs:
`GET /runs`, `GET /runs/:id`, and `GET /runs/:id/events`. This task adds a
small static dashboard shell at `/dashboard` so an operator can inspect runs
from the daemon without adding a frontend build system or any write-capable
control path.

## Decisions

- `GET /dashboard` and `GET /dashboard/` return the same embedded HTML document
  with `Content-Type: text/html; charset=utf-8`.
- The shell is served by `agentd-surface::http::router` so both the in-process
  tests and `agentd-bin` daemon assembly expose it automatically.
- The shell reads only existing endpoints: it fetches `GET /runs`, fetches
  `GET /runs/:id` for a selected run, and tails `GET /runs/:id/events` with
  `EventSource`.
- The shell does not include write controls and does not call `POST /runs`,
  MCP tools, or any Specify endpoint.
- The implementation is static HTML/CSS/JS embedded with `include_str!`; no
  Node/npm tooling, bundler, asset pipeline, or additional dependency is
  introduced. [platform-specific: no Node/npm runtime in this Rust surface]

## Boundaries

### Allowed Changes

- specs/surface/p114-local-dashboard-shell.spec.md
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/dashboard.html
- crates/agentd-surface/tests/http.rs

### Forbidden

- Do not modify crates/agentd-core/**.
- Do not modify crates/agentd-store/**.
- Do not modify crates/agentd-bin/**.
- Do not add dependencies or a frontend build toolchain.
- Do not add write-capable dashboard actions.

## Out of Scope

- Authentication, authorization, and public internet exposure.
- Starting/canceling/retrying runs from the dashboard.
- Persisted dashboard preferences.
- Specify integration or semantic event mapping.

## Completion Criteria

Scenario: dashboard routes serve one HTML shell
  Test: dashboard_routes_serve_html_shell
  Given the surface router is built over a fake host
  When GET /dashboard and GET /dashboard/ are requested
  Then both responses are 200 with text/html content type
  And both bodies contain the dashboard root, runs list, run detail, and event log regions

Scenario: dashboard shell is static and does not query the host while serving
  Test: dashboard_shell_serves_without_host_reads
  Given a fake host whose list_runs endpoint would fail if called
  When GET /dashboard is requested
  Then the response is still 200 with the embedded dashboard HTML

Scenario: dashboard shell uses only existing read-only run endpoints
  Test: dashboard_shell_uses_existing_read_only_endpoints
  Given the dashboard HTML shell and existing read-only endpoints "/runs", "/runs/:id", and "/runs/:id/events"
  When its endpoint references are inspected
  Then it contains fetch("/runs"), fetch(`/runs/${...}`), and EventSource(`/runs/${...}/events`)
  And it does not contain POST /runs, method: "POST", MCP tool calls, or Specify endpoint references

Scenario: dashboard route rejects write-style requests
  Test: dashboard_route_rejects_post
  Given the surface router is built over a fake host
  When POST /dashboard is requested
  Then the response is not 200 and no run-start body is accepted
