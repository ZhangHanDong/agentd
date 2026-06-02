spec: task
name: "Live run start — POST /runs + agentctl run start over the daemon"
tags: [e2e, p0, p0.9, run-start, http]
---

## Intent

Wire the live run-start path (P0.9 9c): the daemon's `POST /runs` creates a run
from a flow and drives it to its first park; `agentctl run start` (without
`--dry-run`) posts to the daemon instead of the P0.8 "deferred" error. This is
the standalone trigger reframed for Path B (`/run start <issue>` → a local CLI
POST). Real agents then advance the run; that, and a live daemon, are the
deployment checklist — offline tests the POST route over the production host and
agentctl's clean failure when no daemon is reachable.

## Decisions

- `POST /runs` accepts `{flow, run_id, context?}` and returns `201` with `{run_id, status}` (the initial `RunProgress` kind: `parked`/`finished`/…); the host's `start_workflow` resolves `flow`→`<flow>.dot`, records the run (`workflow_path`+sha), and executes to the first park, emitting `run_parked`.
- An unknown `flow` is an error response (not a panic).
- `agentctl run start --flow F <id> [--daemon-url URL]` (no `--dry-run`) POSTs `{flow, run_id, context}` to `URL/runs` (default `http://127.0.0.1:8787`); a daemon that cannot be reached is a clean non-zero error, never a hang. `--dry-run` still validates + prints the plan locally.

## Boundaries

### Allowed Changes

- crates/agentd-surface/**
- crates/agentd-bin/**
- crates/agentctl/**
- specs/e2e/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not pull agentd-store into agentd-surface (P0.7 D2 — `start_workflow` is a trait method; the store work is in the agentd-bin impl).

## Out of Scope

- A real running daemon / real agents advancing the run / the rmcp wire (deployment checklist).
- Seeding the initial `context` into the run (the shipped workflows use fixed paths; a real-env gap).

## Completion Criteria

Scenario: POST /runs creates and starts a draft run
  Test: post_runs_creates_and_starts_a_draft_run
  Given the daemon router over a production host on a real store
  When POST /runs with {flow: "draft", run_id: "r1"} is requested
  Then the response is 201, the run is parked, and the run's emitted events include run_parked

Scenario: POST /runs with an unknown flow is an error
  Test: post_runs_unknown_flow_is_error
  Given the daemon router over a production host
  When POST /runs with an unknown flow is requested
  Then the response status is an error (not 2xx)

Scenario: agentctl run start posts to the daemon and reports success
  Test: run_start_live_posts_and_reports_success
  Given a daemon that replies 201 to POST /runs
  When `agentctl run start --flow draft --daemon-url <it> <id>` is invoked
  Then it exits 0 and stdout reports the run started

Scenario: agentctl run start without a reachable daemon fails cleanly
  Test: run_start_live_unreachable_daemon_errors_cleanly
  Given no daemon listening at the given URL
  When `agentctl run start --flow draft --daemon-url <closed> <id>` is invoked
  Then it exits non-zero and stderr reports that the daemon cannot be reached
