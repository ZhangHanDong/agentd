# Control-Plane Task Lease API

- **Date**: 2026-07-10
- **Status**: P270 implementation design
- **Authority basis**: P264 ownership and P265 runtime/worker identity contract
- **Scope**: directed dispatch, bounded leases, fencing, renewal, expiry, and worker reincarnation

## 1. Boundary

`AgentdControlPlane` is the only authority that allocates a canonical task
lease. A lease is not a queue ticket, scheduler reservation, worker process,
runtime session, or transport acknowledgement. It is one durable bounded grant
for one existing `TaskRunId` to one current `WorkerIncarnationId`.

P270 exposes this boundary as `TaskLeasePort` and implements it with SQLite. It
does not add a worker network endpoint or connect compatibility scheduling to
the enterprise API.

## 2. Core Contract

The active grant tuple is:

```text
(TaskRunId, WorkerIncarnationId, LeaseId, FencingToken)
```

`LeaseId` is `ls_` plus a ULID and is immutable. `FencingToken` is a non-zero
unsigned integer whose ordering is scoped to one task. Every dispatch creates
a new lease id and increments the task's durable high-water mark before the
grant is returned.

The closed lease states are `active`, `released`, `expired`, `cancelled`, and
`superseded`. Only `active` is nonterminal. Every terminal transition records
its time and reason and can never return to active.

`TaskLeasePort` provides:

- `dispatch`: grant an unfinished task to a current online incarnation.
- `renew`: extend an exact active claim to a strictly later expiry.
- `release`: worker completion of ownership without declaring task success.
- `cancel`: control-plane cancellation of the exact active ownership grant.
- `validate_claim`: authorize a worker mutation only for the exact current,
  unexpired, active tuple.
- `expire_due`: durably terminalize every elapsed active lease at an explicit
  observation timestamp.

Requests carry control-plane timestamps. Worker-provided wall clocks are not
authority and timestamp ordering never wins over a fencing-token mismatch.

## 3. Durable Model

Migration `0015_enterprise_task_leases.sql` adds:

- `execution_task_leases`: immutable identity/parent/token history, bounded
  expiry, closed state, terminal reason, and record version.
- `execution_task_lease_heads`: one row per task with the greatest allocated
  fencing token and the current active lease pointer.

Foreign keys bind leases to existing `task_runs` and P267
`worker_incarnations`. A partial unique index permits at most one active lease
per task. Triggers prevent identity changes, token rollback, row deletion, and
terminal state mutation. Repository transactions keep the head pointer and
lease state synchronized.

SQLite allocation uses `BEGIN IMMEDIATE`. The transaction validates task and
worker state, reconciles an elapsed or superseded active row, increments the
head, inserts the new lease, and updates the current pointer before commit.
Concurrent dispatches therefore serialize at the task grant boundary; one can
win and the other observes an active-lease conflict.

Token gaps are valid after a rolled-back or reserved allocation. Reusing or
decreasing a token is invalid.

## 4. Worker and Task Rules

New dispatch requires:

1. A syntactically canonical existing `TaskRunId` with `finished_at IS NULL`.
2. A syntactically canonical current `WorkerIncarnationId`.
3. A stable worker in `online` state.
4. `expires_at > observed_at`.
5. No current unexpired active lease.

A `draining` worker may renew an existing claim so in-flight work can finish,
but cannot receive new work. Offline and retired workers have no current
incarnation under P267 and cannot dispatch or renew.

P270 deliberately does not reinterpret the unconstrained compatibility
`task_runs.status` column as the complete P265 task state machine. The
unfinished marker is the existing `finished_at IS NULL` invariant; canonical
task lifecycle migration is a later cutover slice.

When a worker registers a new incarnation, the prior incarnation immediately
fails claim validation. The next claim validation or dispatch transaction
marks its active lease `superseded`; a new dispatch then receives a new lease
id and a greater token.

## 5. Rejection Contract

Claim rejection has stable reasons:

- `claim_mismatch`: tuple values do not identify the persisted grant.
- `not_current_lease`: the lease is not the task head's current pointer.
- `stale_fencing_token`: the supplied token is older than the durable task
  high-water mark or differs from the immutable lease token.
- `terminal_lease`: the lease is already terminal.
- `lease_expired`: the active lease elapsed and was durably expired.
- `stale_worker_incarnation`: the owner is no longer its worker's current
  incarnation.

P270 returns these reasons from the control-plane API. P271 will append the
corresponding rejection to the P268 audit store. Absence of P271 integration
does not permit a rejected mutation to proceed.

## 6. Compatibility and Follow-On Work

`agent_scheduler_reservations.id`, `agent_scheduler_queue.ticket`, the P265
source field `dispatch_queue.ticket`, legacy dispatch JSON, and agent ids
remain compatibility values. P270 does not parse, copy, import, hash, or
compare them into lease identity.

P271 adds control-plane artifact/audit/usage/certification-reference APIs.
P273 reconciles scheduler provision registration. P278 adds authenticated
worker pull, acknowledgement, durable recovery, and lease reports. Until those
slices land, P270 is durable control-plane API evidence rather than a complete
agent-chat replacement or worker-fleet cutover.
