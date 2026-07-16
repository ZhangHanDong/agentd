# agentd Enterprise Execution and Agent-Chat Replacement Roadmap

- Status: canonical single active roadmap; feature-branch candidate baseline
  committed; FSF-0 acceptance and PRD/ADR ratification still gate integration
- Revised: 2026-07-12
- Scope: agentd execution control plane, workers, runtime, Matrix gateway, and
  agent-chat replacement boundaries

This is the single active agentd roadmap. AD-E0 candidate reconciliation has
produced one reviewable feature-branch baseline, but promotion or integration
still requires the FSF-0 acceptance record and the OpenFab PRD/ADR decomposition
gate. This roadmap replaces the previous parallel E-series and N-series
presentation; historical native-runtime ideas are mapped in a non-executable
appendix and do not reserve task IDs.

The roadmap follows the cross-system ownership decision:

    SpecifyProjectAuthority
      -- immutable ProjectExecutionSnapshot -->
    AgentdControlPlane
      -- authenticated fenced lease -->
    AgentdWorker
      -- explicit outcome + content-addressed artifacts -->
    AgentdControlPlane
      -- immutable evidence reference -->
    OpenFabCertificationAuthority

    MatrixRobrixTransport carries commands and summaries.
    ARC compiles approved requirements; it does not execute work.

Specify is required in enterprise mode. Specify-web UX, implementation language,
storage engine, and deployment topology are deferred and are not agentd scope.

OpenFab's current PRD remains authoritative until amended. In an OpenFab-governed
run, OpenFab retains its spec-cycle and `BasePort` orchestration contract while
agentd is the durable execution implementation behind that port. Direct
Robrix-to-agentd delivery is a separate policy-selected profile and does not
silently acquire OpenFab certification.

## 1. Goal

Build agentd into the long-term execution system for the software factory:

- a durable execution control plane;
- an authenticated, replaceable worker fleet;
- policy-constrained scheduling with fenced leases;
- native agent process/session management;
- explicit workflow outcomes and replayable semantic events;
- Matrix/Robrix command and notification integration;
- immutable execution evidence for independent OpenFab certification;
- one execution model from standalone development to a 10,000-developer
  enterprise deployment.

The target is not agent-chat behind a Rust tmux API. The target is an execution
system whose correctness does not depend on tmux names, dashboard state, Matrix
history, worker-local files, or self-reported agent output.

## 2. Non-Goals

- No tmux, rmux, or herdr compatibility layer as a long-term product contract.
- No preservation of tmux session names, pane addresses, PIDs, host names, or
  legacy command shapes as durable identity.
- No project/spec product inside agentd. Specify or explicit local authority owns
  project context and spec lifecycle.
- No Matrix-as-transcript transport. Matrix carries commands, decisions, and
  semantic summaries.
- No OpenFab certification implementation inside agentd.
- No ARC design/implement/convergence agent loop in agentd.
- No silent fallback from configured Specify authority to local authority.
- No requirement that Specify-web be designed before the protocol boundary can
  be implemented and tested.
- No Kubernetes-first architecture. Orchestration cannot repair missing identity,
  lease, artifact, sandbox, or recovery semantics.

## 3. Authoritative Boundaries

### 3.1 Specify Project Authority

Specify is the enterprise `ProjectAuthorityPort` implementation and owns:

- organization, team, project, and repository context;
- project-to-Matrix-room binding;
- issue, requirement, spec review, and frozen spec version;
- product workflow state;
- project membership, RBAC, quota, model, and delivery policy intent;
- optional or required OpenFab certification policy intent.

agentd consumes immutable references and policy snapshots. It does not create a
second project database or follow an unversioned latest spec during execution.

When Specify is configured and unavailable, new resolution fails closed. A
cached snapshot can be used only when its policy explicitly allows pinned
offline recovery and it remains within its validity window.

### 3.2 Agentd Execution Control Plane

`AgentdControlPlane` owns:

- durable agent-profile, worker, worker-incarnation, runtime-session, and
  runtime-attempt identity;
- execution runs, tasks, attempts, checkpoints, and resume state;
- queue ordering, lease ownership, fencing epochs, retry, cancellation, and
  dead-letter state;
- artifact metadata, transcript object references, execution audit, and measured
  quota usage;
- worker registration, health, capacity, trust labels, and drain state.

The control plane owns command acceptance, the resulting run, and execution
effects. It does not own the Matrix sync cursor itself.

Project authority snapshots are immutable inputs. Certification references are
immutable outputs owned by OpenFab. Neither becomes agentd-authored truth.

### 3.3 Agentd Worker

An `AgentdWorker` owns disposable local execution resources:

- live process and PTY;
- bounded output ring;
- worktree and dependency/model caches;
- native Claude/Codex session reference;
- unacknowledged artifact/transcript upload spool.

The worker reports durable facts to the control plane. Losing a worker cannot
erase acknowledged task, lease, artifact, or audit history.

### 3.4 OpenFab Certification Authority

OpenFab owns certification requests, verification results, signatures,
provenance, conformance, and revocation state. agentd sends immutable evidence
references and records returned references.

`gate=none` keeps direct delivery possible and may produce machine-attestation
evidence. It does not imply human certification. A project policy can require a
human/N-of-M result without transferring execution ownership to OpenFab.

### 3.5 Matrix and Robrix

Matrix/Robrix owns human interaction and transport state. Specify owns the
project meaning of a room binding; `AgentdMatrixGateway` owns its gateway cursor,
and `AgentdControlPlane` owns execution effects produced by an authorized command.

Raw runtime transcripts stay in execution storage. Matrix receives bounded
summaries, decisions, failure notices, and actionable links.

`AgentdMatrixGateway` is agentd's `MatrixRobrixTransport` implementation. It owns
the Matrix sync cursor, processed-event inbox, command/run deduplication ledger,
command normalization, and transport outbox. Its atomic handoff to
`AgentdControlPlane` binds one canonical `command_id` to at most one `run_id`.
That handoff requires the gateway and control plane to share one durable
execution store and commit the inbox row, run row, and outbox event in one local
transaction; the shared transaction boundary is a contract requirement, and
splitting the stores requires a new ADR rather than a distributed-transaction
workaround.

## 4. Protocol Model

### 4.1 ProjectAuthorityPort

The unmerged P269 candidate proposes the read side of this logical port:

- `resolve(expected_authority, project_ref, optional_snapshot_ref)`;
- `refresh(snapshot_ref)`;
- `health()`.

The returned `ProjectExecutionSnapshot` pins:

- authority key/revision;
- organization, team, project, repository, and base commit;
- Matrix room binding and allowed command classes;
- requirement and frozen-spec references;
- product workflow reference;
- RBAC, quota, and certification policy versions;
- data classification, allowed worker trust domains/regions, execution image,
  and cache-isolation policy;
- issued/expiry time, revocation epoch, content SHA-256, and offline-recovery
  policy.

Successful adapters validate authority, project, snapshot identity, validity,
repository target, room binding, and content hash before execution receives the
snapshot.

Revocation is checked at dispatch, lease renewal, artifact acceptance, delivery,
and release. Project policy decides whether emergency revocation cancels, drains,
or quarantines already-running work; it never authorizes a new run from an
expired or revoked snapshot.

P264 also requires idempotent execution-summary and artifact-reference
projection back to Specify. The implementation may separate read and command
traits, but together they remain the logical `ProjectAuthorityPort` seam. AD-E0
must reconcile that full contract before P269 can be called current capability.

### 4.2 Execution Identity

Durable identities are distinct:

- agent profile;
- worker enrollment;
- worker incarnation;
- logical runtime session;
- runtime attempt;
- execution run;
- execution task;
- lease;
- artifact;
- audit event.

Legacy agent names, Matrix IDs, host names, backend targets, tmux sessions,
pane IDs, PIDs, native session refs, and paths are aliases or metadata. They are
never durable ownership keys.

Until a separate task-attempt identity is ratified, an execution attempt is the
tuple `(ExecutionTaskId, LeaseId, fencing_epoch)`. This roadmap does not claim
that P265 already defines an `ExecutionTaskAttemptId`.

### 4.3 Lease and Fencing

Lease semantics are defined before worker acquisition:

- every dispatch creates a new immutable lease and monotonically increasing
  task-scoped fencing token;
- worker incarnation and task attempt are bound into the claim;
- renew/release/cancel/expire use compare-and-swap against current ownership;
- reassignment advances fencing before a new worker can mutate task state;
- late output, usage, or artifact publication from an old epoch is rejected and
  audited;
- retries are idempotent; dead-letter and operator recovery are explicit;
- one transaction updates lease ownership and appends the corresponding outbox
  event.

### 4.4 WorkerFleetPort

The fleet protocol covers:

- authenticated enrollment and version negotiation;
- worker-incarnation registration and heartbeat;
- capability, runtime, model, trust-domain, zone, and capacity reporting;
- drain/offline state;
- pull-based lease acquisition;
- lease renewal/release/cancellation;
- artifact upload and acknowledgement;
- execution summary and measured usage reporting;
- bounded offline recovery.

Service identity uses mTLS or an equivalent workload identity. A trust-domain
label is routing metadata, not authentication.

### 4.5 Execution Evidence

Every accepted task result links:

- authority snapshot and policy versions;
- run/task/attempt/lease/fencing identities;
- agent profile, worker incarnation, runtime/model, and sandbox profile;
- base commit, diff/commit, artifact hashes, and storage references;
- requirement, ARC compiler/configuration, spec, prompt, plan, and skill-package
  hashes;
- test commands, status, logs, and timestamps;
- review and human-decision references;
- retries, recovery, usage, and policy decisions.

Workers cannot certify this envelope. OpenFab policy decides which claims must be
independently re-run in an OpenFab-controlled sandbox.

### 4.6 Event Semantics

Durable events contain:

- globally unique event ID and schema version;
- producer, tenant/project, run/task identities;
- correlation and causation IDs;
- occurred time and durable sequence;
- authoritative owner and payload digest.

Cross-system delivery is at-least-once. Consumers provide durable inbox,
idempotency, bounded replay, and explicit projection ordering.

For Matrix commands, `command_id` is unique within the authoritative
room/project binding. The processed-command inbox row, resulting `run_id`, and
outbox event commit atomically; replay returns the prior result. The gateway
advances its cursor only after that transaction is durable.

## 5. Current Candidate Baseline

The agent-chat replacement work has produced many P200-series slices across
worktrees. Their merge and parity status must be read from git history,
agent-spec lifecycle, and the parity audit; this roadmap does not infer
completion from a filename.

As of 2026-07-12, P200-P271 exist as reviewable commits on feature branch
`agentd/tr_01KWWTVEK1AC6C836SXSP7Y3Q3`. They are not merged to the main
integration branch, released, or enterprise-ready.

- P200-P262 compatibility baseline: `f0f3acc`.
- P263 reconciliation: `85518ad`.
- P264 ownership: `c22b402`.
- P265 identity: `0ea1472`.
- P266 authority references: `76ae37d`.
- P267 runtime/worker store: `9634104`.
- P268 artifact/audit store: `cbd0993`.
- P269 project-authority API: `670ad0e`.
- P270 durable leases/fencing: `d9d05dd`.
- P271 execution-evidence APIs: `9ec92bb`.

As of 2026-07-15, an AD-E1 minimum baseline candidate also exists on isolated
branch `agentd/ad-e1-security-baseline`. It is not an AD-E1 or FSF-2 exit and is
not authorized for integration or production deployment:

- core security boundaries: `07120fc`;
- fenced capability persistence: `620618c`;
- workload identity and scoped secret checkout: `57415c8`;
- OCI sandbox and cleanup baseline: `49e8597`;
- ordered enterprise composition and listener-before-startup rejection:
  `c5130d5`;
- trusted-clock, audit-only-auth, cancellation teardown, compound failure, and
  stable provider-unavailable remediation: `0be8baf`.
- trusted-clock lease and action-capability revalidation immediately before
  every external side effect: `368d8f3`.

The candidate exposes a protected-operation composition API through explicit
provider injection. It does not add an AD-E2 worker listener or product-specific
provider configuration; enterprise daemon transport remains incomplete and the
CLI continues to reject enterprise startup before bind rather than use the
legacy compatibility host.

The composition revalidates the current lease and action capability immediately
before each external side effect, using a fresh trusted-clock observation for
secret checkout, sandbox preparation, and sandbox execution.

Focused crate tests, failure injection, formatting, workspace tests, workspace
strict Clippy, diff/secret inspection, and the `--ai-mode off` agent-spec
lifecycle have passed. The lifecycle recorded 13 business scenarios plus the
worktree boundary guard (`14/14`) at quality score `1.0`. The OCI selector used
its guarded no-container default; real-container evidence is still pending.
This prose is candidate evidence, not an acceptance record. AD-E0, AD-E1,
FSF-0, and FSF-2 remain incomplete. AD-E2 worker fleet remains incomplete;
AD-E3 Matrix cutover remains incomplete; AD-E4 OpenFab transport remains
incomplete; AD-E5 native runtime remains incomplete; and AD-E7 scale remains
incomplete.

| Older replacement-worktree candidate | Newer agentd-worktree candidate | AD-E0 disposition |
| --- | --- | --- |
| P224 ownership boundary | P264 ownership boundary | P264 is canonical; P224 is retained only as source evidence |
| P225 identity contract | P265 identity contract | P265 is canonical; sibling vocabulary was reconciled by P263 |
| P226 authority references | P266 authority references | P266 is canonical; sibling snapshot fields are non-executable evidence |
| P227 control-plane store | P267 control-plane store | P267 is canonical on the feature branch with base migration `0013` |
| P228 artifact/audit store | P268 artifact/audit store | P268 is canonical on the feature branch with base migration `0014` |
| none | P269-P271 API candidates | Reviewable feature-branch candidates; main integration remains gated |

P263-P271 are the canonical candidate lineage. P224-P228 are explicitly retired
as an executable competing lineage and remain only as source evidence. This
candidate decision does not authorize main integration: FSF-0 acceptance,
OpenFab PRD/ADR ratification, human review, and integration evidence are still
required.

| Candidate | Contract introduced | Roadmap role |
| --- | --- | --- |
| P264 | Specify/agentd/worker/OpenFab/Matrix ownership boundary | Architecture gate |
| P265 | Runtime/worker/execution identity contract | Identity gate |
| P266 | Foreign project/room/repository authority references and immutable snapshot | Authority contract |
| P267 | Agent/worker/runtime additive store model | Control-plane data candidate |
| P268 | Immutable execution artifact and audit store model | Evidence data candidate |
| P269 | Project authority core port, local adapter, Specify adapter seam, and fail-closed control decision | Authority API candidate |
| P270 | Durable dispatch lease and fencing API | Scheduler foundation candidate |
| P271 | Artifact, audit, usage, and certification-reference APIs | Evidence API candidate |

These candidates provide useful foundations, but do not by themselves complete:

- production Specify HTTP/auth transport;
- service/workload identity;
- tenant authorization;
- execution sandbox;
- worker fleet protocol;
- Matrix/Robrix cutover;
- native runtime;
- OpenFab independent verification transport;
- agent-chat migration and final removal.

## 6. Single Active Roadmap

Phase names use `AD-E` and are the only executable phase order in this document.
Phase implementation and promotion still obey their dependency and factory
gates. Future task IDs are assigned only when an agent-spec task is created.

### AD-E0: Ratify Ownership and Candidate Baseline

Purpose: establish one source of truth and integrate the P264-P271 candidates
without overstating their status.

Factory dependency: reconciliation may run in parallel with FSF-0, but candidate
promotion/integration is blocked until the FSF-0 acceptance record and the
OpenFab PRD/ADR decomposition gate exist.

Current status on 2026-07-12:

- candidate-side lineage reconciliation, feature-branch commits, workspace
  tests, Clippy, formatting, and P271 lifecycle verification are complete;
- P224-P228 no longer form an executable competing lineage;
- FSF-0 acceptance, OpenFab PRD/ADR ratification, human candidate review, and
  main integration remain open, so AD-E0 has not passed its exit gate.

Work:

- diff P224-P228 against P264-P268 and record the canonical/retired mapping;
- review P264 against the older Specify boundary, OpenFab PRD, and factory roadmap;
- verify P265 identity names are consistent across core/store/API;
- verify P266 snapshot references do not create agentd-owned project tables;
- verify P267/P268 migrations are additive and backward compatible;
- verify P269 fails closed for configured Specify and selects local authority
  only through explicit composition;
- verify P270 fencing rejects stale worker claims;
- verify P271 records rejected stale evidence before returning the rejection;
- update the parity map with enterprise replacement rows and current status;
- update roadmap-bound P224/P226 contract assertions to the canonical lineage and
  AD-E phase model after human approval;
- record merge/commit/lifecycle evidence per candidate.

Exit gate:

- all integrated candidate specs pass lifecycle and repository verification;
- no state class has two authoritative owners;
- no roadmap or parity row treats tmux/Matrix/path identifiers as durable keys;
- unintegrated candidates remain visibly marked as candidates.
- P224-P228 cannot remain an executable competing lineage.

### AD-E1: Execution Security Foundation

Depends on: AD-E0.

Purpose: make multi-tenant execution safe before expanding the worker fleet.

Current status on 2026-07-17: code-complete candidate; not an AD-E1 or FSF-2
exit. The candidate now covers OIDC and Matrix enterprise principals, workload
mTLS, immutable tenant/project scope, lease/fencing-bound capabilities, local
and remote secret-broker boundaries, OCI sandbox isolation, bounded content
redaction, policy-revocation checkpoints, placement admission, redacted audit,
cleanup, and fail-closed enterprise composition. Production provider wiring,
authenticated AD-E2 transport, real service exercises, cross-surface rollout,
and every exit-gate scenario remain deferred to
`docs/acceptance/ad-e-roadmap-manual-checklist.md`. Candidate code ownership is
not acceptance evidence and does not authorize promotion.

Work:

- OIDC/enterprise-principal mapping for human/API requests;
- workload identity and mTLS for control plane, gateway, and workers;
- scoped repository/model/object-store credentials through a secret broker;
- short-lived attempt capabilities bound to current lease/fencing state for
  forge, object store, secret broker, and high-risk tool actions;
- tenant/project authorization on APIs, queues, audit, and artifact objects;
- Matrix user/device/appservice mapping to enterprise principals, including
  deprovisioning, device revocation, and homeserver trust policy;
- `ExecutionSandbox` contract:
  - ephemeral workspace;
  - resource limits;
  - default-deny egress profiles;
  - syscall/process isolation;
  - read-only or scoped mounts;
  - deterministic cleanup;
- transcript and log secret redaction;
- policy-version pinning in every run;
- snapshot revocation epoch and enforcement at dispatch, renewal, artifact
  acceptance, delivery, and release;
- placement constraints for data classification, worker trust domain/region,
  signed image, dedicated pool, egress, and cache isolation.

Exit gate:

- cross-tenant API and storage access is denied;
- generated code cannot read host or another tenant's credentials/worktree;
- expired/revoked service identity cannot acquire or renew work;
- sandbox escape and cleanup negative tests pass for each production profile;
- shared cache, model cache, egress, and worker reuse tests prove cross-tenant
  isolation.

### AD-E2: Durable Scheduler and Worker Fleet

Depends on: AD-E1.

Purpose: turn the lease foundation into production scheduling across replaceable
workers.

Work:

- transactional queue/lease/outbox state;
- worker enrollment, incarnation, heartbeat, capability/capacity, zone, drain,
  offline, and version negotiation;
- pull acquisition, renew, release, cancel, expire, retry, and dead letter;
- quota/capacity backpressure;
- artifact upload acknowledgement and retry;
- epoch-aware external-side-effect admission for forge, artifact, secret, and
  tool boundaries;
- reaper for stale leases and worker incarnations;
- snapshot/policy validity checks before dispatch and renewal;
- operator explain API for queued/running/blocked/denied/retried tasks.

Exit gate:

- control-plane restart loses no accepted task or lease state;
- worker disappearance does not corrupt task ownership;
- stale fencing epochs cannot publish outcomes, artifacts, or usage;
- stale fencing epochs cannot push, create PRs, deliver, or invoke protected
  external side effects;
- duplicate acquisition/release/upload is idempotent;
- failure-injection covers reassignment, partial upload, and network partition.

Implementation status (2026-07-17): AD-E2 code-complete candidate. Core now
defines the canonical fleet protocol; migration `0018` owns the durable
enterprise queue, worker availability, report receipts, artifact upload
acknowledgements, side-effect admissions, fencing rejection evidence, and
outbox. `SqliteFleetScheduler` implements trusted placement/epoch-aware pull,
transactional lease acquisition, heartbeat/capacity/quota control,
renew/complete/fail/cancel, deterministic retry/dead-letter, reaping, fenced
artifact/side-effect admission, and explain output. `EnterpriseFleetService`
replaces caller workload/time with mTLS identity and trusted clock data. This is
not an AD-E2 or FSF-3 exit: restart, worker-loss, network-partition,
partial-upload, multi-host, rollback, and operator sign-off evidence remains in
the final manual checklist.

### AD-E3: Matrix/Robrix Gateway and Control-Plane Cutover

Depends on: AD-E2.

Purpose: remove agent-chat from command routing while preserving rollback.

Work:

- Specify-owned room/project binding and project ACL snapshot;
- `AgentdMatrixGateway`-owned durable cursor and processed-event store;
- transactional command inbox/run/outbox handoff with canonical `command_id` and
  a unique room/project deduplication constraint;
- trusted inviter, ignored sender, appservice loop suppression, and command
  normalization;
- attachment ingest as content-addressed project input;
- semantic execution summaries and actionable failures;
- Robrix project/run/task/artifact/approval/evidence views;
- migration:
  - observe;
  - shadow-read-only with side effects disabled;
  - canary project;
  - per-project authority/cursor cutover;
  - drain;
  - retire;
- rollback triggers and state mapping for projects, rooms, agents, tasks,
  messages, cursors, and in-flight runs.

Exit gate:

- Robrix binds a project through Specify and dispatches through agentd without
  agent-chat;
- restart/replay produces zero duplicate accepted executions;
- every accepted sender resolves to a live enterprise principal and current
  project authorization;
- shadow mode produces no source, queue, message, or certification side effects;
- canary rollback preserves authority, cursor, run, task, and artifact ownership;
- raw transcripts never enter Matrix.

This is the control-plane/workflow cutover. The transitional runtime may still
exist until AD-E5 passes.

Implementation status (2026-07-17): AD-E3 code-complete candidate. Core now
defines authenticated Matrix provenance, canonical command/outbox ids, closed
cutover modes, typed semantic summaries, reliable delivery, and bounded Robrix
project/run/task/artifact/approval/evidence projections. Migration `0019` owns
Specify-bound project rooms, immutable command/inbox records, transactional
cursor/run/outbox handoff, durable delivery acknowledgement, and cutover
history plus digest-only legacy state mappings and rollback manifests, without
raw command bodies, attachment bytes, logs, or transcripts.
`SqliteMatrixGateway` enforces inviter/sender/appservice, live principal,
organization, current revocation epoch, snapshot, command ACL, mode, and cursor
conditions inside the write transaction; observe and shadow create no execution
side effects, while replay reuses the canonical command/run/outbox identity.
`AgentdMatrixGateway`
uses authenticated transport provenance and trusted time, content-addresses
attachments, and fails startup closed unless identity, storage, delivery, and
clock providers are present. This is not an AD-E3 or FSF-4 exit: real
homeserver/Robrix dispatch, restart replay, canary rollback, service cutover,
and operator sign-off evidence remains in the final manual checklist.

### AD-E4: OpenFab Evidence and Skill Integration

Depends on: AD-E1; integrates with AD-E2 and AD-E3.

Purpose: export verifiable evidence without making agentd a certification
authority.

Work:

- versioned signed execution evidence envelope;
- trusted builder/worker identity and key rotation/revocation;
- `CertificationPort` transport for immutable artifact and policy refs;
- forge admission/status-check protocol that verifies current policy version,
  exact subject digest, and valid OpenFab result before required merge/release;
- delivery, machine-attestation, human-certification, release, and revocation
  state mapping;
- independent OpenFab verification request/result events;
- Skill Hub package/version/hash/permission refs in execution snapshots and
  evidence;
- install only approved, non-revoked packages under project policy;
- preserve historical verification when a package is yanked or revoked.

Exit gate:

- OpenFab can independently verify required claims without trusting worker
  self-report;
- every certification resolves to immutable project, source, spec, evidence,
  policy, and skill digests;
- optional certification failure does not block delivery under `gate=none`;
- policy-required certification blocks release without taking over execution.

### AD-E5: Native Runtime

Depends on: AD-E2.

Purpose: remove tmux from process/session ownership after durable enterprise
semantics exist.

Work:

- `RuntimeBackend` contract separate from legacy spawn-only backends;
- native PTY/process host for Claude Code, Codex, and future CLIs;
- runtime session versus runtime attempt identity;
- send text/keys, resize, interrupt, shutdown, and bounded capture;
- durable transcript archive and content-addressed object refs;
- Claude/Codex native session-ref capture and resume;
- restart recovery that distinguishes live, resumable, and `runtime_gone`;
- SSE semantic event stream, snapshot API, wait API, and idle reaping;
- runtime execution inside AD-E1 sandbox profiles.

Exit gate:

- fake process/PTY tests prove lifecycle and recovery;
- opt-in real smoke starts a supported agent, receives a prompt, calls MCP, and
  submits an explicit outcome;
- dashboard, Robrix, Matrix summaries, and agentctl use the same runtime APIs;
- daemon restart reconstructs or explicitly terminates every runtime state;
- production runtime control no longer depends on tmux.

### AD-E6: Final agent-chat Cutover and Removal

Depends on: AD-E3, AD-E4, and AD-E5.

Purpose: finish replacement only after control-plane and runtime cutovers both
pass.

Work:

- shadow comparison of agent-chat and agentd decisions;
- final supported-state import with stable ID mappings;
- in-flight run drain and cursor handoff;
- local, team-server, and fleet service installation;
- operator preflight, health, doctor, backup, restore, and rollback;
- remove agent-chat/tmux production configuration, documentation, and code paths;
- retain explicit compatibility import only where a migration contract requires
  it.

Exit gate:

- parity audit has no required missing/partial row without an explicit retained
  dependency and approved product-scope decision;
- pilot passes rollback, worker-loss, authority-outage, Matrix-replay, and
  certification-outage drills;
- any remaining gap is documented product scope rather than hidden
  compatibility debt;
- human sign-off authorizes legacy removal;
- after sign-off, production configuration, startup entrypoints, runtime
  dependencies, and operator procedures contain no agent-chat/tmux path;
- any retained legacy support is an explicitly scoped offline import tool.

### AD-E7: Enterprise Scale and Multi-Region Workers

Depends on: AD-E2, AD-E4, and AD-E6.

Purpose: scale the established contracts.

Work:

- highly available control plane and Specify adapter transport;
- Kubernetes worker profile with signed images and rollout audit;
- per-zone worker pools and pull-based dispatch without worker inbound access;
- queue/policy-driven autoscaling;
- multi-region artifact replication and tenant encryption keys;
- audit retention, legal hold, and disaster-recovery procedures;
- capacity, backlog, failure, budget, and SLO dashboards;
- Palpo/Matrix and Robrix integration against enterprise load profiles.

Exit gate:

- enterprise pre-production target from the factory roadmap passes;
- losing a worker, control-plane instance, or zone loses no accepted state;
- zone recovery meets declared RPO/RTO and never violates lease fencing;
- operators can explain every task and every policy denial.

The capacity report must pin the factory load-model version and cover tenant,
project, room, Matrix-event, queue, artifact/log, certification-throughput,
failure-injection, test-window, and noisy-neighbor dimensions.

## 7. Deployment Models

### Standalone

- explicit `LocalProjectAuthority`;
- embedded `AgentdControlPlane`;
- one local worker;
- SQLite or equivalent local durable store;
- native runtime after AD-E5;
- Robrix through Matrix or local HTTP;
- OpenFab optional.

Standalone mode does not silently activate Specify and does not use enterprise
fallback semantics.

### Team Server

- Specify Project Authority or explicitly managed local authority;
- one durable execution control plane;
- multiple authenticated workers;
- shared Palpo/Matrix;
- central artifact storage;
- Docker Compose or supervised services before Kubernetes;
- OpenFab optional or project-policy required.

### 10,000-Developer Enterprise

- Specify Project Authority for project/spec/policy lifecycle;
- highly available execution control plane;
- worker fleet grouped by team, trust domain, network zone, and resource class;
- pull-based leases;
- central artifact, audit, quota, policy, and observability services;
- Palpo/Matrix collaboration cluster and Robrix cockpit;
- independent OpenFab certification and Skill Hub;
- transcripts outside Matrix;
- Kubernetes/multi-region scale only after AD-E0 through AD-E6 contracts pass.

## 8. Migration and Recovery Rules

### 8.1 Source-of-Truth Switching

- one project has exactly one active Project Authority;
- switching authority requires versioned export/import, ID mapping, validation,
  and explicit rebind;
- transient Specify failure never changes the authority owner;
- agent-chat-to-agentd cutover is per project, not one global flag;
- shadow readers cannot create external side effects.

### 8.2 In-Flight Work

- a cutover inventory records active runs, tasks, leases, workers, messages,
  Matrix cursors, worktrees, and artifact spools;
- each in-flight item is drained, imported with an immutable mapping, or
  cancelled with an audit reason;
- imported leases always receive a new agentd lease/fencing epoch;
- old runtimes cannot submit after ownership transfers.

### 8.3 Rollback

Rollback triggers include:

- duplicate accepted execution;
- stale-lease mutation accepted;
- authority mismatch or lost project binding;
- cursor advancement without durable execution acknowledgement;
- tenant isolation failure;
- artifact/evidence loss;
- recovery beyond the declared RTO.

Rollback is forward-only after command/run ownership transfers. It stops new
agentd ingress, drains or explicitly cancels agentd-owned work under its current
fencing epochs, and can route only not-yet-accepted commands back to the legacy
path through the gateway-owned deduplication ledger, which the legacy route
consults read-only before accepting a command; the ledger itself never transfers
to agent-chat. Rollback never rewinds a cursor, transfers an active lease back to
agent-chat, reuses an expired lease, or erases append-only audit and
certification references.

## 9. Design Principles

- Runtime state is not UI state.
- Project authority is not execution control.
- Policy authorship is not policy enforcement.
- Execution evidence is not certification.
- Matrix transport is not a project or transcript database.
- stdout is not a workflow outcome.
- Worker-local state is disposable; acknowledged execution history is not.
- Every long-running action has an owner, attempt, lease, timeout, fencing token,
  and recovery path.
- Local and enterprise modes share identifiers and logical ports.
- Specs remain narrow and testable; this roadmap does not authorize a wholesale
  rewrite.

## 10. Historical Native-Runtime Mapping

The previous N-series was a useful inventory but is not a second active roadmap.
Its concepts map as follows:

| Historical concept | Active destination |
| --- | --- |
| Reframe away from tmux parity | AD-E0 |
| Native runtime contract and session store | AD-E5 |
| Native PTY/process host | AD-E5 |
| Native session resume and recovery | AD-E5 |
| Runtime event/snapshot/wait APIs | AD-E5 |
| Durable capacity, lease, and backpressure | AD-E2 |
| Matrix/Robrix production gateway | AD-E3 |
| Enterprise worker fleet | AD-E2 and AD-E7 |
| OpenFab provenance/trust integration | AD-E4 |
| Cutover and removal | AD-E6 |

No historical phase name or task number is executable after this revision.

## 11. Factory Phase Mapping and Evidence

| Factory phase | agentd phase | Gate relationship |
| --- | --- | --- |
| FSF-0 | Transitional agent-chat baseline, including P272-P275 parity candidates | FSF-0 acceptance blocks AD-E0 integration; P272-P275 remain paused transitional work, not AD-E implementation |
| FSF-1 | AD-E0 | Ownership, PRD/ADR, and candidate-lineage gate |
| FSF-2 | AD-E1 | Security and tenant-isolation gate |
| FSF-3 | AD-E2 | Scheduler/worker/fencing gate |
| FSF-4 | AD-E3 | Matrix/Robrix command-routing gate |
| FSF-5 | AD-E4 plus OpenFab work | Evidence and certification-state protocol gate |
| FSF-6 | AD-E5 then AD-E6 | Native runtime, pilot sign-off, then actual legacy removal |
| FSF-7 | AD-E7 | Enterprise scale after production legacy removal |

Every phase exit produces a versioned acceptance record with repository
revisions, test/load/failure-injection commands, result and artifact digests,
exceptions, accountable owner, and required human sign-off. A prose status or
dashboard screenshot is not completion evidence.

## 12. Immediate Next Step

1. Review the committed P263-P271 feature-branch candidate baseline. Do not
   integrate it until FSF-0 and the OpenFab PRD/ADR decomposition gates pass.
2. Keep P272-P275 paused and classify them as FSF-0 transitional parity work;
   they are not the next AD-E implementation sequence.
3. Review and verify the isolated AD-E1 minimum baseline candidate without
   treating it as an AD-E1 or FSF-2 exit or integrating it ahead of AD-E0.
4. Complete the remaining AD-E1 work and its versioned acceptance record. Only
   after the AD-E1 gate may the historical P278 worker-fleet scope be re-specified
   under AD-E2, followed by the P276/P277 native-runtime scope under AD-E5;
   those old numbers remain traceability labels until new agent-spec contracts
   are created.

The first new design priority is execution sandbox and service/tenant identity,
not another compatibility endpoint and not Specify-web UI.

## 13. References

- `docs/specs/2026-05-29-agentd-specify-boundary.md`.
- `docs/specs/2026-07-10-enterprise-execution-ownership-boundary.md`.
- `docs/specs/2026-07-10-enterprise-runtime-worker-identity-contract.md`.
- `docs/specs/2026-07-10-enterprise-project-room-repo-reference-contract.md`.
- `docs/parity/agent-chat-capability-map.md`.
- OpenFab `docs/ROADMAP-enterprise-software-factory.md` and
  `docs/OpenFab_MVP_Design_and_PRD.md`.
- mempal `drawer_openfab_review_81b7262efa4c`: Specify is required as the
  enterprise Project Authority; Specify-web details are deferred.
