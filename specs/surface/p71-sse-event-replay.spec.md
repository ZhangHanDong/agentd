spec: task
name: "Event persistence + replay cursor (event_repo) for SSE"
tags: [surface, store, mvp, p0, sse]
---

## Intent

The HTTP+SSE surface (7b Task 5) must let a late dashboard replay a run's events
from where it left off. That rests on an append-only `events` log (the P0.2
table) and a cursor read. This task lands `event_repo` in `agentd-store`:
`append` writes one row and returns its monotonically increasing `seq`;
`read_from` returns a run's events after a `seq` cursor, in order — the SSE
replay primitive. The actual emit (one event per `RunProgress`) and the axum SSE
endpoint are the daemon's / Task 5's job.

## Decisions

- `event_repo::append(run_id, kind, payload)` inserts into `events` (`emitted_at` = now), returning the new `seq` (the autoincrement PK). `seq` is strictly increasing across a database.
- `event_repo::read_from(run_id, after_seq)` returns that run's events with `seq > after_seq`, ordered by `seq` ascending, as `EventRow { seq, run_id, kind, payload, emitted_at }` — the SSE replay cursor (uses `idx_events_run_seq`).
- `events.run_id` is a foreign key to `runs(id)`; appending an event for a run that does not exist is a `StoreError`.

## Boundaries

### Allowed Changes

- crates/agentd-store/**
- crates/agentd-surface/**
- specs/surface/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not reference, open, or write mempal's on-disk database (MCP-only, §3.1).

## Out of Scope

- The emit point (one event per RunProgress — the daemon, P0.9); the axum SSE endpoint (Task 5).

## Completion Criteria

Scenario: append assigns increasing seq and read_from(0) returns them in order
  Test: events_append_and_read_from_zero
  Given a store with an inserted run and three appended events
  When read_from is called with cursor 0
  Then it returns the three events in seq order with strictly increasing seq

Scenario: read_from a cursor skips earlier events
  Test: events_read_from_cursor_skips_earlier
  Given a store with three appended events for a run
  When read_from is called with the first event's seq as the cursor
  Then it returns only the two later events

Scenario: read_from for a different run is empty
  Test: events_read_from_other_run_is_empty
  Given a store with events for run "r1"
  When read_from is called for run "r2"
  Then it returns no events

Scenario: appending an event for an unknown run is an error
  Test: events_append_unknown_run_is_error
  Given a store with no run "ghost"
  When append is called for run "ghost"
  Then it returns Err
