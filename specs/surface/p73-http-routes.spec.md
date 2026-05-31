spec: task
name: "HTTP routes + SSE event replay (axum surface)"
tags: [surface, http, sse, mvp, p0]
---

## Intent

The daemon's observability surface (design §7.2): a small axum `Router` a
dashboard polls for run state and tails for events. This task lands `http.rs` —
`GET /healthz`, `GET /runs/:id` (the `query_run` snapshot as JSON, 404 when
unknown), and `GET /runs/:id/events?from_seq=N` (a finite SSE replay of events
after a `seq` cursor). Every route is a handler over the `RunHost` seam, so the
whole surface is driven in-process by `tower::oneshot` against a `FakeRunHost`
— no socket, no real engine, no store. Wiring the router into a listening daemon
is P0.9.

## Decisions

- `router(AppState { host })` builds the `Router`; `AppState` holds only `Arc<dyn RunHost>` (the surface stays store-free — the production host maps `agentd-store` event rows to `EventRecord` in P0.9).
- `GET /healthz` returns `200` with body `ok`.
- `GET /runs/:id` calls `query_run` over the host: `Ok` → `200` JSON `{status, current_node, completed_nodes, context}`; the `not_found` (`NotFound`) error → `404`.
- `GET /runs/:id/events?from_seq=N` returns a `text/event-stream` replaying the host's events with `seq > from_seq`, each as one SSE frame (`id` = seq, `event` = kind, `data` = payload), then ends the stream. `from_seq` defaults to `0` when absent.
- The replay stream is FINITE (`futures::stream::iter`, no keep-alive); a non-integer `from_seq` is rejected by the query extractor with `400`.

## Boundaries

### Allowed Changes

- crates/agentd-surface/**
- specs/surface/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not add an `agentd-store` dependency to `agentd-surface` (the surface reads events through the `RunHost` seam, not the store directly).

## Out of Scope

- A live event tail (streaming new events as they are emitted) — v0 replays the existing log then ends; live tail + the emit point are P0.9.
- Binding the router to a TCP listener / running the daemon (P0.9).

## Completion Criteria

Scenario: healthz returns 200 ok
  Test: http_healthz_ok
  When GET /healthz is requested
  Then the response is 200 with body "ok"

Scenario: GET a run returns its snapshot as JSON
  Test: http_get_run_returns_snapshot
  Given a host with a snapshot for run "r1" whose status is "parked"
  When GET /runs/r1 is requested
  Then the response is 200 and the JSON body status is "parked"

Scenario: GET an unknown run is 404
  Test: http_get_run_unknown_is_404
  Given a host with no snapshot for run "ghost"
  When GET /runs/ghost is requested
  Then the response is 404

Scenario: SSE replays events after the from_seq cursor
  Test: http_sse_replays_from_cursor
  Given a host with events seq 1 "run.started" and seq 2 "node.parked" for run "r1"
  When GET /runs/r1/events?from_seq=1 is requested
  Then the response is 200 and the body contains "node.parked" but not "run.started"

Scenario: SSE with no events is an empty 200 stream
  Test: http_sse_empty_when_no_events
  Given a host with no events for run "r1"
  When GET /runs/r1/events is requested
  Then the response is 200 and the body contains no event frame

Scenario: a non-integer from_seq is rejected with 400
  Test: http_sse_invalid_from_seq_is_400
  When GET /runs/r1/events?from_seq=notanumber is requested
  Then the response is 400
