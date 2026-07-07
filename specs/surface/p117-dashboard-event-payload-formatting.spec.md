spec: task
name: "Dashboard formats JSON event payloads"
tags: [surface, http, p1, dashboard]
---

## Intent

The local dashboard event log currently displays SSE payload data as a single
escaped string, which makes production JSON payloads hard to scan. This task
improves operator readability by formatting JSON payloads in the dashboard
rendering layer only, without changing the HTTP/SSE protocol or backend event
storage.

## Decisions

- `appendEvent` renders event payloads through a `formatEventData(data)` helper.
- `formatEventData(data)` attempts `JSON.parse(data)` and renders valid JSON via
  `JSON.stringify(parsed, null, 2)` before HTML escaping.
- Non-JSON payloads fall back to the existing escaped raw text behavior.
- The event data element preserves whitespace so pretty-printed JSON remains
  readable in the log.
- The dashboard remains static HTML/CSS/JS with no dependencies, no frontend
  build toolchain, no write controls, no MCP tool calls, and no Specify endpoint
  references.

## Boundaries

### Allowed Changes

- specs/surface/p117-dashboard-event-payload-formatting.spec.md
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

- Changing event payload schemas, event kinds, SSE frame encoding, or persistence.
- Filtering, searching, collapsing, or grouping event log rows.
- Starting, canceling, retrying, or mutating runs from the dashboard.
- Specify integration or semantic event mapping.

## Completion Criteria

Scenario: JSON event payloads are pretty-printed in the dashboard log
  Test: dashboard_shell_pretty_prints_json_event_payloads
  Level: static HTML contract test
  Given the embedded dashboard HTML shell
  When its event payload rendering path is inspected
  Then `appendEvent` uses `formatEventData(data)`
  And `formatEventData(data)` parses payloads with `JSON.parse(data)` and renders valid JSON with `JSON.stringify(parsed, null, 2)`

Scenario: malformed JSON event payloads keep escaped raw rendering
  Test: dashboard_shell_keeps_raw_fallback_for_non_json_event_payloads
  Level: static HTML contract test
  Given the embedded dashboard HTML shell
  When `formatEventData(data)` handles malformed JSON or another payload that cannot be parsed as JSON
  Then the fallback path returns `escapeText(data)` without throwing

Scenario: formatted event payloads remain readable and read-only
  Test: dashboard_shell_event_payload_formatting_remains_read_only
  Level: static HTML contract test
  Given the embedded dashboard HTML shell after event payload formatting is added
  When CSS and endpoint references are inspected
  Then the event data style preserves whitespace
  And the shell still does not contain "POST /runs", `method: "POST"`, "tools/call", or "Specify"
