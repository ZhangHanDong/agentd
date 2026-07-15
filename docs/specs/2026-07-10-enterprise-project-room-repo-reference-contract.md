# Enterprise Project, Room, and Repository Reference Contract

- **Date**: 2026-07-10
- **Status**: Decided by P266
- **Authority basis**: `2026-07-10-enterprise-execution-ownership-boundary.md`
- **Identity basis**: `2026-07-10-enterprise-runtime-worker-identity-contract.md`
- **Scope**: foreign authority references and immutable execution snapshots

This contract refines `ProjectAuthorityPort` without creating an agentd project
authority. Specify or `LocalProjectAuthority` owns project resources and
versions; agentd consumes one validated immutable snapshot for each execution
run.

## 1. Reference Tuple and Authority

An authority-owned reference contains exactly:

- `AuthorityKey`: stable identity of the configured authority deployment and trust domain;
- `ResourceKind`: closed kind vocabulary from the catalog below;
- `ResourceId`: immutable, opaque id allocated by that authority;
- `ResourceVersion`: immutable version/revision selected for this use.

Authority-owned reference equality compares `AuthorityKey`, `ResourceKind`, `ResourceId`, and `ResourceVersion`. Matching id strings from different authorities or resource kinds are not equal. Agentd may validate tuple syntax and compare/hash values, but it does not infer organization, project, repository, room, path, or policy meaning from an id string.

`MatrixRoomRef` is the one transport-owned exception in the catalog. A
`ProjectRoomBindingRef` owned by `SpecifyProjectAuthority` gives that transport
room project meaning and policy context.

## 2. Resource Reference Catalog

| Reference type | Authoritative owner | Resource kind | Versioning |
| --- | --- | --- | --- |
| `OrganizationRef` | `SpecifyProjectAuthority` | `organization` | `immutable version` |
| `TeamRef` | `SpecifyProjectAuthority` | `team` | `immutable version` |
| `ProjectRef` | `SpecifyProjectAuthority` | `project` | `immutable version` |
| `RepositoryRef` | `SpecifyProjectAuthority` | `repository` | `immutable version` |
| `ProjectRoomBindingRef` | `SpecifyProjectAuthority` | `project_room_binding` | `immutable version` |
| `IssueRef` | `SpecifyProjectAuthority` | `issue` | `immutable version` |
| `RequirementRef` | `SpecifyProjectAuthority` | `requirement` | `immutable version` |
| `FrozenSpecVersionRef` | `SpecifyProjectAuthority` | `frozen_spec` | `immutable version` |
| `ProductWorkflowRef` | `SpecifyProjectAuthority` | `product_workflow` | `immutable version` |
| `RbacPolicyVersionRef` | `SpecifyProjectAuthority` | `rbac_policy` | `immutable version` |
| `QuotaPolicyVersionRef` | `SpecifyProjectAuthority` | `quota_policy` | `immutable version` |
| `CertificationPolicyVersionRef` | `SpecifyProjectAuthority` | `certification_policy` | `immutable version` |
| `MatrixRoomRef` | `MatrixRobrixTransport` | `matrix_room` | `transport state version` |
| `ProjectExecutionSnapshotRef` | `SpecifyProjectAuthority` | `execution_snapshot` | `immutable version` |

The same type names and tuple semantics apply to Specify-backed and
`LocalProjectAuthority` adapters. The `AuthorityKey` makes their records
explicitly distinct and prevents fallback from masquerading as continuity.

## 3. Project Execution Snapshot

`ProjectAuthorityPort` resolves a `ProjectExecutionSnapshotRef` into the exact
fields below. Every row is required; absent values use an explicit empty list or
null authority value defined by the later wire contract.

| Field | Requirement | Meaning |
| --- | --- | --- |
| `snapshot_ref` | `required` | Immutable `ProjectExecutionSnapshotRef`. |
| `authority_key` | `required` | Authority that issued and validates the snapshot. |
| `authority_revision` | `required` | Monotonic authority revision at issuance. |
| `organization_ref` | `required` | Versioned organization context. |
| `team_refs` | `required` | Ordered versioned team context. |
| `project_ref` | `required` | Versioned project context. |
| `repository_bindings` | `required` | One or more versioned repository bindings. |
| `room_bindings` | `required` | Zero or more versioned room bindings. |
| `issue_ref` | `required` | Issue context or explicit null authority value. |
| `requirement_refs` | `required` | Ordered requirement versions. |
| `frozen_spec_version_ref` | `required` | Exact frozen executable spec version. |
| `product_workflow_ref` | `required` | Product workflow version receiving semantic summaries. |
| `rbac_policy_version_ref` | `required` | Authorization policy version. |
| `quota_policy_version_ref` | `required` | Budget/quota policy version. |
| `certification_policy_version_ref` | `required` | Optional/required certification policy version. |
| `issued_at` | `required` | Authority issuance timestamp. |
| `valid_until` | `required` | Last instant permitted by the recovery decision table. |
| `content_sha256` | `required` | Canonical snapshot content hash. |
| `offline_recovery_policy` | `required` | `deny` or `allow_pinned_until_expiry`. |

Each `ExecutionRunId` pins exactly one `ProjectExecutionSnapshotRef` before execution state becomes `running`. The run MUST NOT resolve `latest` after the run is created. Snapshot content is immutable; changed project, repository, binding, spec, workflow, RBAC, quota, or certification policy creates a new snapshot and a new execution authorization decision.

Semantic events and artifact records carry the pinned snapshot ref and content
hash. Specify may advance product workflow state independently, but that does
not rewrite execution inputs for an existing run.

## 4. Repository Binding Rules

A project execution snapshot contains one or more versioned repository bindings. Each binding references one `RepositoryRef`, its authority role (`target`, `source`, `dependency`, or `docs`), and forge locator metadata.

Before a run starts, authority validation selects exactly one target `RepositoryRef` and exactly one immutable base commit SHA. Additional repository bindings are read-only execution inputs for that run. Multi-repository writes require separate coordinated execution runs so each run has one target, lease/audit chain, and delivery result.

Remote URL, forge slug, branch name, checkout path, worktree path, and local filesystem path are locators or metadata, never `RepositoryRef` identity. A branch may move after snapshot issuance; execution and provenance remain pinned to the base commit SHA. A worker checkout mismatch is an execution validation failure, not an authority rebind.

Delivery reports the resulting commit/artifact against the same target
repository and snapshot. Changing target repository or base commit requires a
new snapshot/run, not mutation of the current run.

## 5. Project-Room Binding Rules

A project may have zero or more active room bindings. Binding roles are
`command`, `notification`, or `review`; one binding may carry multiple roles.
For deterministic command routing, a Matrix room has at most one active `command` binding per `AuthorityKey`.

A normalized enterprise command carries both `ProjectRoomBindingRef` and `ProjectExecutionSnapshotRef`. The binding references `ProjectRef`, `MatrixRoomRef`, allowed command classes, and the governing `RbacPolicyVersionRef`.

Room membership is transport input, not sufficient authorization. Specify
evaluates membership/project policy and supplies the pinned `RbacPolicyVersionRef`; agentd enforces the resulting immutable snapshot for execution. A bare Matrix room id MUST NOT dispatch enterprise work. Notification-only/review-only bindings cannot acquire execution leases.

Binding disable/replacement creates a new binding version. Previously accepted
events and runs retain the binding/snapshot version used for their decision.

## 6. Authority Validation and Recovery

The decision is fail-closed except for one explicit, bounded existing-run
recovery mode.

| Operation | Authority state | Snapshot state/policy | Decision | Conditions |
| --- | --- | --- | --- | --- |
| `new_execution` | `configured_specify_live` | `validated_current` | `allow` | Pin returned snapshot before run starts. |
| `new_execution` | `configured_specify_unavailable` | `any` | `deny` | No local/cache fallback. |
| `new_execution` | `local_explicit_live` | `validated_current` | `allow` | Specify is not configured; local adapter was explicitly selected. |
| `existing_recovery` | `authority_live` | `validated_pinned` | `allow` | Revalidate the same snapshot and unchanged run inputs. |
| `existing_recovery` | `authority_unavailable` | `deny_or_missing` | `deny` | Default and absent policies fail closed. |
| `existing_recovery` | `authority_unavailable` | `allow_pinned_unexpired_unchanged` | `allow` | Same run, snapshot, target repo/base commit, spec/policies; current time is before `valid_until`. |
| `existing_recovery` | `authority_unavailable` | `expired_or_changed` | `deny` | Expiry or any input change requires live reauthorization. |

The default offline recovery policy is `deny`. Configured Specify failure MUST NOT select `LocalProjectAuthority`. An allowed offline recovery may restore a worker/runtime attempt for the already-authorized run, but it cannot create another run, change repository/base commit/spec/policy, deliver source, request certification, or modify product workflow authority state while validation is unavailable.

When live authority returns, the control plane revalidates before the next
authority-sensitive operation. Discovery of expiry, revocation, or mismatch
blocks that operation and records an execution audit event.

## 7. Authority Rebind

Authority changes are explicit imports, not configuration fallbacks. An `AuthorityRebindRecord` is immutable and contains:

- old authority key and resource refs;
- new authority key and resource refs;
- operator, reason, created time, and mapping hash;
- import evidence/status and any resources intentionally not mapped.

Reference equality includes `AuthorityKey` even when two resource id strings match. A successful rebind creates new refs/snapshots for future runs. Historical runs, artifacts, audit events, and certification requests retain their original snapshot and authority references. No background job rewrites historical references in place.

Failed/partial import remains visible and cannot switch active authority.
Activating the new authority is a separate audited decision after required
mappings validate.

## 8. Base Compatibility Classification

The base schema predates P264-P266. P266 preserves behavior but freezes the
following fields as compatibility data:

| Base field | P266 classification | Future treatment |
| --- | --- | --- |
| `projects.id` | `import alias` | Maps explicitly to a `ProjectRef`; not authority by itself. |
| `projects.name` | `projection` | Display/search projection from project snapshot. |
| `projects.repo_path` | `locator` | Local checkout hint; never repository identity. |
| `projects.github_repo` | `locator` | Forge locator metadata; never repository identity. |
| `projects.matrix_room_id` | `transport hint` | Legacy room locator; never binding authority. |
| `issues.id` | `cache` | Per-run Specify context cache key. |
| `issues.project_id` | `import alias` | Legacy project association awaiting explicit mapping. |
| `runs.project_id` | `import alias` | Future runs pin `ProjectExecutionSnapshotRef`; old values remain readable. |
| `matrix_events.project_id` | `projection` | Routing/audit projection, not project authority. |
| `matrix_events.room_id` | `transport hint` | Matrix transport locator. |
| `agent_scheduler_queue.room` | `transport hint` | Unvalidated legacy correlation metadata. |

None of these base fields is a project authority record, canonical `RepositoryRef`, or canonical `ProjectRoomBindingRef`. Later migrations add explicit refs/mappings before deprecating fields; P266 does not alter or delete them.

## 9. Consequences

- P267 agent/worker schema stores foreign snapshot/project refs on execution
  records but creates no project authority table.
- P269 `ProjectAuthorityPort` payloads must represent every required snapshot
  field and validation outcome without relying on null/implicit fallback.
- Matrix gateway work must normalize room commands into binding/snapshot refs
  before execution dispatch.
- P268 artifact/audit/provenance records retain snapshot ref/hash and target
  repository/base commit.
- Base project rows remain compatibility-only until an explicit migration slice
  defines mapping, backfill, dual-read, cutover, and rollback.
