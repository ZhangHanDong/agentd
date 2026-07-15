spec: task
name: "enterprise execution artifact and audit store model"
tags: [e2e, store, sqlite, artifact, audit, provenance, enterprise, p268]
---

## Intent

Implement the P264-P266 enterprise execution-artifact and audit ownership model
as additive SQLite records and focused repositories. The slice must preserve
the legacy content-addressed `artifacts` and broadcast `events` tables while
making enterprise artifact metadata immutable, OpenFab references external,
and execution audit ordering/idempotency durable across retries and restarts.

## Decisions

- Add `ExecutionArtifactId` with prefix `ar_` and `AuditEventId` with prefix
  `ae_`; both use an immutable ULID payload and remain distinct from a content
  hash, legacy event sequence, OpenFab request id, path, or storage locator.
- Add migration `0014_enterprise_artifact_audit.sql` with four additive tables:
  `execution_artifacts`, `legacy_artifact_mappings`,
  `artifact_certification_refs`, and `execution_audit_events`; update
  `schema_meta` to version `14` without altering existing tables or rows.
- `execution_artifacts` stores a closed artifact kind, lowercase SHA-256,
  non-negative size, media type, opaque storage reference, valid provenance
  JSON, required execution run and P266 snapshot/repository/base-commit tuple,
  optional task/session/attempt/producer-incarnation links, and creation time.
  SQL triggers reject update/delete after insertion.
- Repository creation validates canonical ids and the complete parent chain:
  task belongs to run; session belongs to task and pins the same snapshot;
  attempt belongs to session; producer incarnation matches the attempt when
  both are supplied. Optional links may be absent for run-level inputs such as
  requirements, specs, and plans.
- `legacy_artifact_mappings` explicitly maps one existing `artifacts.sha256`
  row to one enterprise artifact. Mapping requires equal content hash and byte
  size, is idempotent for the same pair, conflicts for any remap, and never
  rewrites the legacy row.
- `artifact_certification_refs` appends immutable external
  `request|result|signature|attestation` references owned by
  `OpenFabCertificationAuthority`. The artifact row is never updated when a
  certification reference arrives; exact retries are idempotent and conflicting
  external-reference reuse fails.
- `execution_audit_events` uses a database-assigned monotonically increasing
  `sequence` for deterministic replay and a unique `(idempotency_scope,
  idempotency_key)` pair for retry collapse. It stores a canonical event id,
  actor/event/payload hash+JSON, required run and snapshot/repository tuple,
  optional task/session/attempt/artifact/worker links, occurrence and recording
  time, and rejects update/delete through SQL triggers.
- Audit append returns the original row for an exact idempotent retry without
  allocating a new sequence. Reuse of the same event id or idempotency pair for
  a different immutable envelope returns `Conflict`; cursor reads order only by
  `sequence`, not caller timestamps.
- P268 adds store APIs and tests only. It does not upload objects, read artifact
  bytes, call OpenFab, add certification workflow/gating, add HTTP/CLI/MCP
  surfaces, modify SSE events, or claim artifact/audit cutover readiness.

## Boundaries

### Allowed Changes

- specs/e2e/p268-enterprise-artifact-audit-model.spec.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- docs/parity/agent-chat-capability-map.md
- docs/superpowers/plans/2026-07-10-p268-artifact-audit-store-model.md
- crates/agentd-core/src/types/ids.rs
- crates/agentd-core/src/types/mod.rs
- crates/agentd-core/tests/enterprise_artifact_identity.rs
- crates/agentd-store/migrations/0014_enterprise_artifact_audit.sql
- crates/agentd-store/src/lib.rs
- crates/agentd-store/src/execution_artifact_repo.rs
- crates/agentd-store/src/execution_audit_repo.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-store/tests/migration_backcompat.rs
- crates/agentd-store/tests/enterprise_execution_artifacts.rs
- crates/agentd-store/tests/enterprise_execution_audit.rs
- crates/agentctl/tests/enterprise_artifact_audit_contract.rs

### Forbidden

- Do not alter, drop, rename, backfill, or rewrite current `artifacts`, `events`,
  `delivery_events`, run/task, project, agent, worker, or runtime rows.
- Do not use a hash, path, Matrix event, legacy event sequence, OpenFab id,
  worker-local locator, or provider id as a canonical artifact/audit identity.
- Do not create project/spec/policy authority tables or certification-authority
  state; OpenFab values remain immutable external references only.
- Do not permit enterprise artifact/certification/audit update or delete, audit
  duplicate sequences, or a changed envelope under an existing idempotency key.
- Do not add dependencies, object-store clients, HTTP routes, agentctl commands,
  MCP tools, Matrix handlers, daemon composition, upload behavior, or network
  calls.
- Do not change p200-p267 output/runtime/parity-gate semantics, and do not start
  Claude, Codex, tmux, Matrix, Specify, OpenFab, or remote services in tests.

## Out of Scope

- Object storage upload/download, multipart transfer, retention, garbage
  collection, encryption, malware scanning, transcript redaction, or presigned
  URLs.
- OpenFab request submission/polling, signature verification, trust roots,
  certification policy enforcement, or delivery gating.
- Artifact/audit HTTP schemas, pagination tokens, authorization, tenant
  partitioning, quota accounting, and operational dashboards.
- Automatic migration/backfill from legacy artifacts/events, dual-write,
  shadow comparison, API cutover, rollback, and final agent-chat replacement.

## Completion Criteria

<!-- lint-ack: decision-coverage — nine scenarios cover typed ids, schema/backcompat, artifact graph/immutability, legacy/certification mapping, audit replay/idempotency/conflicts, and roadmap integration. -->
<!-- lint-ack: observable-decision-coverage — P268 outputs are SQLite rows, repository outcomes/errors, typed ids, and roadmap/parity files, all bound to tests. -->
<!-- lint-ack: output-mode-coverage — P268 adds no CLI/network output mode; repository outcomes and persisted state are covered directly. -->
<!-- lint-ack: boundary-entry-point — scenarios bind core ids, migration/backcompat, both repository modules, and documentation integration. -->
<!-- lint-ack: bdd-rule-grouping — all scenarios prove the single additive enterprise artifact/audit store model. -->

Scenario: enterprise artifact and audit ids are distinct canonical identities
  Test:
    Package: agentd-core
    Filter: enterprise_artifact_and_audit_ids_are_distinct
  Level: core unit
  Test Double: none
  Given generated execution artifact and audit event ids
  When their strings and Rust types are inspected
  Then each has the P268 prefix plus a ULID payload
  And neither identity is interchangeable with the other or with existing ids

Scenario: migration creates immutable enterprise artifact and audit tables
  Test:
    Package: agentd-store
    Filter: migration_creates_enterprise_artifact_audit_tables
  Level: SQLite migration integration
  Test Double: temporary SQLite database
  Given a newly migrated store
  When schema version, tables, columns, constraints, indexes, and triggers are inspected
  Then all four P268 tables exist and schema version is `14`
  And canonical id/hash/JSON checks reject invalid direct inserts
  And artifact, certification, and audit update/delete triggers exist

Scenario: enterprise artifact audit migration preserves existing rows
  Test:
    Package: agentd-store
    Filter: enterprise_artifact_audit_migration_preserves_existing_rows
  Level: SQLite migration backcompat
  Test Double: raw in-memory SQLite with real migration files
  Given representative legacy artifact and broadcast-event rows after migration 0013
  When migration 0014 is applied
  Then both legacy rows remain byte-equivalent
  And all four P268 tables start empty
  And schema version becomes `14`

Scenario: execution artifact persists immutable provenance and valid parent graph
  Test:
    Package: agentd-store
    Filter: execution_artifact_persists_immutable_provenance_and_parent_graph
  Level: store transaction integration
  Test Double: temporary SQLite database
  Given a run, task, pinned runtime session, attempt, and producer incarnation
  When an execution artifact is created with matching snapshot and repository metadata
  Then every identity/hash/size/media/storage/provenance/link field round-trips
  And direct SQL update or delete is rejected
  And the artifact remains unchanged

Scenario: execution artifact rejects invalid metadata or mismatched parent graph
  Test:
    Package: agentd-store
    Filter: execution_artifact_rejects_invalid_metadata_or_parent_graph
  Level: store transaction integration
  Test Double: temporary SQLite database
  Given malformed ids/hashes/JSON and task/session/attempt/worker links from different parents
  When artifact creation is attempted
  Then malformed input returns `Invariant` before insertion
  And mismatched parent links return `Conflict`
  And no enterprise artifact row is inserted

Scenario: legacy mapping and certification references are explicit and append only
  Test:
    Package: agentd-store
    Filter: legacy_mapping_and_certification_refs_are_explicit_and_append_only
  Level: store transaction integration
  Test Double: temporary SQLite database
  Given one legacy content-addressed artifact and one matching enterprise artifact
  When the mapping and OpenFab request/result/signature/attestation refs are appended twice
  Then exact retries return the original records without duplicates
  And legacy bytes/path/kind remain unchanged
  And remapping or changing an external ref returns `Conflict`
  And direct certification update/delete is rejected

Scenario: audit append is ordered idempotent and append only
  Test:
    Package: agentd-store
    Filter: audit_append_is_ordered_idempotent_and_append_only
  Level: store transaction integration
  Test Double: temporary SQLite database
  Given two valid audit envelopes with caller timestamps in reverse order
  When they are appended and the first is retried exactly
  Then the retry returns the original event and sequence
  And `execution_audit_events.sequence` is database-assigned and strictly increasing
  And replay returns one row per event ordered by database sequence
  And direct update or delete is rejected

Scenario: audit append rejects changed retries and mismatched links
  Test:
    Package: agentd-store
    Filter: audit_append_rejects_changed_retries_and_mismatched_links
  Level: store transaction integration
  Test Double: temporary SQLite database
  Given a persisted idempotency key and task/session/attempt/artifact/worker links from different parents
  When a changed envelope or mismatched link is appended
  Then each operation returns `Conflict`
  And no additional sequence is allocated
  And the original event remains unchanged

Scenario: roadmap and parity record store evidence without claiming integration
  Test:
    Package: agentctl
    Filter: p268_roadmap_and_parity_record_store_without_claiming_integration
  Level: artifact inspection
  Test Double: repository Markdown files
  Given P268 store tests pass
  When roadmap and artifact-audit parity text are inspected
  Then both reference P268 and migration 0014
  And artifact-audit provenance remains partial pending APIs/upload/certification integration
  And Immediate Next Step advances to P269 control-plane project API
