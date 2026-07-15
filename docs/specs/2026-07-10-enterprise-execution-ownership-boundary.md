# Enterprise Execution Ownership Boundary

- **Date**: 2026-07-10
- **Status**: Decided by P264
- **Amends**: `2026-05-29-agentd-specify-boundary.md`
- **Scope**: source-of-truth ownership across Specify, agentd, workers,
  OpenFab, Matrix, and Robrix

This amendment resolves enterprise ownership that the Path B decision left
implicit. It preserves Specify as project authority and agentd as the execution
system, but splits durable execution control from worker-local runtime state.
Where the replacement roadmap or the Path B document is ambiguous about these
roles, this document is authoritative.

P264 is an architecture contract only. It does not select storage technology,
define wire payloads, or implement the ports named below.

## 1. Role Definitions

- `SpecifyProjectAuthority` is the source of truth for organization, project,
  repository, issue/spec lifecycle, product workflow, project membership,
  policy intent, and project-to-Matrix-room binding.
- `AgentdControlPlane` is the source of truth for durable execution identity,
  dispatch, leases, runtime sessions, checkpoints, execution artifacts, audit,
  and measured quota usage. It is not a second project/spec product.
- `AgentdWorker` owns ephemeral resources needed to execute work on one host.
  It reports durable facts to `AgentdControlPlane` and can be replaced without
  changing project or execution history.
- `OpenFabCertificationAuthority` is the source of truth for certification
  requests, results, signatures, and provenance attestations.
- `MatrixRobrixTransport` owns Matrix identities, rooms, interaction, commands,
  and notifications as transport state. It is not durable project, execution,
  artifact, transcript, lease, or certification storage.

An authoritative owner validates writes and resolves conflicts for its state
class. Other roles may keep immutable references, projections, or bounded
caches, but those copies cannot override the authoritative record.

## 2. State Ownership

| State id | Authoritative owner | Replicas or consumers | Ownership rule |
| --- | --- | --- | --- |
| `organization_team` | `SpecifyProjectAuthority` | agentd policy snapshot | Organization and team identity originate in Specify. |
| `project_repository` | `SpecifyProjectAuthority` | agentd run reference | Project and repository context is versioned by Specify. |
| `matrix_project_binding` | `SpecifyProjectAuthority` | Matrix gateway routing projection | The binding is project state even though Matrix transports commands. |
| `issue_requirement_spec` | `SpecifyProjectAuthority` | agentd immutable execution input | Specify owns issue, requirement, review, freeze, and spec versions. |
| `product_workflow_state` | `SpecifyProjectAuthority` | agentd semantic event projection | Specify owns product progress such as review-ready and accepted. |
| `project_rbac_policy` | `SpecifyProjectAuthority` | agentd enforcement snapshot | Specify authors membership, RBAC, quota, and delivery policy. |
| `certification_policy_intent` | `SpecifyProjectAuthority` | agentd and OpenFab policy reference | Specify selects optional or required certification for a project. |
| `worker_registry` | `AgentdControlPlane` | worker heartbeat projection | Enrollment, health, capacity, drain state, and trust labels are durable execution state. |
| `agent_capability_registry` | `AgentdControlPlane` | scheduler projection | Executable agent profiles and worker placement are control-plane state. |
| `execution_queue_lease` | `AgentdControlPlane` | worker-held fencing token | Queue order, lease owner, fencing, TTL, retry, and recovery are durable. |
| `runtime_session_record` | `AgentdControlPlane` | worker live-process reference | Runtime identity and recoverability survive worker loss. |
| `execution_run_checkpoint` | `AgentdControlPlane` | Specify semantic summary | Run, task, checkpoint, and resume state belong to execution control. |
| `execution_artifact_index` | `AgentdControlPlane` | Specify and OpenFab immutable references | Hashes and durable transcripts index execution outputs separately from certification. |
| `execution_audit_usage` | `AgentdControlPlane` | Specify quota and audit projection | agentd records enforcement decisions, model usage, and execution audit events. |
| `live_process_pty` | `AgentdWorker` | control-plane runtime status | The live process, PTY, and bounded output ring exist only on the executing host. |
| `worktree_cache_transcript_spool` | `AgentdWorker` | control-plane artifact acknowledgements | Worktrees, caches, and unacknowledged upload spool are disposable local resources. |
| `certification_attestation` | `OpenFabCertificationAuthority` | Specify and agentd immutable references | OpenFab owns certification results, signatures, and provenance attestations. |
| `matrix_identity_room_transport` | `MatrixRobrixTransport` | Specify binding reference and agentd gateway cursor | Matrix owns protocol identity and room transport, not project meaning or execution history. |

## 3. Negative Boundaries

- `SpecifyProjectAuthority` MUST NOT own worker registrations, runtime sessions, execution leases, checkpoints, or transcripts.
- `AgentdControlPlane` MUST NOT own organization/project creation, issue/spec
  review or freeze, product workflow authority, Matrix command authority, or
  certification signatures.
- `AgentdWorker` MUST NOT become the durable source of truth for projects, specs, runs, tasks, leases, artifacts, or transcripts.
- `OpenFabCertificationAuthority` MUST NOT own execution queues, runtime sessions, task leases, commits, or pull requests.
- `MatrixRobrixTransport` MUST NOT own canonical project bindings, execution
  state, runtime transcripts, artifacts, leases, or certification decisions.

Fencing and lease recovery remain control-plane decisions even when the worker
holds the current fencing token. A disappeared worker may lose an unacknowledged
local spool, but it cannot erase or rewrite acknowledged execution history.

## 4. Protocol Seams

### 4.1 ProjectAuthorityPort

`ProjectAuthorityPort` supplies immutable project, repository, room-binding,
issue/spec, workflow, RBAC, quota, and certification-policy snapshots to
agentd. It accepts idempotent semantic execution summaries and artifact
references; it does not accept worker-local process state as product truth.

Enterprise mode uses a Specify-backed implementation. Standalone mode uses the
same call semantics and stable identifiers: `LocalProjectAuthority` implements `ProjectAuthorityPort` rather than introducing a second project model.

Authority selection follows this precedence:

1. When Specify is configured, `SpecifyProjectAuthority` is authoritative.
2. If a configured Specify endpoint returns an error or is unavailable, agentd
   MUST fail closed and MUST NOT silently fall back to `LocalProjectAuthority`.
3. When Specify is not configured, standalone mode selects
   `LocalProjectAuthority` explicitly.
4. Changing authority mode requires an explicit import/rebind operation; a
   restart or transient network error cannot change the owner.

Both modes use the same stable project, repository, workflow, task, and policy identity model. Later specs may define their concrete representations.

### 4.2 WorkerFleetPort

`WorkerFleetPort` separates `AgentdControlPlane` from `AgentdWorker`. It will
cover authenticated registration, heartbeat, capability/capacity reporting,
pull acquisition, lease renewal/release, cancellation, drain, artifact upload
acknowledgement, and offline recovery. A worker receives snapshots and fenced
leases; it does not author project policy or durable task ownership.

### 4.3 CertificationPort

`CertificationPort` submits an immutable execution-artifact reference and
project policy reference to OpenFab, then records the returned request id,
result, signature, and attestation reference. OpenFab does not mutate the
execution run or deliver source changes.

The default integration remains `gate=none`. Under this default, certification
evidence can be requested and recorded without blocking delivery. `deliver` and `certify` are separate decisions; a later project policy may require a
certification result, but that policy does not transfer execution ownership to
OpenFab.

## 5. Policy, Quota, Artifact, and Audit Split

Specify authors project membership, RBAC, quota budgets, model restrictions,
delivery rules, and certification-policy intent. `AgentdControlPlane` consumes
a versioned policy snapshot, enforces it for execution, and owns the resulting
decision log and measured usage. Enforcement does not make agentd the policy
author, and policy authorship does not make Specify the runtime owner.

Execution artifacts, including durable transcript objects, logs, patches,
commits, test reports, and their hashes, are indexed by `AgentdControlPlane`.
Specify stores project-facing references and lifecycle summaries. OpenFab owns
only certification requests/results, signatures, and attestations over
immutable artifact references. Matrix receives summaries and actionable
notifications, never the canonical runtime transcript.

## 6. Deployment Modes

### Standalone

- `LocalProjectAuthority` is selected explicitly because no Specify endpoint is
  configured.
- The embedded `AgentdControlPlane` and local `AgentdWorker` still communicate
  through the same logical ports and identity model used in enterprise mode.
- `CertificationPort` may be disabled; absence of OpenFab does not block
  delivery under `gate=none`.

### Enterprise

- Specify is the configured `ProjectAuthorityPort` implementation and errors
  fail closed.
- A durable `AgentdControlPlane` schedules one or more replaceable workers.
- Matrix/Robrix transports human commands and notifications using the binding
  owned by Specify.
- OpenFab certification is optional or policy-required while remaining
  independent from source delivery and execution ownership.

## 7. Consequences for Later Specs

- P265 and later identity/schema slices must preserve these owner names and may
  add references or projections, not compound owners.
- APIs must route writes to the authoritative role and expose owner/version
  metadata for cached snapshots.
- Worker loss, Matrix replay, Specify outage, and OpenFab outage require
  separate recovery behavior because they affect different authorities.
- Moving a state class to another authority requires a new architecture
  decision and migration contract; implementation convenience is insufficient.
