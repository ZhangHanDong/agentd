spec: task
name: "enterprise project room repository authority model"
tags: [e2e, architecture, project, repository, matrix, authority, snapshot, enterprise, p266]
---

## Intent

Define the foreign project-authority references and immutable execution
snapshot that connect Specify or `LocalProjectAuthority` to agentd without
making agentd a second project database. This slice replaces ambiguous bare
project, repository path, and Matrix room strings with versioned authority
references before enterprise worker or execution schemas consume them.

## Decisions

- Add `docs/specs/2026-07-10-enterprise-project-room-repo-reference-contract.md`
  as the authoritative P266 refinement of `ProjectAuthorityPort` under P264 and
  `ProjectAuthorityRef` under P265.
- Every authority-owned reference is the tuple `AuthorityKey`, `ResourceKind`,
  immutable `ResourceId`, and immutable `ResourceVersion`. Equality includes
  all four fields; agentd never parses meaning from the id string.
- Define an explicit catalog for organization, team, project, repository,
  project-room binding, issue, requirement, frozen spec version, product
  workflow, RBAC policy version, quota policy version, certification policy
  version, Matrix room transport, and project execution snapshot references.
- A `ProjectExecutionSnapshotRef` resolves to one immutable snapshot containing
  exact project/resource refs, binding versions, policy versions, issue/
  requirement/frozen-spec refs, issue time, expiry, content hash, and offline
  recovery policy. Each `ExecutionRunId` pins one snapshot and never follows
  `latest` during the run.
- A project snapshot contains one or more repository bindings. Each execution
  run selects exactly one target `RepositoryRef` plus an immutable base commit
  SHA; remote URL, forge slug, default branch, local checkout, and worktree path
  are locators or metadata, never repository identity.
- A project may have zero or more room bindings, but a Matrix room has at most
  one active `command` binding per `AuthorityKey`. Command routing carries the
  binding ref and snapshot ref; Matrix membership is transport input while the
  pinned RBAC policy version remains authorization authority.
- New enterprise executions require live authority validation. Existing-run
  recovery may use its pinned snapshot only when policy is
  `allow_pinned_until_expiry`, the snapshot is unexpired, and project/spec/
  policy/repository inputs remain unchanged; default recovery policy is `deny`.
- Configured Specify failures are fail-closed and never select
  `LocalProjectAuthority`. Authority changes require an explicit immutable
  `AuthorityRebindRecord`; historical runs retain old references and are never
  rewritten in place.
- Current `projects`, `issues`, `runs.project_id`, `matrix_events` project/room
  columns, and `agent_scheduler_queue.room` are compatibility projections,
  caches, aliases, locators, or transport hints. They remain unchanged in P266
  and cannot be treated as future authority records.
- P266 changes documentation contracts only. It does not add project tables,
  adapters, protocol payloads, migrations, or API routes; P267 and later slices
  consume the reference model.

## Boundaries

### Allowed Changes

- specs/e2e/p266-enterprise-project-room-repo-model.spec.md
- docs/specs/2026-07-10-enterprise-project-room-repo-reference-contract.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- docs/parity/agent-chat-capability-map.md
- docs/superpowers/plans/2026-07-10-p266-project-authority-reference.md
- crates/agentctl/tests/enterprise_project_authority_contract.rs

### Forbidden

- Do not add or modify database migrations, production Rust types, HTTP routes,
  CLI commands, MCP tools, Matrix handlers, service configuration, or Specify
  adapters.
- Do not make `projects`, `issues`, repository/checkout/worktree paths, Matrix
  room ids, branch names, forge slugs, or scheduler room strings authoritative
  project/repository identity.
- Do not create agentd-owned organization, team, project, repository, room
  binding, issue/spec, RBAC, quota, or certification-policy tables.
- Do not permit implicit authority switching, local fallback after configured
  Specify failure, mutable `latest` lookup during a run, or in-place rewriting
  of historical authority references.
- Do not change P200-P265 runtime behavior or parity gate exit semantics.
- Do not start Claude, Codex, tmux, Matrix, OpenFab, Specify, or remote services.

## Out of Scope

- Concrete Specify/LocalProjectAuthority APIs, JSON/protobuf payloads, auth
  tokens, signatures, cache storage, and cryptographic snapshot validation.
- Project UI, project creation, issue/spec review/freeze, room creation/invite,
  forge repository provisioning, and RBAC policy authoring.
- Multi-repository atomic commits; one execution run has one target repository.
- Production offline operation beyond the documented validation decision table.
- Compatibility migration/backfill and deletion of legacy project columns.

## Completion Criteria

<!-- lint-ack: decision-coverage — the eight artifact scenarios verify the resource catalog, snapshot, repository/room bindings, authority/recovery failures, rebind history, legacy classifications, and roadmap integration. -->
<!-- lint-ack: observable-decision-coverage — every P266 result is a repository Markdown artifact inspected by a bound integration test. -->
<!-- lint-ack: output-mode-coverage — P266 produces repository Markdown only and adds no CLI, network, cache, or runtime output mode. -->
<!-- lint-ack: boundary-entry-point — enterprise_project_authority_contract reads every allowed documentation entry point; its own file is the bound test surface. -->
<!-- lint-ack: bdd-rule-grouping — all scenarios prove one foreign project-authority reference contract. -->

Scenario: resource catalog separates authority and transport references
  Test:
    Package: agentctl
    Filter: p266_resource_catalog_assigns_owner_kind_and_versioning
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the P266 resource reference catalog
  When type, owner, resource kind, and versioning columns are parsed
  Then all required reference types occur exactly once
  And project resources are owned by `SpecifyProjectAuthority`
  And Matrix room transport is owned by `MatrixRobrixTransport`
  But no reference is owned by `AgentdControlPlane`

Scenario: execution snapshot pins every authority input
  Test:
    Package: agentctl
    Filter: p266_execution_snapshot_is_immutable_complete_and_run_pinned
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the `ProjectExecutionSnapshot` field contract
  When required fields and run pinning rules are inspected
  Then project, repository/room bindings, requirement/spec, workflow, policies, revision, expiry, hash, and recovery policy are present
  And an execution run stores exactly one immutable snapshot ref
  But it never resolves `latest` or mutates the snapshot during execution

Scenario: repository binding selects identity and immutable source separately
  Test:
    Package: agentctl
    Filter: p266_repository_binding_requires_one_target_and_base_commit
  Level: artifact inspection
  Test Double: repository Markdown file
  Given a project with one or more repository bindings
  When an execution run is authorized
  Then it selects exactly one target `RepositoryRef` and one base commit SHA
  And additional repository refs are read-only execution inputs
  But URL, slug, branch, checkout, worktree, and local path are never repository identity

Scenario: room binding makes command routing deterministic
  Test:
    Package: agentctl
    Filter: p266_room_binding_has_single_command_owner_and_pinned_rbac
  Level: artifact inspection
  Test Double: repository Markdown file
  Given versioned project-room bindings
  When a Matrix command is normalized for agentd
  Then it carries `ProjectRoomBindingRef` and `ProjectExecutionSnapshotRef`
  And a room has at most one active command binding per authority
  And authorization uses the pinned RBAC policy version rather than room membership alone

Scenario: authority validation fails closed with bounded recovery exception
  Test:
    Package: agentctl
    Filter: p266_authority_validation_and_offline_recovery_fail_closed
  Level: artifact inspection
  Test Double: repository Markdown file
  Given configured Specify, local authority, and pinned-snapshot states
  When new execution or existing-run recovery is requested during authority failure
  Then the decision table has an exact allow/deny result for every state
  And new execution is denied without live authority validation
  And default offline recovery is denied
  But explicitly allowed unexpired unchanged pinned recovery may continue

Scenario: explicit authority rebind preserves historical references
  Test:
    Package: agentctl
    Filter: p266_authority_rebind_is_explicit_versioned_and_non_rewriting
  Level: artifact inspection
  Test Double: repository Markdown file
  Given a local-to-Specify or Specify-to-Specify authority change
  When resources are imported or rebound
  Then an immutable `AuthorityRebindRecord` maps old refs to new refs
  And equality includes `AuthorityKey` even when resource id strings match
  But old runs and artifacts retain their original snapshot and authority refs

Scenario: base project fields are compatibility data only
  Test:
    Package: agentctl
    Filter: p266_legacy_project_fields_are_classified_non_authoritative
  Level: artifact inspection
  Test Double: repository Markdown file
  Given base projects, issues, runs, Matrix events, and scheduler queue fields
  When their P266 classification table is parsed
  Then every required field is a projection, cache, alias, locator, or transport hint
  But no field is a project authority record or canonical repository/room binding

Scenario: roadmap and parity advance without claiming implementation
  Test:
    Package: agentctl
    Filter: p266_roadmap_and_parity_advance_without_project_implementation
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the replacement roadmap and parity map
  When P266 integration is inspected
  Then both reference the project authority contract
  And `project_room_repo_binding` remains missing with implementation pending
  And the roadmap Immediate Next Step advances to P267 agent/worker schema
