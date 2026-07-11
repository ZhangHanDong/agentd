spec: task
name: "agentd Specify OpenFab ownership boundary"
tags: [e2e, architecture, ownership, specify, openfab, enterprise, p264]
---

## Intent

Resolve source-of-truth ambiguity before enterprise identities and schemas are
implemented. This slice distinguishes project authority, execution control,
worker-local runtime state, certification authority, and transport/client roles
while preserving one execution model for standalone and enterprise deployment.

## Decisions

- Add `docs/specs/2026-07-10-enterprise-execution-ownership-boundary.md` as the
  P264 amendment. It is authoritative where the replacement roadmap or the
  2026-05-29 Path B document is ambiguous, while preserving Specify ownership
  of project context, spec lifecycle, and Matrix command authority.
- Use five explicit roles: `SpecifyProjectAuthority`, `AgentdControlPlane`,
  `AgentdWorker`, `OpenFabCertificationAuthority`, and
  `MatrixRobrixTransport`. Every state class has exactly one authoritative
  owner; other roles hold references, projections, or bounded caches.
- `SpecifyProjectAuthority` owns organization/team/project/repository context,
  Matrix project-room binding, issue/requirement/spec lifecycle, product
  workflow state, project membership/RBAC policy, and versioned certification
  policy declarations. agentd consumes immutable ids and policy snapshots.
- `AgentdControlPlane` owns worker and agent capability registries, execution
  queue and fenced leases, runtime-session records, execution run/checkpoint
  state, runtime artifact indexes, execution audit events, and quota usage.
  This is an execution control plane, not a second project/spec product.
- `AgentdWorker` owns only live local process/PTY state, worktrees, caches, and
  transcript upload spool while work is executing. Durable records are
  acknowledged to `AgentdControlPlane`; worker disappearance cannot become the
  authoritative project or task state.
- `OpenFabCertificationAuthority` owns certification requests/results,
  signatures, and provenance attestations. Delivery and certification are
  separate; default `gate=none` records evidence without making OpenFab the
  execution owner or a mandatory delivery gate.
- `MatrixRobrixTransport` provides human interaction, Matrix identity, rooms,
  commands, and notifications. Matrix/Robrix does not own project bindings,
  execution state, runtime transcripts, artifacts, leases, or certification.
- Define three future protocol seams: `ProjectAuthorityPort` between Specify or
  the local adapter and agentd, `WorkerFleetPort` between the execution control
  plane and workers, and `CertificationPort` between agentd and OpenFab. P264
  documents responsibilities only; later specs define wire types and APIs.
- Standalone mode uses `LocalProjectAuthority`, an embedded implementation of
  `ProjectAuthorityPort`, with the same stable ids and execution contracts.
  `SpecifyProjectAuthority` has precedence whenever configured; enterprise mode
  must not silently fall back to local authority after remote errors.
- Amend the P263 reconciliation regression so it requires one unique P263
  anchor without assuming P263 remains the maximum e2e spec id. That historical
  maximum assertion would reject every correctly reserved P264-P279 spec.

## Boundaries

### Allowed Changes

- specs/e2e/p264-agentd-specify-openfab-ownership-boundary.spec.md
- docs/specs/2026-07-10-enterprise-execution-ownership-boundary.md
- docs/specs/2026-05-29-agentd-specify-boundary.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- docs/parity/agent-chat-capability-map.md
- docs/superpowers/plans/2026-07-10-p264-ownership-boundary.md
- crates/agentctl/tests/enterprise_ownership_contract.rs
- crates/agentctl/tests/worktree_reconciliation_contract.rs

### Forbidden

- Do not add migrations, Rust production types, HTTP routes, CLI commands, MCP
  tools, Matrix handlers, or service configuration.
- Do not move issue/spec review/freeze or Matrix command authority into agentd.
- Do not make OpenFab certification mandatory by default.
- Do not make Matrix rooms, terminal output, worktrees, or worker memory a
  durable source of truth.
- Do not alter P200-P263 runtime behavior or parity gate exit semantics.
- Do not start Claude, Codex, tmux, Matrix, OpenFab, Specify, or remote services.

## Out of Scope

- Concrete database schemas, identifier representations, event envelopes,
  authentication tokens, signatures, or API payloads.
- Selecting a central SQL/event/object storage technology or deployment vendor.
- Implementing `ProjectAuthorityPort`, `WorkerFleetPort`, or
  `CertificationPort`.
- Importing project context from Specify or certification data from OpenFab.
- Resolving merge/branch protection policy beyond recording which authority
  supplies policy and certification results.

## Completion Criteria

<!-- lint-ack: decision-coverage — the eight artifact scenarios jointly verify the ownership table, negative boundaries, protocol seams, deployment precedence, roadmap/parity integration, and forward-compatible P263 id reconciliation. -->
<!-- lint-ack: observable-decision-coverage — every observable P264 result is a repository Markdown artifact inspected by a bound integration test. -->
<!-- lint-ack: output-mode-coverage — P264 produces repository Markdown only, and every modified documentation surface is covered by a bound inspection test. -->
<!-- lint-ack: boundary-entry-point — the enterprise_ownership_contract integration test reads every allowed documentation entry point; its own file is the bound test surface. -->
<!-- lint-ack: bdd-rule-grouping — all scenarios prove the single P264 source-of-truth boundary rule. -->

Scenario: every enterprise state class has exactly one authoritative owner
  Test:
    Package: agentctl
    Filter: p264_ownership_contract_assigns_each_state_class_once
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the P264 ownership table
  When durable and worker-local state classes are collected
  Then every required state id occurs exactly once
  And every row names one of the five decided roles as owner
  And no row uses `shared`, `agentd/Specify`, or an empty owner

Scenario: Specify remains project authority without becoming execution state owner
  Test:
    Package: agentctl
    Filter: p264_specify_owns_project_authority_not_execution_state
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the enterprise ownership amendment
  When `SpecifyProjectAuthority` responsibilities are inspected
  Then project, repository, room binding, spec lifecycle, product workflow, RBAC, and policy intent are owned by Specify
  But worker registry, runtime session, execution lease, checkpoint, and transcript state are not owned by Specify

Scenario: agentd control plane and worker local state are disjoint
  Test:
    Package: agentctl
    Filter: p264_agentd_control_plane_and_worker_have_disjoint_state
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the `AgentdControlPlane` and `AgentdWorker` rows
  When their owned state ids are compared
  Then the control plane owns durable execution records and lease recovery
  And the worker owns live process/PTY, worktree/cache, and upload spool only
  But neither role owns canonical project/spec lifecycle state

Scenario: OpenFab certification is optional and separate from delivery
  Test:
    Package: agentctl
    Filter: p264_openfab_certifies_without_owning_delivery_or_execution
  Level: artifact inspection
  Test Double: repository Markdown files
  Given OpenFab integration defaults to `gate=none`
  When certification ownership is inspected
  Then OpenFab owns certification results, signatures, and attestations
  And `deliver` and `certify` are separate decisions
  But OpenFab does not own execution queues, runtime sessions, task leases, commits, or pull requests

Scenario: standalone mode uses the same project port with explicit precedence
  Test:
    Package: agentctl
    Filter: p264_standalone_mode_uses_same_ports_and_explicit_precedence
  Level: artifact inspection
  Test Double: repository Markdown file
  Given local and enterprise deployment modes
  When project authority selection is inspected
  Then both modes use `ProjectAuthorityPort` and the same stable identity model
  And standalone mode uses `LocalProjectAuthority` only when Specify is not configured
  But configured enterprise mode does not fall back to local authority after remote failure

Scenario: original Path B boundary and roadmap reference the P264 amendment
  Test:
    Package: agentctl
    Filter: p264_existing_boundary_and_roadmap_reference_amendment
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the 2026-05-29 Path B boundary and replacement roadmap
  When their authority statements are inspected
  Then both reference the P264 amendment
  And the roadmap distinguishes project control from execution control
  And P264 precedes the P267 enterprise schema slice

Scenario: parity rows name the resolved authority for cross-system gaps
  Test:
    Package: agentctl
    Filter: p264_parity_rows_name_resolved_owners
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the P263 enterprise replacement rows
  When project binding, worker fleet, lease, RBAC/quota, artifact/provenance, and doctor rows are inspected
  Then each row names its authoritative role or explicit split
  And none describes agentd as the project/spec source of truth

Scenario: P263 reconciliation accepts the next reserved spec id
  Test:
    Package: agentctl
    Filter: p263_reconciliation_reserves_non_conflicting_sequences
  Level: artifact inspection
  Test Double: repository spec directory
  Given P263 reserved P264-P279 for collision-free continuation
  When the e2e spec ids are scanned after P264 is added
  Then every P200+ id remains unique
  And the P263 reconciliation anchor occurs exactly once
  And P264 is accepted without weakening migration reservations
