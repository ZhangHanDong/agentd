spec: task
name: "Per-run delivery serialization — close the concurrent-verdict double-advance (P2 Foundation A)"
tags: [e2e, daemon, p2, concurrency, runhost]
---

## Intent

Serialize the daemon's run-advancing operations PER RUN so concurrent events for
one run can't double-advance it. Verified (2026-06-07) against the engine: the
engine is strictly sequential within a call, but `ProductionRunHost` is a single
shared instance whose `deliver` is called CONCURRENTLY, with no per-run guard. The
shipped N-reviewer review sends N concurrent `submit_review → deliver`: all N
resolve the same review park (`lookup_park_by_review_run` gated `count < expected`)
before any `insert_review_verdict`, then all see `collected == expected` and all
return `Done` → N concurrent `run_loop`s advance one run → double `gh pr create` /
double-finish / (via `goal_gate_unmet → implement`) multiple writer task_runs.

This is LATENT today — the rmcp/MCP wire that lets agent processes reach `deliver`
concurrently is deployment-deferred — but certain once it lands (the real-agent
path). It is also the hard prerequisite for the P2 C1 per-run worktree (which
assumes ≤1 open writer task_run per run). Fixing the root (serialize delivery)
satisfies C1 AND closes the race; per-task_run worktree would dodge the assumption
but leave the race.

## Decisions

- `ProductionRunHost` holds a per-run lock registry:
  `run_locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>`.
  A private `run_lock(run_id) -> Arc<tokio::sync::Mutex<()>>` returns the SAME
  `Arc` for a given run id (so concurrent callers contend) and a DIFFERENT one for
  a different run (so cross-run delivery stays concurrent). The std mutex guards
  only the map insert/lookup and is never held across an `.await`.
- `deliver` resolves the run id via `run_for_event` (a read-only resolve — safe
  unlocked), then acquires the per-run async lock and holds it across
  `resolve_graph → engine.deliver_event → emit`. The mutation
  (`insert_review_verdict`) thus runs under the lock; a serialized later caller's
  `engine.deliver_event` re-resolves the gate and sees the prior insert
  (relies on sqlx/SQLite autocommit read-after-commit visibility), so exactly one
  caller advances and the rest re-park.
- `start_run` takes the same per-run lock around `resolve_graph → execute → emit`,
  so every run-advancing entry point on one run is mutually exclusive (defensive —
  the verified race is in `deliver`, but the serialization is uniform).
- The lock lives in `ProductionRunHost` (agentd-bin), NOT agentd-core (D1 intact):
  the engine is already sequential; the concurrency is the daemon's.

## Boundaries

### Allowed Changes

- crates/agentd-bin/**
- specs/e2e/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1 — this foundation is
  daemon-only; D1 is lifted later, at C1/C2).
- Do not pull agentd-store into agentd-surface (P0.7 D2).

## Out of Scope

- Lock-entry eviction: ONE entry per distinct run id accumulates (bounded by run
  volume). Eviction is deferred — evict-on-terminal is safe only because
  post-terminal events are `Ignored`, but it adds a fresh-mutex re-create window
  not worth it for this foundation.
- Wiring the rmcp/MCP server into the daemon (the deployment path that makes the
  race reachable) — separate, and the serialization lands in the shared host the
  future wiring will use.
- Cross-process / multi-daemon serialization (single-daemon MVP).

## Completion Criteria

Scenario: the per-run lock registry returns one lock per run
  Test: run_lock_is_per_run
  Given a ProductionRunHost
  When run_lock is called twice for the same run id and once for a different run id
  Then the two same-run locks are the same Arc and the different-run lock is a different Arc

Scenario: concurrent review verdicts advance the run exactly once
  Test: concurrent_review_verdicts_advance_run_once
  Given a shared ProductionRunHost with an execute.dot run driven to its review park
  When the three reviewers submit pass verdicts CONCURRENTLY on the shared host
  Then the run reaches finished with exactly one run_finished event (it advanced once, not N times)
