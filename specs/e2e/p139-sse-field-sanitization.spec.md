spec: task
name: "SSE field sanitization at the stream boundary"
tags: [e2e, surface, sse, p1, recovery]
---

## Intent

The P0.9 deployment checklist still carries the P0.7 D9 hazard: axum SSE
frames panic when untrusted `event` or `data` field values contain CR/LF. The
production emit point currently serializes payloads compactly, but the SSE
boundary must still be defensive because it streams opaque `EventRecord` values
from replay and live broadcast sources. This slice makes the SSE frame builder
sanitize CR/LF before calling axum's `Event::event` or `Event::data`, so
malformed stored or live event fields cannot panic the HTTP route or inject extra
SSE fields.

## Decisions

- Keep the persisted `EventRecord` shape unchanged; sanitization is only at the
  HTTP/SSE frame boundary.
- Sanitize event kinds by replacing `\r` and `\n` with `_` before passing them
  to `Event::event`.
- Sanitize payload data by replacing `\r` with the literal string `\\r` and
  `\n` with the literal string `\\n` before passing it to `Event::data`.
- Apply the same helpers to replayed events, live events, and `state_resync`
  frames.
- Update `p73-http-routes` and the deployment checklist to mark the D9 hazard
  resolved by P139.

## Boundaries

### Allowed Changes

- specs/e2e/p139-sse-field-sanitization.spec.md
- specs/surface/p73-http-routes.spec.md
- docs/p0.9-deployment-checklist.md
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/tests/sse.rs
- crates/agentd-surface/tests/http.rs
- crates/agentd-bin/tests/deployment_checklist.rs

### Forbidden

- Do not modify files under `crates/agentd-core/**`.
- Do not modify files under `crates/agentd-store/**`.
- Do not change `EventRecord`, `LiveEvent`, or `RunHost` public type shapes.
- Do not change the production emit payload JSON shape or event kind names.
- Do not change dashboard JavaScript behavior.

## Out of Scope

- Validating or rejecting malformed event rows before persistence.
- Changing compact JSON serialization at the production emit point.
- Changing SSE retry/keepalive behavior.
- SSE event emission transactionality.

## Completion Criteria

Scenario: live stream sanitizes event kind CRLF
  Test:
    Package: agentd-surface
    Filter: live_stream_sanitizes_event_kind_crlf
  Level: stream boundary
  Test Double: live_event_stream with fake host
  Given a replayed event whose `kind` contains CR/LF and an injected `event:` line
  When the SSE stream is collected through axum's `Sse` response
  Then collection does not panic
  And the wire body contains one sanitized `event:` line for that record
  And it does not contain the injected `event:` line as a separate SSE field

Scenario: live stream sanitizes payload CRLF
  Test:
    Package: agentd-surface
    Filter: live_stream_sanitizes_payload_crlf
  Level: stream boundary
  Test Double: live_event_stream with fake host
  Given a replayed event whose `payload` contains CR/LF and injected `event:`/`data:` lines
  When the SSE stream is collected through axum's `Sse` response
  Then collection does not panic
  And the wire body contains literal `\\r` and `\\n` escapes
  And it does not contain the injected `event:` or `data:` line as a separate SSE field

Scenario: HTTP SSE route sanitizes CRLF fields
  Test:
    Package: agentd-surface
    Filter: http_sse_sanitizes_crlf_fields
  Level: HTTP route
  Test Double: axum oneshot over FakeRunHost
  Given a host event replay with CR/LF in both `kind` and `payload`
  When GET /runs/r1/events is requested
  Then the response is 200
  And collecting the body does not panic
  And the body contains no injected SSE field lines

Scenario: deployment checklist marks SSE sanitization resolved
  Test:
    Package: agentd-bin
    Filter: deployment_checklist_marks_p139_sse_sanitization_resolved
  Level: docs regression
  Test Double: source inspection
  Given docs/p0.9-deployment-checklist.md and the P139 spec
  When the known gaps section is inspected
  Then the SSE field sanitization line names P139 as the boundary sanitizer
  And it no longer says to sanitize at the SSE boundary as future work
