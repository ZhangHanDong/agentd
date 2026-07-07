spec: task
name: "Dashboard refresh button updates selected run detail"
tags: [surface, http, p1, dashboard]
---

## Intent

The local dashboard refresh button currently reloads only the run overview list,
so a selected run's detail pane can stay stale when the operator manually
refreshes after an SSE disconnect or a missed event. This task wires the manual
refresh control to the same read-only selected-run refresh path added by P116.

## Decisions

- The refresh button calls a dedicated `refreshDashboard()` handler instead of
  calling `loadRuns()` directly.
- `refreshDashboard()` calls `refreshSelectedRunState()` when `selectedRunId` is
  set, so the selected run detail and run overview are both re-read.
- `refreshDashboard()` falls back to `loadRuns()` when no run is selected.
- The change does not recreate the `EventSource`; event tail lifecycle remains
  owned by `selectRun(runId)`.
- The dashboard remains static HTML/CSS/JS with no dependencies, no frontend
  build toolchain, no write controls, no MCP tool calls, and no Specify endpoint
  references.

## Boundaries

### Allowed Changes

- specs/surface/p118-dashboard-refresh-selected-run.spec.md
- crates/agentd-surface/src/dashboard.html
- crates/agentd-surface/tests/http.rs

### Forbidden

- Do not modify crates/agentd-core/**.
- Do not modify crates/agentd-store/**.
- Do not modify crates/agentd-bin/**.
- Do not modify agentd-surface HTTP/SSE route semantics.
- Do not add dependencies or a frontend build toolchain.
- Do not add write-capable dashboard actions.

## Out of Scope

- Reconnecting or recreating SSE tails from the refresh button.
- Polling, debouncing, or automatic interval refresh.
- Starting, canceling, retrying, or mutating runs from the dashboard.
- Specify integration or semantic event mapping.

## Completion Criteria

Scenario: refresh button updates selected run detail and overview
  Test: dashboard_shell_refresh_button_updates_selected_run_state
  Given the embedded dashboard HTML shell
  When the refresh button click handler is inspected
  Then it calls `refreshDashboard()`
  And `refreshDashboard()` calls `refreshSelectedRunState()` when `selectedRunId` is present

Scenario: refresh button falls back to overview when no run is selected
  Test: dashboard_shell_refresh_button_falls_back_to_runs_without_selection
  Given the embedded dashboard HTML shell
  When `refreshDashboard()` is inspected
  Then it calls `loadRuns()` when `selectedRunId` is absent

Scenario: refresh button remains read-only and does not recreate event tails
  Test: dashboard_shell_refresh_button_remains_read_only_and_keeps_event_tail
  Given the embedded dashboard HTML shell after refresh-button alignment
  When endpoint references and click handler code are inspected
  Then the shell still uses only `fetch("/runs")`, `fetch(`/runs/${...}`), and `EventSource(`/runs/${...}/events`)
  And the refresh button handler does not call `tailEvents`
  And the shell does not contain "POST /runs", `method: "POST"`, "tools/call", or "Specify"
