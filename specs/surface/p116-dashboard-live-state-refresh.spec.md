spec: task
name: "Dashboard refreshes read-only state after live events"
tags: [surface, http, p1, dashboard]
---

## Intent

P114/P115 made the local dashboard shell serve and listen to production SSE
events, but a visible event should also refresh the read-only run state shown on
screen. This task keeps the dashboard as a thin local operator/debug console by
re-reading existing HTTP endpoints after state-changing live events instead of
deriving run status in browser code.

## Decisions

- Named state-changing event handlers for `run_parked`, `run_finished`,
  `run_failed`, and `state_resync` append the event row and then trigger a
  read-only state refresh.
- The refresh reuses existing `GET /runs` and `GET /runs/:id` calls by invoking
  `loadRuns()` and `loadRunDetail(selectedRunId)` for the currently selected
  run.
- Generic `onmessage` and compatibility `node.parked` handlers continue to append
  events without becoming a second state authority.
- The dashboard remains static HTML/CSS/JS with no dependencies, no frontend
  build toolchain, no write controls, no MCP tool calls, and no Specify endpoint
  references.

## Boundaries

### Allowed Changes

- specs/surface/p116-dashboard-live-state-refresh.spec.md
- crates/agentd-surface/src/dashboard.html
- crates/agentd-surface/tests/http.rs

### Forbidden

- Do not modify crates/agentd-core/**.
- Do not modify crates/agentd-store/**.
- Do not modify crates/agentd-bin/**.
- Do not add dependencies or a frontend build toolchain.
- Do not add write-capable dashboard actions.

## Out of Scope

- Optimistic browser-side run status derivation from event payloads.
- Starting, canceling, retrying, or mutating runs from the dashboard.
- Authentication, authorization, public exposure, or persisted dashboard
  preferences.
- Specify integration or semantic event mapping.

## Completion Criteria

Scenario: production live events refresh the selected run state
  Test: dashboard_shell_refreshes_state_after_live_events
  Given the embedded dashboard HTML shell
  When its production live event handlers for "run_parked", "run_finished", "run_failed", and "state_resync" are inspected
  Then each handler appends the event row and triggers a selected-run state refresh
  And the refresh path calls both `loadRunDetail(selectedRunId)` and `loadRuns()`

Scenario: compatibility and generic event handlers stay log-only
  Test: dashboard_shell_keeps_generic_and_compat_events_log_only
  Given the embedded dashboard HTML shell
  When its generic `onmessage` and compatibility "node.parked" handlers are inspected
  Then they still append events without invoking the selected-run state refresh helper

Scenario: live state refresh remains read-only
  Test: dashboard_shell_live_state_refresh_remains_read_only
  Given the embedded dashboard HTML shell after live-refresh behavior is added
  When endpoint references and method strings are inspected
  Then it still uses only `fetch("/runs")`, `fetch(`/runs/${...}`), and `EventSource(`/runs/${...}/events`)
  And it does not contain "POST /runs", `method: "POST"`, "tools/call", or "Specify"
