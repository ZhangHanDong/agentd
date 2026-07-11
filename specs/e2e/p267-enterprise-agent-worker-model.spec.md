spec: task
name: "enterprise agent worker runtime store model"
tags: [e2e, store, sqlite, identity, agent, worker, runtime, enterprise, p267]
---

## Intent

Implement the first enterprise execution schema from the P264-P266 contracts:
typed agent-profile/worker/runtime identities, additive SQLite records, and
transactional store repositories. Preserve all base behavior while proving
that worker reincarnation and runtime attempts never reuse legacy agent, host,
tmux, PID, path, or Matrix identities.

## Decisions

- Add migration `0013_enterprise_agent_worker_runtime.sql` after the base
  `0012_matrix_bridge_contract.sql`, with six additive tables:
  `agent_profiles`, `legacy_agent_aliases`, `workers`,
  `worker_incarnations`, `runtime_sessions`, and `runtime_attempts`. Update
  `schema_meta` to version `13` without altering existing tables or rows.
- Add `AgentProfileId`, `WorkerId`, `WorkerIncarnationId`, `RuntimeSessionId`,
  and `RuntimeAttemptId` newtypes to `agentd-core`, preserving P265 prefixes and
  ULID generation. Add closed status enums for profile, worker, runtime session,
  and runtime attempt with exact string round trips.
- SQL and repository validation require canonical prefix plus a valid 26-char
  ULID payload for every new id. Invalid ids fail before mutation; legacy ids
  enter only through `legacy_agent_aliases`.
- `agent_profiles` stores reusable role/capability/runtime/model/prompt metadata,
  closed `active|disabled|retired` state, record version, and timestamps.
  `retired` cannot reactivate.
- `legacy_agent_aliases` maps an existing `agents.id` to one
  `AgentProfileId`. It copies no runtime-handle fields and changes no existing
  registry reads or writes.
- `workers` stores stable enrollment/status/trust metadata.
  `worker_incarnations` stores daemon/host/zone/capability metadata and enforces
  at most one current incarnation per worker. Registration transactionally
  supersedes the prior current incarnation.
- Current-incarnation heartbeat returns `Accepted`; a known superseded
  incarnation returns `Stale` without mutation; unknown ids return `NotFound`.
  A retired worker cannot register a new incarnation.
- `runtime_sessions` binds one existing `TaskRunId`, one `AgentProfileId`, and
  the opaque P266 snapshot authority/kind/id/version/hash tuple. It uses exact
  P265 states and never mutates identity or snapshot fields.
- `runtime_attempts` binds one runtime session to one current worker
  incarnation and stores process/runtime locators as metadata. A session has at
  most one current attempt; resume requires `resume_pending`, marks the prior
  attempt `gone`, and keeps the same session id.
- P267 adds core/store APIs and tests only. It adds no HTTP/CLI/MCP endpoint,
  worker protocol, scheduler lease, project authority storage, native runtime,
  automatic backfill, or replacement-readiness claim.

## Boundaries

### Allowed Changes

- specs/e2e/p267-enterprise-agent-worker-model.spec.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- docs/parity/agent-chat-capability-map.md
- docs/superpowers/plans/2026-07-10-p267-agent-worker-store-model.md
- crates/agentd-core/src/types/ids.rs
- crates/agentd-core/src/types/enterprise.rs
- crates/agentd-core/src/types/mod.rs
- crates/agentd-core/tests/enterprise_identity.rs
- crates/agentd-store/migrations/0013_enterprise_agent_worker_runtime.sql
- crates/agentd-store/src/lib.rs
- crates/agentd-store/src/agent_profile_repo.rs
- crates/agentd-store/src/worker_repo.rs
- crates/agentd-store/src/runtime_session_repo.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-store/tests/migration_backcompat.rs
- crates/agentd-store/tests/enterprise_agent_profiles.rs
- crates/agentd-store/tests/enterprise_workers.rs
- crates/agentd-store/tests/enterprise_runtime_sessions.rs
- crates/agentctl/tests/enterprise_store_contract.rs

### Forbidden

- Do not alter, drop, or rename current tables/columns, and do not rewrite or
  delete existing `agents`, project, message, task, scheduler, relay, or Matrix
  rows.
- Do not add project/organization/team/repository/room/spec/policy authority
  tables or reinterpret base `projects` as authority state.
- Do not add dependencies, HTTP routes, agentctl commands, MCP tools, Matrix
  handlers, daemon composition, worker network protocol, scheduler lease APIs,
  or native runtime behavior.
- Do not store host names, backend/tmux locators, PIDs, workdirs, native resume
  refs, Matrix ids, or legacy agent ids in canonical id columns.
- Do not permit two current worker incarnations or two current runtime attempts
  for the same parent.
- Do not change P200-P266 command output, runtime behavior, or parity gate exit
  semantics. Tests must not start Claude, Codex, tmux, Matrix, Specify, OpenFab,
  or remote services.

## Out of Scope

- Lease/fencing tables and scheduler queue migration.
- Worker HTTP protocol, authentication, secrets, TLS, capacity scheduling,
  drain orchestration, or remote artifact upload.
- Runtime process spawn/status/capture/shutdown/resume adapters, native PTY
  hosting, and transcript persistence.
- ProjectAuthorityPort adapter/cache implementation and snapshot signature validation.
- Automatic alias discovery, dual-write, API cutover, and rollback.

## Completion Criteria

<!-- lint-ack: decision-coverage — the ten scenarios cover typed ids/states, schema/backcompat, profile/alias lifecycle, worker stale/retired failures, runtime resume/stale/terminal failures, and roadmap integration. -->
<!-- lint-ack: observable-decision-coverage — P267 outputs are SQLite rows, repository results/errors, typed values, and roadmap/parity files, all bound to tests. -->
<!-- lint-ack: output-mode-coverage — P267 adds no CLI/network output modes; store success/failure and persisted state are covered directly. -->
<!-- lint-ack: boundary-entry-point — scenarios separately bind core types, migration/backcompat, all three repository modules, and documentation integration. -->
<!-- lint-ack: bdd-rule-grouping — all scenarios prove the single additive enterprise agent/worker/runtime store model. -->

Scenario: enterprise ids and statuses are distinct typed contracts
  Test:
    Package: agentd-core
    Filter: enterprise_identity_ids_and_states_are_distinct_and_round_trip
  Level: core unit
  Test Double: none
  Given generated profile, worker, incarnation, session, and attempt ids
  When ids and all status variants are serialized to contract strings
  Then each id has its P265 prefix and a distinct Rust type
  And every status string round-trips to the same enum variant
  And each terminal-state predicate matches P265

Scenario: migration creates constrained enterprise identity tables
  Test:
    Package: agentd-store
    Filter: migration_creates_enterprise_agent_worker_runtime_tables
  Level: SQLite migration integration
  Test Double: temporary SQLite database
  Given a newly migrated store
  When schema version, tables, checks, foreign keys, and unique indexes are inspected
  Then all six P267 tables exist and schema version is `13`
  And canonical id/status constraints reject invalid direct inserts
  And one-current-incarnation and one-current-attempt indexes exist

Scenario: enterprise migration preserves base rows
  Test:
    Package: agentd-store
    Filter: enterprise_identity_migration_preserves_base_rows
  Level: SQLite migration backcompat
  Test Double: raw in-memory SQLite with real migration files
  Given representative base rows after migration 0012
  When migration 0013 is applied
  Then every representative row remains byte-equivalent on selected columns
  And all six enterprise tables are empty
  And schema version becomes `13`

Scenario: agent profile and explicit legacy alias round trip
  Test:
    Package: agentd-store
    Filter: agent_profile_round_trip_alias_and_lifecycle
  Level: store integration
  Test Double: temporary SQLite database
  Given an existing base agent and a new active agent profile
  When the legacy id is explicitly mapped and profile becomes disabled then active
  Then profile fields, status, and version round trip
  And alias lookup returns the canonical profile id
  But no legacy runtime-handle value is copied into the profile

Scenario: agent profile rejects invalid ids and retired reactivation
  Test:
    Package: agentd-store
    Filter: agent_profile_rejects_invalid_id_and_retired_reactivation
  Level: store integration
  Test Double: temporary SQLite database
  Given invalid-prefix or invalid-ULID profile ids and a retired profile
  When create or reactivation is attempted
  Then invalid ids return `Invariant` before insertion
  And retired reactivation returns `Conflict`
  And persisted rows remain unchanged

Scenario: worker registration supersedes old incarnation and rejects stale heartbeat
  Test:
    Package: agentd-store
    Filter: worker_registration_supersedes_incarnation_and_rejects_stale_heartbeat
  Level: store transaction integration
  Test Double: temporary SQLite database
  Given an enrolled worker with a current incarnation
  When a second incarnation registers
  Then the first is atomically non-current with `superseded_at`
  And the second is the only current incarnation
  And heartbeat for the first returns `Stale` without changing its timestamp
  And heartbeat for the second returns `Accepted`

Scenario: retired worker rejects new incarnation
  Test:
    Package: agentd-store
    Filter: retired_worker_rejects_new_incarnation
  Level: store transaction integration
  Test Double: temporary SQLite database
  Given a worker transitioned to terminal `retired`
  When another incarnation registration is attempted
  Then registration returns `Conflict`
  And no current incarnation is created
  And the retired worker version and state remain unchanged

Scenario: runtime resume keeps session and immutable attempt placement
  Test:
    Package: agentd-store
    Filter: runtime_session_attempt_resume_keeps_session_and_immutable_placement
  Level: store transaction integration
  Test Double: temporary SQLite database
  Given a task, profile, pinned snapshot tuple, and current worker incarnation
  When a session starts an attempt, marks it gone, and starts a resume attempt
  Then both attempts share one unchanged `RuntimeSessionId`
  And the first attempt is terminal `gone` and non-current
  And the second is the only current attempt on the selected incarnation
  And snapshot fields round trip unchanged

Scenario: runtime attempt rejects terminal session or stale worker
  Test:
    Package: agentd-store
    Filter: runtime_session_rejects_terminal_or_stale_worker_attempt
  Level: store transaction integration
  Test Double: temporary SQLite database
  Given a terminal runtime session and a superseded worker incarnation
  When a new attempt is requested for either invalid parent
  Then each request returns `Conflict`
  And no runtime attempt row is inserted

Scenario: roadmap and parity record schema without claiming worker protocol
  Test:
    Package: agentctl
    Filter: p267_roadmap_and_parity_record_schema_without_claiming_protocol
  Level: artifact inspection
  Test Double: repository Markdown files
  Given P267 store tests pass
  When roadmap and parity rows are inspected
  Then both reference migration 0013 and P267
  And durable runtime identity remains partial pending API, protocol, and runtime use
  And worker fleet protocol remains partial
  And Immediate Next Step advances to P268 artifact/audit model
