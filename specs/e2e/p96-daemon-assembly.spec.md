spec: task
name: "Daemon assembly — the production host behind the HTTP/SSE router"
tags: [e2e, p0, p0.9, daemon, http]
---

## Intent

Prove the daemon (P0.9 9b) assembles the production `RunHost` behind the
`agentd-surface` HTTP/SSE router so a dashboard can observe runs. The router is
the same one shipped + unit-tested in P0.7 (p73); this verifies it works over the
PRODUCTION host on a real `SqliteStore`, driven in-process by `tower::oneshot`
(no socket bind — that, plus real agents, is the deployment checklist).

## Decisions

- `daemon::build_router(host: Arc<dyn RunHost>)` returns the `agentd-surface` router with `AppState { host }`; `daemon::serve(config)` binds a TCP listener and serves it (the bind/serve path is exercised only on a real box).
- `build_production_host(config)` opens the `SqliteStore` (migrations apply), and constructs the host with the real `TmuxBackend` + the offline mempal + `SystemClock`; constructing the host does not spawn tmux.
- Over the assembled router: `GET /healthz` → 200 `ok`; `GET /runs/:id` → the run's snapshot JSON (its `current_node` after a park); `GET /runs/:id/events` → the run's emitted events.

## Boundaries

### Allowed Changes

- crates/agentd-bin/**
- crates/agentd-store/src/run_repo.rs
- specs/e2e/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not pull agentd-store into agentd-surface (P0.7 D2).

## Out of Scope

- A real TCP listener bind / real agent spawn / real tmux (deployment checklist).

## Completion Criteria

Scenario: the assembled router serves healthz
  Test: daemon_router_healthz_ok
  Given the daemon router built over a production host on a real store
  When GET /healthz is requested
  Then the response is 200 with body "ok"

Scenario: the assembled router serves a started run's snapshot
  Test: daemon_router_serves_run_snapshot
  Given a production host with a draft.dot run started to its propose_spec park
  When GET /runs/r1 is requested
  Then the response is 200 and the JSON current_node is "propose_spec"

Scenario: the assembled router streams a run's emitted events
  Test: daemon_router_streams_run_events
  Given a production host with a draft.dot run started to its first park
  When GET /runs/r1/events is requested
  Then the response is 200 and the body contains "run_parked"
