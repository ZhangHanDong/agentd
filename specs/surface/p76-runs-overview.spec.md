spec: task
name: "GET /runs overview — the at-a-glance latest-state view (mosh state-sync)"
tags: [surface, http, p1, dashboard, herdr]
---

## Intent

P0.7/P0.9 expose per-run reads (`GET /runs/:id` snapshot, `GET /runs/:id/events`
log). This P1 task adds the at-a-glance overview a dashboard actually wants
(herdr's "every agent at a glance: blocked / working / done"): `GET /runs` lists
ALL runs with their current status — the mosh state-sync idea (the latest
authoritative state, not an event replay). It is reached through a new
`RunHost::list_runs` seam so `agentd-surface` stays store-free (P0.7 D2); the
production impl reads `run_repo`. agentd-core stays frozen (D1).

## Decisions

- `RunHost::list_runs()` returns `Vec<RunSummary { run_id, status, current_node, started_at }>` — one entry per run, most-recently-started first.
- `run_repo::list_runs(pool)` reads `SELECT id, status, current_node, started_at FROM runs ORDER BY started_at DESC` (no new columns/migration; the `runs` table already has them).
- `GET /runs` returns `200` with a JSON array of those summaries (`current_node` is `null` for a run with no checkpoint yet). An empty store returns `200` with `[]`.
- A store failure surfaced through `list_runs` returns `500`.

## Boundaries

### Allowed Changes

- crates/agentd-surface/**
- crates/agentd-bin/**
- crates/agentd-store/src/run_repo.rs
- specs/surface/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not pull agentd-store into agentd-surface (P0.7 D2 — the overview is reached via the `RunHost` seam; the store read is the agentd-bin impl).

## Out of Scope

- Pagination / filtering / status-count aggregation (the MVP returns the full list).
- A live-updating overview (this is a point-in-time snapshot; live updates are the SSE tail, p75).

## Completion Criteria

Scenario: GET /runs lists every run with its current status
  Test: get_runs_lists_all_runs
  Given a host with two runs, one "running" and one "finished"
  When GET /runs is requested
  Then the response is 200 and the JSON array contains both run ids with their statuses

Scenario: GET /runs on an empty store is a 200 empty array
  Test: get_runs_empty_is_empty_array
  Given a host with no runs
  When GET /runs is requested
  Then the response is 200 and the body is an empty JSON array

Scenario: list_runs over the real store reflects each run's distinct status
  Test: production_list_runs_reflects_statuses
  Given a production host with a started run and a separately-recorded run marked finished
  When list_runs is called
  Then it returns both runs, most-recently-started first, with their statuses

Scenario: a store failure surfaces as 500
  Test: get_runs_store_error_is_500
  Given a host whose list_runs returns an error
  When GET /runs is requested
  Then the response status is 500
