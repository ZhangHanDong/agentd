# AD-E2 Durable Scheduler and Worker Fleet Design

- Status: code-first candidate approved by the operator on 2026-07-17
- Depends on: AD-E1 candidate commit `287b999`
- Canonical roadmap: `docs/plans/2026-07-09-agentd-native-runtime-roadmap.md`

## Goal

Build the durable enterprise scheduler and authenticated worker control-plane
boundary required by AD-E2. Real multi-host, restart, partition, and operator
acceptance remains deferred to the cumulative AD-E checklist.

## Ownership

AgentdControlPlane owns accepted task queue state, worker enrollment and current
incarnation, leases and fencing, retry/dead-letter state, artifact upload
acknowledgements, and scheduler outbox events. Specify remains authoritative for
project snapshots, quota/RBAC policy versions, placement policy, and revocation
epochs. Workers own only live capacity and execution processes.

Legacy `agent_scheduler_queue` tickets, agent names, tmux targets, paths, and
Matrix identifiers remain compatibility inputs. They never become enterprise
task, worker, lease, or command identity.

## Contracts

1. `ProjectExecutionSnapshot` pins placement policy and revocation epoch in
   addition to existing policy references and validity.
2. `FleetSchedulerPort` accepts idempotent canonical task submissions, current
   authenticated worker heartbeats/pulls, exact lease renew/complete/fail/cancel
   reports, artifact upload acknowledgements, reaping, and explain queries.
3. Worker availability is typed: protocol range, version, zone/region, resource
   class, capabilities, slots, data classifications, image digest/signature,
   dedicated-pool membership, egress profiles, and tenant-cache isolation.
4. Migration `0018` adds enterprise queue, worker availability, scheduler
   outbox, and artifact-upload acknowledgement tables. Queue mutation, lease
   allocation, and outbox append share one SQLite transaction.
5. Pull acquisition verifies current mTLS workload incarnation, worker status,
   heartbeat freshness, protocol compatibility, available capacity, placement,
   quota, snapshot validity, and current revocation epoch before lease dispatch.
6. Completion and artifact acknowledgement require the exact current lease and
   fencing token. Duplicate requests return the existing result; stale claims
   are rejected and recorded without mutating task state.
7. Failures either enter bounded retry wait with deterministic backoff or dead
   letter. Reaping marks stale workers offline, expires their leases, and
   requeues/dead-letters tasks without reusing fencing tokens.
8. Explain output is stable structured state: current status, attempts, lease,
   worker, block/denial code, next eligibility, and policy/snapshot refs. It
   contains no secret, transcript, raw provider error, or worker path.

## Failure Model

All enterprise operations fail closed on stale identity, stale heartbeat,
unsupported protocol, unavailable authority, invalid snapshot, placement or
quota denial, stale fencing, and malformed durable state. Accepted queue and
lease changes survive restart. Network retries are idempotent by explicit keys;
wall-clock order never overrides fencing order.

## Deferred Acceptance

Focused automated tests are development feedback only. Real worker loss,
control-plane restart, partial artifact upload, network partition, quota load,
and multi-host recovery are executed once through
`docs/acceptance/ad-e-roadmap-manual-checklist.md` after AD-E1 through AD-E7
candidate implementation. Tests unset Claude credentials and start no agent.
