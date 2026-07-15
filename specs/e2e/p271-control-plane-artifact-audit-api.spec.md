spec: task
name: "control plane artifact audit usage and certification reference API"
tags: [e2e, control-plane, artifact, audit, usage, certification, fencing, p271]
---

## Intent

Expose the P268 immutable execution-evidence store through typed control-plane
ports for artifact metadata publication/lookup, bounded audit replay, measured
usage, and OpenFab-owned certification references. Worker-originated artifact
and usage reports must carry a current P270 lease claim; rejected stale,
terminal, expired, or superseded reports must be durably audited before the
control plane returns the rejection.

## Decisions

- Add closed execution artifact, audit actor, usage metric, and certification
  reference kinds plus immutable evidence links, records, cursors, pages, and
  errors to one `agentd-core` execution-evidence API module.
- Add four async core ports: `ArtifactIndexPort`, `ExecutionAuditPort`,
  `UsageLedgerPort`, and `CertificationReferencePort`. Each port exposes only
  state owned or referenced by `AgentdControlPlane` under P264.
- `SqliteExecutionEvidenceControlPlane<L>` implements all four ports over the
  existing P268 repositories and an injected `TaskLeasePort`. It defines no
  HTTP, worker wire, object-store, Specify, or OpenFab transport.
- Artifact publication records immutable metadata after object bytes already
  have a durable opaque `storage_ref`. Exact retries under the same
  `ExecutionArtifactId` return the original record; changed retries conflict.
  Lookup supports exact id and bounded stable `(created_at, id)` run pages.
- Audit append preserves P268 event-id plus idempotency semantics. Replay is
  bounded to `1..=200` rows and advances only by database sequence, never by
  caller occurrence time.
- Measured usage is a typed `usage.measured` audit event rather than a new
  usage identity/table. `AuditEventId`, idempotency scope/key, parent links,
  database sequence, and append-only triggers remain authoritative. Usage
  pages and totals parse only that closed payload schema.
- Closed usage metrics are input, cached-input, output, and reasoning tokens;
  tool calls; runtime milliseconds; and artifact bytes. Quantities are
  unsigned; provider and model are optional dimensions, not identity.
- Certification reference recording wraps P268
  `artifact_certification_refs`. It stores only immutable external
  request/result/signature/attestation refs and provides ordered lookup. It
  neither submits to OpenFab nor interprets a certification result.
- Worker artifact and usage APIs require an exact current `TaskLeaseClaim` and
  matching task/worker evidence links. Before returning a typed lease
  rejection, the adapter appends an `execution.report_rejected` audit event
  carrying operation, lease, fencing token, and stable rejection reason.
- A failure to persist the required rejection audit fails closed as evidence
  API unavailability. Successful report acceptance and general audit append
  remain explicit operations; P271 does not invent cross-repository atomicity.
- P271 adds Rust APIs, repository queries, SQLite orchestration, and tests only.
  Object bytes, OpenFab calls, worker network protocol, auth/policy
  enforcement, dual-write, cutover, and delivery gating remain later work.

## Boundaries

### Allowed Changes

- specs/e2e/p271-control-plane-artifact-audit-api.spec.md
- docs/specs/2026-07-10-control-plane-execution-evidence-api.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- docs/parity/agent-chat-capability-map.md
- docs/superpowers/plans/2026-07-10-p271-control-plane-artifact-audit-api.md
- crates/agentd-core/src/ports/execution_evidence.rs
- crates/agentd-core/src/ports/mod.rs
- crates/agentd-core/tests/execution_evidence.rs
- crates/agentd-store/Cargo.toml
- crates/agentd-store/src/lib.rs
- crates/agentd-store/src/execution_artifact_repo.rs
- crates/agentd-store/src/execution_audit_repo.rs
- crates/agentd-store/src/execution_evidence_control_plane.rs
- crates/agentd-store/tests/control_plane_execution_evidence.rs
- crates/agentctl/tests/control_plane_execution_evidence_api_contract.rs
- crates/agentctl/tests/enterprise_artifact_audit_contract.rs
- crates/agentctl/tests/control_plane_dispatch_api_contract.rs
- crates/agentctl/tests/enterprise_ownership_contract.rs
- crates/agentctl/tests/worktree_reconciliation_contract.rs

### Forbidden

- Preserve migration `0015` as the latest schema for P271; do not add, alter,
  drop, rename, backfill, or reinterpret current tables, columns, triggers, or
  existing rows.
- Canonical artifact/audit ids continue to originate from their control-plane
  allocators. Hashes, paths, storage refs, OpenFab refs, Matrix values, worker
  locators, and legacy event sequences remain metadata or external refs only.
- Preserve append-only artifact, audit, usage-event, and certification history;
  changed idempotent retries, cursor rollback, and unbounded page requests are
  rejected.
- OpenFab remains authoritative for certification requests, results,
  signatures, and attestations. Agentd records references without deriving a
  certification verdict or changing delivery state.
- Worker evidence is accepted only after exact P270 claim validation and link
  matching. A required rejection audit cannot be skipped on the successful
  rejection-return path.
- Do not add daemon, agentctl command, HTTP, MCP, Matrix, Specify, OpenFab,
  object-store, worker network, native runtime, or delivery behavior.
- Do not change P200-P270 command output/runtime semantics, and do not start
  Claude, Codex, tmux, Matrix, Specify, OpenFab, or remote services in tests.

## Out of Scope

- Artifact byte upload/download, multipart protocols, presigned URLs,
  retention, garbage collection, encryption, scanning, and redaction.
- OpenFab submission/polling/webhooks, signature verification, trust roots,
  certification policy evaluation, source delivery, and delivery gating.
- Authenticated worker transport, acknowledgement/retry wire schemas, offline
  spool recovery, bandwidth control, and remote fleet operation.
- RBAC/quota authorization, budget reservation, price/currency calculation,
  enforcement decisions, and operator quota diagnostics.
- Legacy artifact/event dual-write, shadow comparison, service cutover,
  rollback automation, public API schemas, and dashboards.

## Completion Criteria

<!-- lint-ack: decision-coverage - eight scenarios cover core ports, artifact/audit/usage/certification adapters, fenced worker acceptance/rejection auditing, and roadmap integration. -->
<!-- lint-ack: observable-decision-coverage - outputs are typed records/errors, bounded pages/totals, durable rows, and repository files bound to explicit tests. -->
<!-- lint-ack: output-mode-coverage - P271 adds no CLI/network output mode; Rust API and persistence outcomes are directly tested. -->
<!-- lint-ack: boundary-entry-point - scenarios bind all four core ports, the SQLite adapter, fenced worker entry points, and documentation artifacts. -->
<!-- lint-ack: bdd-rule-grouping - all scenarios prove one execution-evidence control-plane slice. -->

Scenario: execution evidence ports expose the typed P271 contract
  Test:
    Package: agentd-core
    Filter: execution_evidence_contract
  Level: core API unit
  Test Double: recording implementations of all four ports
  Given closed artifact audit usage and certification values with immutable links
  When each port method is called with typed requests cursors and limits
  Then every request reaches the matching port unchanged
  And all closed enum strings round trip
  And page limits outside `1..=200` are rejected by request validation
  And no object bytes OpenFab verdict or delivery command exists in the API

Scenario: artifact index publication is idempotent and lists stable run pages
  Test:
    Package: agentd-store
    Filter: artifact_index_publish_is_idempotent_and_lists_by_run
  Level: control-plane SQLite integration
  Test Double: temporary SQLite database
  Given valid run task runtime and worker parents plus two immutable artifact envelopes
  When publish exact retry changed retry get and bounded run listing are called
  Then exact retry returns the original record without a duplicate
  And changed retry returns conflict without mutation
  And exact lookup returns the immutable metadata
  And run pages advance by `(created_at, id)` without gaps or duplicates

Scenario: audit append and bounded replay preserve database sequence
  Test:
    Package: agentd-store
    Filter: audit_log_append_and_bounded_replay_preserve_sequence
  Level: control-plane SQLite integration
  Test Double: temporary SQLite database
  Given audit envelopes whose occurrence timestamps are reverse ordered
  When append exact retry changed retry and one-row replay pages are called
  Then exact retry returns the original event and sequence
  And changed retry conflicts without another sequence
  And pages follow database sequence with a stable next cursor
  And invalid limits or cursors fail before query

Scenario: usage ledger records typed audit measurements and totals
  Test:
    Package: agentd-store
    Filter: usage_ledger_records_typed_audit_measurements_and_totals
  Level: control-plane SQLite integration
  Test Double: temporary SQLite database
  Given typed token runtime tool-call and artifact-byte measurements
  When they are recorded retried paged and totaled for one run
  Then records persist as `usage.measured` audit events with no new usage table
  And exact retries do not double count while changed retries conflict
  And bounded pages preserve audit sequence
  And totals contain one deterministic unsigned sum per metric

Scenario: certification reference port records external refs without a delivery gate
  Test:
    Package: agentd-store
    Filter: certification_reference_port_records_external_refs_without_delivery_gate
  Level: control-plane SQLite integration
  Test Double: temporary SQLite database and no OpenFab transport
  Given one immutable artifact and OpenFab-owned request result signature and attestation refs
  When refs are appended retried and listed
  Then exact retries return original records and listing follows database id order
  And changed or cross-artifact ref reuse conflicts
  And artifact metadata remains unchanged
  And no OpenFab call verdict interpretation or delivery mutation occurs

Scenario: worker artifact publication requires a current lease and audits rejection
  Test:
    Package: agentd-store
    Filter: worker_artifact_publish_requires_current_lease_and_audits_rejection
  Level: fenced control-plane SQLite integration
  Test Double: temporary SQLite database with real P270 lease adapter
  Given an active lease and a matching worker artifact report
  When the current report and then an old-token retry are submitted
  Then the current report publishes the artifact
  And the old-token report returns `stale_fencing_token` without another artifact
  And one durable `execution.report_rejected` audit event identifies the operation lease token and reason

Scenario: worker usage rejects superseded or terminal leases and audits each report
  Test:
    Package: agentd-store
    Filter: worker_usage_report_rejects_superseded_or_terminal_lease_and_audits
  Level: fenced control-plane SQLite integration
  Test Double: temporary SQLite database with real P270 lease and worker repositories
  Given a usage report from a superseded incarnation and another from a terminal lease
  When both reports are submitted
  Then they return `stale_worker_incarnation` and `terminal_lease` respectively
  And no usage measurement is counted
  And each rejection appends one durable audit event before the error returns

Scenario: roadmap and parity record P271 without claiming upload or OpenFab network
  Test:
    Package: agentctl
    Filter: p271_roadmap_and_parity_record_evidence_apis_without_claiming_upload_or_openfab_network
  Level: artifact inspection
  Test Double: repository Markdown and Rust files
  Given P271 core and SQLite adapter tests pass
  When roadmap and parity evidence are inspected
  Then they reference P271 all four ports typed usage audit events and fenced rejection audit
  And artifact audit provenance remains partial pending object storage OpenFab network and cutover
  And auth quota remains partial pending P279 enforcement
  And Immediate Next Step advances to P272 runtime status capture shutdown and rebind compatibility port
