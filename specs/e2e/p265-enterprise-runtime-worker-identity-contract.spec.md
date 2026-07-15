spec: task
name: "enterprise runtime worker identity contract"
tags: [e2e, architecture, identity, worker, runtime, lease, enterprise, p265]
---

## Intent

Freeze the durable identity and lifecycle model that later enterprise schemas,
APIs, scheduler leases, and native runtime code must share. This slice separates
agent capability, worker enrollment, worker incarnation, logical runtime
session, runtime attempt, execution run/task, and lease identity so legacy
agent-chat and tmux addressing cannot become enterprise source-of-truth keys.

## Decisions

- Add `docs/specs/2026-07-10-enterprise-runtime-worker-identity-contract.md`
  as the authoritative P265 identity contract under the P264 ownership roles.
- Define eight immutable durable ids with distinct prefixes:
  `AgentProfileId=ap_`, `WorkerId=wk_`, `WorkerIncarnationId=wi_`,
  `RuntimeSessionId=rs_`, `RuntimeAttemptId=ra_`, `ExecutionRunId=r_`,
  `ExecutionTaskId=tr_`, and `LeaseId=ls_`. New ids use ULID payloads, are
  opaque outside validation/logging, and are never reused.
- Preserve `r_` and `tr_` compatibility with current core ids. Existing
  operator-authored `agents.id`, `mxid`, `server`, host names,
  `backend_target`, `session_name`, pane ids, PIDs, native resume refs, and
  `disp-N` tickets are aliases or metadata, never any of the eight ids.
- `AgentProfileId` names a reusable capability/configuration profile and has no
  online/busy process state. `WorkerId` names a stable enrollment;
  `WorkerIncarnationId` changes on every worker daemon registration or restart
  and fences reports from prior incarnations.
- `RuntimeSessionId` names one logical agent interaction across zero or more
  process attempts. Every spawn or resume creates a new `RuntimeAttemptId`;
  PID, PTY, backend address, and native resume ref remain attempt metadata.
- Define explicit state vocabularies and allowed transitions for agent profile,
  worker, runtime session, runtime attempt, execution run, execution task, and
  lease records. Terminal records do not reactivate; retry or resume creates the
  required new lease, incarnation, or attempt identity.
- Every active lease binds one `ExecutionTaskId` to one
  `WorkerIncarnationId`, has one `LeaseId`, and carries a task-scoped monotonic
  unsigned `FencingToken`. A later lease for the task has a strictly greater
  token; stale or terminal-lease mutations are rejected and audited.
- Worker loss invalidates its live process references and moves nonterminal
  runtime sessions to `resume_pending`. A valid native resume ref permits a new
  attempt under the same session; otherwise the session becomes terminal
  `lost` with reason `runtime_gone`.
- Specify or `LocalProjectAuthority` ids are immutable foreign authority
  references. agentd does not regenerate them or infer them from Matrix rooms,
  repository paths, worktrees, host names, or legacy agent ids.
- P265 changes documentation contracts only. P266-P268 decide project
  authority references and concrete enterprise schema/migration slices.

## Boundaries

### Allowed Changes

- specs/e2e/p265-enterprise-runtime-worker-identity-contract.spec.md
- docs/specs/2026-07-10-enterprise-runtime-worker-identity-contract.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- docs/parity/agent-chat-capability-map.md
- docs/superpowers/plans/2026-07-10-p265-runtime-worker-identity.md
- crates/agentctl/tests/enterprise_identity_contract.rs

### Forbidden

- Do not add or modify database migrations, production Rust types, HTTP routes,
  CLI commands, MCP tools, Matrix handlers, service configuration, or runtime
  backends.
- Do not reinterpret legacy `agents.id`, `backend_target`, `session_name`, PID,
  pane id, host name, Matrix id, or dispatch ticket as a new canonical id.
- Do not make a process, PTY, worker memory, Matrix room, worktree, or native
  agent resume ref the durable execution identity.
- Do not define project/spec/RBAC ownership inside agentd or alter P264 owners.
- Do not change P200-P264 runtime behavior or parity gate exit semantics.
- Do not start Claude, Codex, tmux, Matrix, OpenFab, Specify, or remote services.

## Out of Scope

- Concrete SQLite/PostgreSQL tables, foreign keys, indexes, migrations, and
  backfill algorithms.
- Rust newtypes, serde representations, protocol payloads, API routes, worker
  enrollment credentials, and secret rotation.
- Full scheduler retry/backoff/dead-letter policy beyond identity, terminal
  states, and fencing invariants.
- Native PTY process hosting, provider-specific resume commands, transcript
  storage, and artifact schemas.
- Specify project/reference payload design and OpenFab certification ids.

## Completion Criteria

<!-- lint-ack: decision-coverage — the eight artifact scenarios verify the id catalog, legacy exclusions, relationships, all lifecycle vocabularies, recovery/fencing failures, and roadmap integration. -->
<!-- lint-ack: observable-decision-coverage — every P265 output is a repository Markdown artifact inspected by a bound integration test. -->
<!-- lint-ack: output-mode-coverage — P265 produces repository Markdown only, with no CLI, network, cache, or persisted runtime behavior. -->
<!-- lint-ack: boundary-entry-point — enterprise_identity_contract reads every allowed documentation entry point; its own file is the bound test surface. -->
<!-- lint-ack: bdd-rule-grouping — all scenarios prove the single enterprise identity contract and its failure semantics. -->

Scenario: identity catalog uses distinct opaque durable ids
  Test:
    Package: agentctl
    Filter: p265_identity_catalog_uses_distinct_opaque_ids
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the P265 identity catalog
  When its canonical type, prefix, owner, and durability columns are parsed
  Then the eight required id types occur exactly once with distinct prefixes
  And run/task prefixes remain `r_` and `tr_`
  And every id is immutable, opaque, ULID-backed, and never reused

Scenario: legacy runtime fields cannot become canonical ids
  Test:
    Package: agentctl
    Filter: p265_legacy_runtime_fields_are_never_canonical_ids
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the base legacy identity fields
  When their P265 classifications are parsed
  Then every required field is classified as alias, locator, metadata, or import input
  But none is classified as a canonical durable id

Scenario: worker reincarnation fences stale reports
  Test:
    Package: agentctl
    Filter: p265_worker_incarnation_rejects_stale_reports
  Level: artifact inspection
  Test Double: repository Markdown file
  Given a stable `WorkerId` with a current `WorkerIncarnationId`
  When the worker daemon registers again or restarts
  Then a new incarnation id supersedes the prior incarnation
  And heartbeat, capacity, process, and lease reports from the prior incarnation are rejected and audited
  But the stable worker enrollment and history remain unchanged

Scenario: runtime session recovery separates logical session from process attempts
  Test:
    Package: agentctl
    Filter: p265_runtime_session_survives_attempt_loss_or_becomes_explicitly_lost
  Level: artifact inspection
  Test Double: repository Markdown file
  Given a nonterminal runtime session whose live process disappears
  When a valid native resume ref and retry policy are available
  Then the same runtime session enters `resume_pending` and receives a new runtime attempt
  But without a valid resume path it becomes terminal `lost` with reason `runtime_gone`

Scenario: lease fencing rejects stale or terminal mutations
  Test:
    Package: agentctl
    Filter: p265_lease_fencing_rejects_stale_mutations
  Level: artifact inspection
  Test Double: repository Markdown file
  Given an execution task with successive leases
  When a worker reports using an older fencing token or a terminal lease
  Then the mutation is rejected and audited
  And a retry uses a new lease id with a strictly greater task-scoped token

Scenario: execution relationships have one unambiguous direction
  Test:
    Package: agentctl
    Filter: p265_execution_relationships_are_unambiguous
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the P265 relationship table
  When profile, worker, session, attempt, run, task, lease, and project references are inspected
  Then each child names one authoritative parent or explicit foreign reference
  And live process metadata is scoped by runtime attempt and worker incarnation
  But no circular ownership or compound id is defined

Scenario: lifecycle tables define terminal and recovery states
  Test:
    Package: agentctl
    Filter: p265_lifecycle_tables_define_terminal_and_recovery_states
  Level: artifact inspection
  Test Double: repository Markdown file
  Given lifecycle rows for all seven durable record kinds
  When their state vocabularies and terminal sets are parsed
  Then every required state occurs exactly once in its record vocabulary
  And terminal states cannot transition back to active states
  And retry, resume, or re-registration allocates the required new identity

Scenario: roadmap and parity advance after the identity contract
  Test:
    Package: agentctl
    Filter: p265_roadmap_and_parity_advance_after_identity_contract
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the replacement roadmap and parity map
  When P265 integration is inspected
  Then both reference the authoritative identity contract
  And parity keeps durable runtime identity partial until implementation exists
  And the roadmap Immediate Next Step advances to P266 without adding project tables to agentd
