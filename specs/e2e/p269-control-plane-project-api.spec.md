spec: task
name: "control plane project authority API and adapters"
tags: [e2e, control-plane, authority, project, specify, standalone, enterprise, p269]
---

## Intent

Implement the P266 project-authority reference contract as a pure core port,
explicit standalone and Specify-backed adapters, and a control-plane decision
service. New execution and recovery must pin and revalidate immutable project
snapshots without treating legacy project rows, paths, room ids, or caches as
authority and without silently falling back from configured Specify to local.

## Decisions

- Add P266 authority tuple types, closed resource kinds, typed references,
  repository/room bindings, the complete `ProjectExecutionSnapshot`, and
  deterministic snapshot validation to `agentd-core`.
- Add `ProjectAuthorityPort` with asynchronous `resolve`, `refresh`, and
  `health` calls plus typed unavailable/not-found/invalid/unverifiable errors.
- Add workspace crate `agentd-project-authority`; keep adapters and
  control-plane decisions outside `agentd-core`.
- `LocalProjectAuthority` is constructed from explicit immutable snapshots,
  rejects ambiguous current project mappings, and is available only when the
  composition root selects it.
- `SpecifyProjectAuthority<T>` wraps an injected `SpecifyAuthorityTransport`.
  It defines no HTTP wire contract and validates successful envelopes against
  the configured authority and request before returning them.
- A configured Specify failure is fail-closed. The Specify adapter has no
  local adapter, local snapshot map, cache fallback, or legacy project lookup.
- New execution resolves one current valid snapshot and returns an immutable
  pinned snapshot with exactly one target repository and base commit.
- Existing recovery refreshes the same snapshot. Unavailability permits only
  unchanged, unexpired `allow_pinned_until_expiry` recovery; every other
  unavailable, changed, expired, or unverifiable case is denied.
- P269 adds Rust APIs/tests and roadmap evidence only. It adds no HTTP route,
  Specify network client, project table, migration, dispatch, lease, artifact
  upload, OpenFab call, Matrix command normalization, or compatibility cutover.

## Boundaries

### Allowed Changes

- specs/e2e/p269-control-plane-project-api.spec.md
- docs/specs/2026-07-10-project-authority-port-api.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- docs/parity/agent-chat-capability-map.md
- docs/superpowers/plans/2026-07-10-p269-control-plane-project-api.md
- **/Cargo.toml
- crates/agentd-core/src/types/project_authority.rs
- crates/agentd-core/src/types/mod.rs
- crates/agentd-core/src/ports/project_authority.rs
- crates/agentd-core/src/ports/mod.rs
- crates/agentd-core/tests/project_authority.rs
- crates/agentd-project-authority/Cargo.toml
- crates/agentd-project-authority/src/lib.rs
- crates/agentd-project-authority/src/local.rs
- crates/agentd-project-authority/src/specify.rs
- crates/agentd-project-authority/src/control_plane.rs
- crates/agentd-project-authority/tests/support/mod.rs
- crates/agentd-project-authority/tests/local_project_authority.rs
- crates/agentd-project-authority/tests/specify_project_authority.rs
- crates/agentd-project-authority/tests/control_plane_authority.rs
- crates/agentctl/tests/enterprise_project_authority_contract.rs
- crates/agentctl/tests/project_authority_api_contract.rs
- crates/agentctl/tests/worktree_reconciliation_contract.rs

### Forbidden

- Do not create or modify project-authority SQLite tables, migrations, base
  project rows, or P267/P268 enterprise rows.
- Do not infer authority from `projects`, `runs.project_id`, paths, forge slugs,
  Matrix room ids, environment discovery, or cached legacy records.
- Do not define or call a real Specify HTTP endpoint, read credentials, make a
  network request, or start Specify in tests.
- Do not allow configured Specify errors or invalid envelopes to select local
  authority, stale cache, or a different authority key.
- Do not allow `latest`, multiple target repositories, mutable snapshot data,
  expired new execution, or changed offline recovery inputs.
- Do not add daemon/agentctl/MCP/Matrix routes, dispatch/lease behavior, object
  upload, OpenFab integration, or agent runtime behavior.
- Do not change P200-P268 command output, runtime behavior, or parity gates,
  and do not start Claude, Codex, tmux, Matrix, or remote services in tests.

## Out of Scope

- Specify HTTP/SDK wire schemas, authentication, retries, TLS, service
  discovery, streaming invalidations, and remote integration tests.
- Durable snapshot cache/pinning tables, legacy project mapping/backfill,
  dual-read, dual-write, cutover, and rollback.
- Dispatch, lease/fencing, worker protocol, Matrix room-command normalization,
  RBAC/quota enforcement, and usage accounting.
- Artifact upload/query APIs, certification requests, delivery, and product
  workflow mutation.

## Completion Criteria

<!-- lint-ack: decision-coverage — eight scenarios cover core tuples/snapshots, both adapters, fail-closed selection, pinning/recovery, and roadmap integration. -->
<!-- lint-ack: observable-decision-coverage — outputs are typed values/errors and repository files, each bound to explicit tests. -->
<!-- lint-ack: output-mode-coverage — P269 adds no CLI or network output mode; domain and adapter outcomes are directly tested. -->
<!-- lint-ack: boundary-entry-point — scenarios bind core types/port, local/Specify adapters, control-plane service, and docs. -->
<!-- lint-ack: bdd-rule-grouping — all scenarios prove one ProjectAuthorityPort control-plane slice. -->

Scenario: authority references and complete snapshots enforce P266 invariants
  Test:
    Package: agentd-core
    Filter: project_authority_refs_and_snapshot_validation_follow_p266
  Level: core domain unit
  Test Double: none
  Given typed authority references and complete project execution snapshots
  When valid and malformed values are constructed and validated
  Then tuple equality includes authority, kind, id, and version
  And every typed reference requires its closed resource kind
  And valid snapshots contain every P266 field with exactly one target repository
  And wrong authorities, hashes, base commits, or target counts are rejected

Scenario: local authority resolves refreshes and reports health explicitly
  Test:
    Package: agentd-project-authority
    Filter: local_project_authority_resolves_refreshes_and_reports_health
  Level: adapter unit
  Test Double: in-memory immutable snapshots
  Given an explicitly constructed local authority with one current project snapshot
  When resolve refresh and health are called
  Then resolve returns that project's validated snapshot
  And refresh returns the exact immutable snapshot
  And health reports available local mode and its authority key

Scenario: local authority rejects ambiguous or mismatched configuration
  Test:
    Package: agentd-project-authority
    Filter: local_project_authority_rejects_ambiguous_or_mismatched_configuration
  Level: adapter unit
  Test Double: in-memory immutable snapshots
  Given duplicate current snapshots or a snapshot from another authority
  When the local adapter is constructed
  Then construction returns a typed invalid configuration error
  And no authority adapter is produced

Scenario: Specify adapter forwards the port and validates returned envelopes
  Test:
    Package: agentd-project-authority
    Filter: specify_project_authority_forwards_contract_and_validates_envelopes
  Level: adapter unit
  Test Double: recording Specify transport
  Given a configured Specify adapter and recording transport
  When resolve refresh and health are called
  Then each request is forwarded once with unchanged typed values
  And matching validated responses are returned
  And a mismatched authority or snapshot response is `Unverifiable`

Scenario: configured Specify failure is fail closed without local fallback
  Test:
    Package: agentd-project-authority
    Filter: configured_specify_failure_is_fail_closed_without_local_fallback
  Level: adapter unit
  Test Double: unavailable Specify transport and adapter with no local fallback field
  Given Specify is the selected configured authority and its transport is unavailable
  When a new snapshot resolve is requested
  Then the result is `Unavailable`
  And no local adapter legacy row cache or alternate authority is consulted

Scenario: control plane pins one valid snapshot for new execution
  Test:
    Package: agentd-project-authority
    Filter: control_plane_new_execution_pins_validated_snapshot
  Level: control-plane unit
  Test Double: recording project authority port
  Given an available authority returning a current unexpired snapshot
  When new execution authorization is requested
  Then the port is resolved once and the exact snapshot is pinned
  And target repository and base commit derive from its sole target binding
  And expired mismatched or unverifiable snapshots are denied

Scenario: control plane recovery enforces live or bounded offline policy
  Test:
    Package: agentd-project-authority
    Filter: control_plane_recovery_enforces_live_or_bounded_offline_policy
  Level: control-plane unit
  Test Double: scripted project authority port
  Given a previously pinned snapshot and unchanged execution inputs
  When recovery is revalidated live or the authority is unavailable
  Then an exact live refresh returns `LiveRevalidated`
  And unexpired allow-pinned policy returns `OfflinePinned`
  And deny policy expiry changed inputs or changed live snapshots are denied

Scenario: roadmap and parity record API evidence without claiming integration
  Test:
    Package: agentctl
    Filter: p269_roadmap_and_parity_record_api_without_claiming_network_integration
  Level: artifact inspection
  Test Double: repository Markdown files
  Given P269 core and adapter tests pass
  When roadmap and project-authority parity text are inspected
  Then both reference P269 the port API and explicit local and Specify adapters
  And project-room-repository binding remains partial pending network and durable integration
  And Immediate Next Step advances to P270 dispatch lease and fencing API
