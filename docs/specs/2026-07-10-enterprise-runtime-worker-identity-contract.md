# Enterprise Runtime and Worker Identity Contract

- **Date**: 2026-07-10
- **Status**: Decided by P265
- **Authority basis**: `2026-07-10-enterprise-execution-ownership-boundary.md`
- **Scope**: identity, relationships, lifecycle states, fencing, and loss/recovery semantics

This contract defines the identity model that P266-P268 schemas and later APIs,
worker protocols, scheduler leases, and native runtime code must preserve. It
does not change current storage or runtime behavior.

## 1. Identity Principles

1. Identity and location are separate. A durable record never uses a host name,
   PID, PTY, Matrix room, tmux target, worktree path, or provider resume token as
   its primary identity.
2. Capability and process are separate. An agent profile describes reusable
   execution capability; it is not online, busy, or attached to one process.
3. Logical session and process attempt are separate. Resume keeps the logical
   session while allocating a new attempt.
4. Enrollment and daemon lifetime are separate. A worker keeps its enrollment
   id while every registration/restart gets a new incarnation id.
5. Task ownership is fenced. A lease id alone is insufficient; every accepted
   worker mutation also carries the current task-scoped fencing token.
6. Project context remains a foreign authority reference owned by Specify or
   `LocalProjectAuthority` under P264.

## 2. Canonical Identity Catalog

| Type | Prefix | Authoritative owner | Durability | Meaning |
| --- | --- | --- | --- | --- |
| `AgentProfileId` | `ap_` | `AgentdControlPlane` | `durable` | Reusable model/runtime/role/capability and prompt-policy profile. |
| `WorkerId` | `wk_` | `AgentdControlPlane` | `durable` | Stable enrolled worker independent of host name or daemon lifetime. |
| `WorkerIncarnationId` | `wi_` | `AgentdControlPlane` | `durable` | One accepted worker daemon registration epoch. |
| `RuntimeSessionId` | `rs_` | `AgentdControlPlane` | `durable` | One logical agent interaction across zero or more attempts. |
| `RuntimeAttemptId` | `ra_` | `AgentdControlPlane` | `durable` | One spawn or resume attempt on one worker incarnation. |
| `ExecutionRunId` | `r_` | `AgentdControlPlane` | `durable` | One execution workflow run tied to foreign project/spec references. |
| `ExecutionTaskId` | `tr_` | `AgentdControlPlane` | `durable` | One schedulable execution task within a run. |
| `LeaseId` | `ls_` | `AgentdControlPlane` | `durable` | One bounded ownership grant for one execution task. |

New canonical ids use the shown prefix plus a ULID payload. IDs are immutable and never reused, including after deletion, retry, resume, import, or authority rebind. Consumers MUST treat every canonical id as opaque outside syntax validation, equality, indexing, and logging. `r_` and `tr_` preserve the existing core prefixes; the other six prefixes are new and must not alias current values.

`FencingToken` is not an id. It is an unsigned, task-scoped, monotonically
increasing integer used with `LeaseId` to order ownership grants.

## 3. Legacy Value Classification

| Existing value | P265 classification | Future treatment |
| --- | --- | --- |
| `agents.id` | `import alias` | Maps to one `AgentProfileId` through an explicit compatibility record. |
| `agents.mxid` | `transport metadata` | Matrix identity reference; never an agent profile or worker id. |
| `agents.server` | `placement metadata` | Legacy placement hint; import may resolve it to a worker mapping. |
| `host_name` | `placement metadata` | Mutable inventory label on a worker/incarnation. |
| `backend_target` | `attempt locator` | Opaque runtime adapter address scoped to one attempt. |
| `session_name` | `attempt locator` | Legacy runtime adapter locator scoped to one attempt. |
| `pane_id` | `attempt locator` | Legacy tmux metadata scoped to one attempt. |
| `pid` | `attempt metadata` | Worker-local operating-system observation scoped to one attempt. |
| `native_session_ref` | `resume locator` | Provider-specific opaque resume metadata, not logical session identity. |
| `dispatch_queue.ticket` | `import input` | Legacy queue correlation value; never a lease or execution task id. |
| `workdir` | `attempt metadata` | Mutable worker-local path scoped to an attempt/task allocation. |

Matrix room ids, repository paths, worktree paths, and provider resume refs MUST NOT be parsed into agentd canonical ids. Import and compatibility layers store explicit mappings; string equality between a legacy value and a canonical id never creates a relationship.

## 4. Relationship Model

| Child/type | Authoritative parent | Required references | Rule |
| --- | --- | --- | --- |
| `AgentProfileId` | `AgentdControlPlane catalog root` | policy snapshot refs | A profile is reusable and has no live worker/process ownership. |
| `WorkerId` | `AgentdControlPlane enrollment root` | enrollment policy ref | A worker survives daemon restart and placement-label changes. |
| `WorkerIncarnationId` | `WorkerId` | registration version | Exactly one current incarnation may report for a non-retired worker. |
| `ExecutionRunId` | `ProjectAuthorityRef` | project, repository, frozen-spec and policy versions | agentd owns execution state while the parent reference stays foreign. |
| `ExecutionTaskId` | `ExecutionRunId` | requested capability/profile constraints | A task is the unit that leases fence and mutate. |
| `LeaseId` | `ExecutionTaskId` | `WorkerIncarnationId`, `FencingToken` | A lease grants bounded task ownership to one current incarnation. |
| `RuntimeSessionId` | `ExecutionTaskId` | `AgentProfileId`, active/current `LeaseId`, `ProjectAuthorityRef` | A logical session can outlive any one process attempt. |
| `RuntimeAttemptId` | `RuntimeSessionId` | `WorkerIncarnationId`, active/current `LeaseId` | An attempt locates one spawn/resume on one worker incarnation. |

`ProjectAuthorityRef` is the tuple of authority key, resource kind, immutable
resource id, and referenced version supplied through `ProjectAuthorityPort`.
agentd stores it verbatim and does not derive it from Matrix or filesystem
state. P266 refines its concrete resource catalog and snapshot relationships.

Live process metadata is scoped by both `RuntimeAttemptId` and `WorkerIncarnationId`. Relationships use references between ids; concatenated or compound ids are forbidden.

## 5. Lifecycle Contracts

| Record kind | Complete state vocabulary | Terminal states | New identity trigger |
| --- | --- | --- | --- |
| `AgentProfile` | `active`, `disabled`, `retired` | `retired` | Replacement after retirement allocates a new `AgentProfileId`. |
| `Worker` | `online`, `draining`, `offline`, `retired` | `retired` | Registration/restart allocates a new `WorkerIncarnationId`, not a new `WorkerId`. |
| `RuntimeSession` | `requested`, `starting`, `running`, `resume_pending`, `completed`, `failed`, `cancelled`, `lost` | `completed`, `failed`, `cancelled`, `lost` | Retry after terminal state allocates a new `RuntimeSessionId`. |
| `RuntimeAttempt` | `starting`, `running`, `exited`, `gone` | `exited`, `gone` | Every spawn/resume allocates a new `RuntimeAttemptId`. |
| `ExecutionRun` | `pending`, `running`, `succeeded`, `failed`, `cancelled` | `succeeded`, `failed`, `cancelled` | Re-execution after terminal state allocates a new `ExecutionRunId`. |
| `ExecutionTask` | `queued`, `leased`, `running`, `succeeded`, `failed`, `cancelled`, `dead_letter` | `succeeded`, `failed`, `cancelled`, `dead_letter` | Replacement after terminal state allocates a new `ExecutionTaskId`. |
| `Lease` | `active`, `released`, `expired`, `cancelled`, `superseded` | `released`, `expired`, `cancelled`, `superseded` | Every acquisition/retry allocates a new `LeaseId` and greater `FencingToken`. |

Allowed transitions are:

- Agent profile: `active <-> disabled`; either nonterminal state may become
  `retired`.
- Worker: `online -> draining|offline|retired`, `draining -> online|offline|retired`,
  and `offline -> online|retired`. Returning online requires a newly accepted
  incarnation.
- Runtime session: `requested -> starting|cancelled`; `starting -> running|resume_pending|failed|cancelled|lost`; `running -> resume_pending|completed|failed|cancelled|lost`; `resume_pending -> starting|cancelled|lost`.
- Runtime attempt: `starting -> running|exited|gone`; `running -> exited|gone`.
- Execution run: `pending -> running|cancelled`; `running -> succeeded|failed|cancelled`.
- Execution task: `queued -> leased|cancelled|dead_letter`; `leased -> running|queued|failed|cancelled|dead_letter`; `running -> succeeded|failed|cancelled`.
- Lease: `active -> released|expired|cancelled|superseded`.

Terminal records MUST NOT transition back to an active state. Re-registration, resume, retry, or replacement allocates the new incarnation, attempt, lease, or task identity named in the table.

## 6. Failure and Recovery Invariants

### Worker Reincarnation

`WorkerId` remains unchanged across daemon restarts and registrations. Every accepted registration allocates a new `WorkerIncarnationId`. The new incarnation atomically supersedes the prior incarnation before it can acquire work.

After supersession, heartbeat, capacity, live-process, artifact, and lease reports from a superseded incarnation MUST be rejected and audited. Superseding an incarnation does not delete the worker enrollment or its history. An operator retirement is the only terminal worker transition.

### Runtime Attempt Loss

A `RuntimeSessionId` is stable across spawn and resume attempts. Every spawn or resume allocates a new `RuntimeAttemptId`. A missing process marks only the current attempt `gone` and moves the runtime session to `resume_pending` while recovery is evaluated.

When a valid native resume ref and retry policy exist, recovery keeps the same `RuntimeSessionId` and creates a new `RuntimeAttemptId` on a current worker incarnation. Without a valid resume path, the session becomes terminal `lost` with reason `runtime_gone`. A PID, PTY, backend address, session name, and native resume ref never replace either runtime id.

Worker loss also invalidates every unacknowledged live-process reference for its
incarnation. It does not directly declare execution success/failure or mutate
Specify product workflow state.

### Lease Fencing

An active ownership grant binds one `ExecutionTaskId`, one `WorkerIncarnationId`, one `LeaseId`, and one `FencingToken`. The control plane allocates a token strictly greater than every earlier token for that task before exposing the lease as active.

Reports carrying an older token MUST be rejected and audited. Reports for a terminal lease MUST be rejected and audited. Retry allocates a new `LeaseId`; it never reactivates an expired, released, cancelled, or superseded lease. Wall-clock timestamps never override lease-token ordering.

### Foreign Authority Failure

A cached `ProjectAuthorityRef` remains a reference, not local ownership. If a
configured Specify authority cannot validate the required project/spec/policy
version, new execution fails closed under P264. Runtime or worker recovery may
continue only when its already-authorized immutable snapshot remains valid for
that operation.

## 7. Compatibility and Follow-On Slices

- P265 does not modify P200-P264 rows, APIs, or behavior. Current
  `agents.id`-based surfaces remain compatibility inputs until a later migration
  supplies explicit mappings.
- P266 defines the cross-authority project-room-repository reference model. It
  must not create agentd-owned project authority tables.
- P267 maps the identity catalog to the additive agent/worker/runtime schema and
  migration `0013`.
- P268 defines execution artifact/audit identities and their relationship to
  OpenFab certification references in migration `0014`.
- P269-P279 implement authority adapters, leases, worker recovery, native
  runtime, policy enforcement, and operations against these identities.
