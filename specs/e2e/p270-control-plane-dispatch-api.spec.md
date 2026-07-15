spec: task
name: "control plane dispatch lease and fencing API"
tags: [e2e, control-plane, dispatch, lease, fencing, sqlite, enterprise, p270]
---

## Intent

Implement the P265 bounded task-ownership contract as typed core APIs and an
additive durable SQLite control-plane adapter. A dispatched task must be owned
by exactly one current worker incarnation through a new immutable `LeaseId`
and a task-scoped monotonically increasing `FencingToken`; stale, expired,
terminal, mismatched, and superseded claims must fail before task mutation.

## Decisions

- Add `LeaseId` with the canonical `ls_` plus ULID format, a non-zero unsigned
  `FencingToken`, and the closed P265 `LeaseStatus` vocabulary to
  `agentd-core`.
- Add `TaskLeasePort` with directed `dispatch`, `renew`, `release`, `cancel`,
  `validate_claim`, and `expire_due` operations. Requests carry explicit
  control-plane observation and expiry timestamps so decisions are
  deterministic and testable.
- Add migration `0015_enterprise_task_leases.sql` after P268. Store immutable
  lease history in `execution_task_leases` and the last allocated token plus
  current lease pointer in `execution_task_lease_heads`.
- Every successful dispatch allocates a fresh `LeaseId` and a token strictly
  greater than the durable head for that task before returning the active
  grant. Token gaps are allowed; reuse and rollback to an older token are not.
- Dispatch requires an existing unfinished canonical `TaskRunId` and the
  current incarnation of an online non-retired worker. A draining worker may
  renew existing work but may not receive a new dispatch.
- One unexpired active lease blocks a second dispatch. An expired lease or a
  lease owned by a superseded incarnation is terminalized before a new grant
  is allocated in the same `BEGIN IMMEDIATE` transaction.
- Renewal and worker release require the exact task, worker incarnation,
  lease id, and fencing token claim. Control-plane cancellation uses the same
  exact claim. Expiry never depends on token ordering and never reactivates a
  terminal row.
- Claim validation checks the durable head, immutable lease row, expiry, and
  current worker incarnation. Rejections return a stable typed reason for P271
  audit integration; P270 does not silently accept or mutate through an old
  claim.
- Existing `agent_scheduler_reservations`, `agent_scheduler_queue`, ticket,
  reservation id, and legacy agent values remain compatibility state. P270
  neither imports nor promotes any of them into canonical lease identity.
- P270 adds Rust APIs, SQLite persistence, and tests only. Worker pull
  transport, capacity scheduling, task lifecycle cutover, audit persistence,
  runtime binding, and compatibility dual-write remain later slices.

## Boundaries

### Allowed Changes

- specs/e2e/p270-control-plane-dispatch-api.spec.md
- docs/specs/2026-07-10-control-plane-task-lease-api.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- docs/parity/agent-chat-capability-map.md
- docs/superpowers/plans/2026-07-10-p270-control-plane-dispatch-api.md
- crates/agentd-core/src/types/ids.rs
- crates/agentd-core/src/types/enterprise.rs
- crates/agentd-core/src/types/mod.rs
- crates/agentd-core/src/ports/task_lease.rs
- crates/agentd-core/src/ports/mod.rs
- crates/agentd-core/tests/task_lease.rs
- crates/agentd-store/migrations/0015_enterprise_task_leases.sql
- crates/agentd-store/src/lib.rs
- crates/agentd-store/src/task_lease_control_plane.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-store/tests/migration_backcompat.rs
- crates/agentd-store/tests/enterprise_task_leases.rs
- crates/agentctl/tests/control_plane_dispatch_api_contract.rs
- crates/agentctl/tests/enterprise_identity_contract.rs
- crates/agentctl/tests/enterprise_store_contract.rs
- crates/agentctl/tests/project_authority_api_contract.rs
- crates/agentctl/tests/worktree_reconciliation_contract.rs

### Forbidden

- Do not alter, drop, rename, reinterpret, or backfill existing task,
  scheduler, agent, runtime, artifact, audit, project, message, relay, or Matrix
  tables and rows.
- Canonical `LeaseId`, `FencingToken`, task id, and worker incarnation id values
  derive exclusively from their control-plane allocators. Queue tickets,
  scheduler reservation ids, legacy agent ids, host/Matrix/tmux/process/path
  values, and provider resume references remain compatibility metadata only.
- Do not permit multiple active leases for one task, token reuse, token
  decrease, identity mutation, terminal lease reactivation, expiry extension
  by stale claims, or timestamp precedence over fencing tokens.
- Do not dispatch to an offline, retired, draining, unknown, or superseded
  worker incarnation.
- Do not add daemon, agentctl, HTTP, MCP, Matrix, Specify, OpenFab, worker
  network, native runtime, object upload, scheduler capacity, or task-state
  cutover behavior.
- Do not change P200-P269 command output or runtime behavior, and do not start
  Claude, Codex, tmux, Matrix, Specify, OpenFab, or remote services in tests.

## Out of Scope

- Authenticated worker registration/pull/ack/report protocols, TLS, secrets,
  capacity offers, heartbeat recovery, and remote worker operation.
- Compatibility scheduler dual-read/dual-write, ticket import, queue ordering,
  profile/capability matching, and provision-registration reconciliation.
- Runtime session/attempt lease foreign keys, native process dispatch, process
  capture/shutdown/resume, and provider-native session restoration.
- Durable stale-rejection audit events, artifact upload, usage accounting,
  certification calls, RBAC/quota enforcement, and operator diagnostics.
- Canonical execution-task state migration and replacement cutover/rollback.

## Completion Criteria

<!-- lint-ack: decision-coverage - ten scenarios cover core types/port, schema/backcompat, dispatch, conflict/retry, mutation transitions, rejection, reincarnation, concurrency, and roadmap integration. -->
<!-- lint-ack: observable-decision-coverage - outputs are typed values/errors, durable SQLite rows, constraints, and repository files bound to explicit tests. -->
<!-- lint-ack: output-mode-coverage - P270 adds no CLI or network output mode; domain and persistence outcomes are tested directly. -->
<!-- lint-ack: boundary-entry-point - scenarios bind the core type/port, migration, SQLite adapter, concurrency boundary, and documentation artifacts. -->
<!-- lint-ack: bdd-rule-grouping - every scenario proves one control-plane dispatch/lease/fencing slice. -->

Scenario: task lease types and port preserve the P265 identity contract
  Test:
    Package: agentd-core
    Filter: task_lease_types_and_port_preserve_p265_contract
  Level: core domain unit
  Test Double: recording task lease port
  Given canonical task lease ids statuses fencing tokens and port requests
  When their syntax serialization terminal predicates and method values are inspected
  Then generated lease ids use `ls_` plus a ULID payload
  And zero fencing tokens are rejected while positive tokens round trip
  And only `active` is nonterminal
  And a recording port receives every typed claim unchanged

Scenario: migration creates constrained durable lease tables
  Test:
    Package: agentd-store
    Filter: migration_creates_constrained_task_lease_tables
  Level: SQLite migration integration
  Test Double: temporary SQLite database
  Given a newly migrated store
  When schema version tables foreign keys indexes and triggers are inspected
  Then schema version is `15`
  And lease ids statuses positive tokens parent references and terminal fields are constrained
  And at most one active lease exists per task
  And lease identity terminal history head history and token monotonicity reject direct mutation

Scenario: lease migration preserves enterprise and compatibility rows
  Test:
    Package: agentd-store
    Filter: task_lease_migration_preserves_enterprise_and_compatibility_rows
  Level: SQLite migration backcompat
  Test Double: raw in-memory SQLite with real migration files
  Given representative P267 P268 task worker scheduler artifact and audit rows after migration 0014
  When migration 0015 is applied
  Then every representative existing row remains byte-equivalent on selected columns
  And both new lease tables start empty
  And scheduler tickets and reservation ids remain absent from canonical lease rows
  And schema version becomes `15`

Scenario: dispatch binds an unfinished task to the current online incarnation
  Test:
    Package: agentd-store
    Filter: dispatch_binds_current_worker_and_allocates_first_fencing_token
  Level: control-plane SQLite integration
  Test Double: temporary SQLite database
  Given an unfinished canonical task and a current incarnation of an online worker
  When directed dispatch is requested with a bounded future expiry
  Then one active grant binds the exact task and incarnation
  And it has a new canonical lease id and fencing token `1`
  And the durable head points to that lease
  And unknown finished malformed or non-online targets are rejected before a grant is exposed

Scenario: conflict expiry and reacquisition allocate new monotonic grants
  Test:
    Package: agentd-store
    Filter: active_conflict_and_reacquisition_allocate_new_monotonic_grants
  Level: control-plane SQLite integration
  Test Double: temporary SQLite database
  Given a task with one active unexpired lease
  When another dispatch is requested before and after release or expiry
  Then the unexpired request returns conflict without changing the active grant
  And each later successful dispatch has a different lease id
  And each later fencing token is strictly greater than every earlier token
  And no terminal lease is reactivated

Scenario: renew release and cancel require the exact current claim
  Test:
    Package: agentd-store
    Filter: renew_release_and_cancel_require_exact_current_claim
  Level: control-plane SQLite integration
  Test Double: temporary SQLite database
  Given an active lease and its exact task worker lease and token claim
  When renewal release or cancellation is requested
  Then exact renewal only extends expiry and increments record version
  And exact release or cancellation creates the requested terminal status and clears the head
  And mismatched task worker lease or token claims are rejected without mutation
  And terminal rows reject renewal and transition

Scenario: stale terminal and expired claims cannot authorize mutation
  Test:
    Package: agentd-store
    Filter: stale_terminal_and_expired_claims_are_rejected
  Level: control-plane SQLite integration
  Test Double: temporary SQLite database
  Given old-token terminal and elapsed active lease claims
  When `validate_claim` is called before a worker mutation
  Then old tokens return `stale_fencing_token`
  And terminal leases return `terminal_lease`
  And elapsed leases are durably expired and return `lease_expired`
  And none of those claims is returned as authorized

Scenario: worker reincarnation fences the old lease before redispatch
  Test:
    Package: agentd-store
    Filter: worker_reincarnation_supersedes_old_lease_before_new_dispatch
  Level: control-plane SQLite integration
  Test Double: temporary SQLite database
  Given an active lease owned by one current worker incarnation
  When a new incarnation registers for the same stable worker
  Then the old claim returns `stale_worker_incarnation`
  And redispatch terminalizes the old lease as `superseded`
  And the new incarnation receives a fresh lease id and greater token
  And the old incarnation cannot renew release or validate the new grant

Scenario: concurrent dispatch exposes one active grant and unique token
  Test:
    Package: agentd-store
    Filter: concurrent_dispatch_has_one_active_lease_and_unique_token
  Level: SQLite transaction concurrency integration
  Test Double: temporary SQLite database with two pool connections
  Given two simultaneous dispatch requests for the same task and current incarnation
  When both transactions contend for the lease head
  Then exactly one request returns an active grant
  And the other returns active-lease conflict
  And storage contains one active lease one unique token and one matching current head

Scenario: roadmap and parity record P270 without claiming worker protocol cutover
  Test:
    Package: agentctl
    Filter: p270_roadmap_and_parity_record_durable_lease_api_without_claiming_worker_protocol
  Level: artifact inspection
  Test Double: repository Markdown and Rust files
  Given P270 core migration and control-plane tests pass
  When roadmap and parity evidence are inspected
  Then they reference P270 `TaskLeasePort` migration 0015 and durable fencing
  And durable task leases remain partial pending worker protocol audit integration and compatibility cutover
  And scheduler tickets remain documented as noncanonical compatibility input
  And Immediate Next Step advances to P271 artifact audit usage and certification-reference API
