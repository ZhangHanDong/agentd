spec: task
name: "AD-E2 authenticated worker fleet recovery"
tags: [e2, worker-fleet, heartbeat, recovery, lease]
---

## Intent

Provide a durable cross-process worker boundary in which authenticated workers
register an incarnation, heartbeat, pull only while online, and recover safely
after disconnect. Offline recovery must expire active leases and prevent stale
incarnations from publishing outcomes or acquiring new work.

## Decisions

- HTTP and mTLS transports call the same `WorkerFleetPort`; transport code does
  not own lease or worker state.
- Worker incarnation identity is durable and current-incarnation checks fence
  superseded workers.
- Heartbeat cutoff marks workers offline; the supervisor also expires due leases
  and retries native recovery through the durable recovery registry.
- Draining workers finish existing leases but cannot pull new leases.
- Authentication is fail-closed when proofs are explicitly configured.

## Boundaries

### Allowed Changes

- crates/agentd-core/src/ports/worker_fleet.rs
- crates/agentd-store/src/worker_fleet.rs
- crates/agentd-store/src/worker_repo.rs
- crates/agentd-store/src/task_lease_control_plane.rs
- crates/agentd-bin/src/daemon.rs
- crates/agentd-surface/src/worker_fleet_http.rs
- crates/agentd-surface/src/worker_fleet_mtls_http.rs
- crates/agentd-bin/tests/**
- crates/agentd-store/tests/**
- specs/e2e/ad-e2-worker-fleet-recovery.spec.md

### Forbidden

- Do not execute Claude, tmux, Matrix, Robrix, or remote services in tests.
- Do not accept stale incarnations as current workers.
- Do not let draining/offline workers acquire new leases.
- Do not bypass the durable lease/fencing control plane in transports.

## Completion Criteria

Scenario: authenticated registration and heartbeat share the durable boundary
  Test:
    Package: agentd-store
    Filter: worker_fleet
  Given a configured worker proof and a new worker incarnation
  When the worker registers and heartbeats through the fleet port
  Then the registration and last-seen state are durable

Scenario: draining workers cannot pull new leases
  Test:
    Package: agentd-store
    Filter: worker_fleet
  Given a current worker transitioned to draining
  When it pulls work
  Then the pull is rejected without creating an active lease

Scenario: stale incarnation cannot mutate current lease state
  Test:
    Package: agentd-store
    Filter: enterprise_task_leases
  Given an old worker incarnation after reincarnation supersession
  When it renews or reports evidence
  Then the operation is rejected by fencing

Scenario: disconnected workers are recovered durably
  Test:
    Package: agentd-store
    Filter: worker_fleet
  Given a worker whose heartbeat is older than the cutoff and an expired lease
  When the maintenance tick runs
  Then the worker is offline and the lease is terminalized

Scenario: worker startup and heartbeat reconnect after a transient daemon outage
  Test:
    Package: agentd-bin
    Filter: worker_fleet_client::tests::transient_http_statuses_trigger_reconnect
  Given a worker whose daemon endpoint returns a transient 5xx, 408, or 429
  When registration or heartbeat uses the bounded retry policy
  Then the client classifies the response as unavailable and retries with
       exponential backoff
  And authentication, stale-incarnation, and other business conflicts are not
      retried

## Out of Scope

- Cloud worker orchestration and autoscaling.
- Dashboard, Matrix, Robrix presentation and cutover.
- Remote object storage.
