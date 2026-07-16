# AD-E2 Durable Scheduler and Worker Fleet Plan

**Goal:** Implement the AD-E2 code candidate over canonical task/worker/lease identities while deferring real/manual acceptance.

**Architecture:** Add closed fleet contracts in core, additive SQLite state in store, and an authenticated control-plane service/router in agentd-bin. Reuse P267 worker and P270 fencing state; keep compatibility scheduler state separate.

## Constraints

- Specify owns snapshot, RBAC, quota, placement, and revocation policy truth.
- Agentd owns accepted queue, worker lifecycle, lease/fencing, retries, upload acknowledgements, and outbox.
- Queue/lease/outbox mutations are atomic and idempotent.
- Stale fencing never mutates result, artifact, usage, forge, delivery, secret, or tool state.
- Tests start no Claude, Matrix, Specify, OpenFab, tmux, or external worker.
- Real/manual validation is appended to the cumulative checklist and run only at the end.

### Task 1: Snapshot and Fleet Contracts

**Files:**
- Modify: `crates/agentd-core/src/types/project_authority.rs`
- Create: `crates/agentd-core/src/ports/fleet_scheduler.rs`
- Modify: `crates/agentd-core/src/ports/mod.rs`
- Modify: `crates/agentd-core/tests/project_authority.rs`
- Create: `crates/agentd-core/tests/fleet_scheduler.rs`

- [x] Pin placement policy and revocation epoch in project snapshots.
- [x] Define typed worker availability, queue states, assignments, reports, reaper, outbox, and explain contracts.
- [x] Bind every mutating report to canonical task, worker incarnation, lease, and fencing identities.

### Task 2: Durable Enterprise Scheduler Schema

**Files:**
- Create: `crates/agentd-store/migrations/0018_enterprise_fleet_scheduler.sql`
- Modify: `crates/agentd-store/tests/migration.rs`
- Create: `crates/agentd-store/tests/enterprise_fleet_scheduler.rs`

- [x] Add enterprise queue, current worker availability, scheduler outbox, and artifact-upload acknowledgement tables.
- [x] Add status/check/unique/immutable constraints and indexes for pull, retry, reap, and outbox drain.
- [x] Prove migration is additive and stores no secret, transcript, path, tmux, Matrix, or raw error content.

### Task 3: Transactional Queue, Pull, and Lease Lifecycle

**Files:**
- Create: `crates/agentd-store/src/fleet_scheduler.rs`
- Modify: `crates/agentd-store/src/task_lease_control_plane.rs`
- Modify: `crates/agentd-store/src/lib.rs`
- Modify: `crates/agentd-store/tests/enterprise_fleet_scheduler.rs`

- [x] Implement idempotent submit and capacity/quota backpressure.
- [x] Implement pull acquisition with current worker, protocol, heartbeat, capability, placement, snapshot, and revocation checks.
- [x] Allocate queue state, lease/fencing token, and outbox event in one transaction.
- [x] Implement renew, complete, cancel, retry wait, dead letter, and duplicate-safe outcomes.

### Task 4: Worker Heartbeat and Reaper

**Files:**
- Modify: `crates/agentd-store/src/fleet_scheduler.rs`
- Modify: `crates/agentd-store/tests/enterprise_fleet_scheduler.rs`

- [x] Persist monotonic heartbeat sequence and typed availability only for the current authenticated incarnation.
- [x] Support online, draining, offline, and version-negotiation denials.
- [x] Reap stale workers/leases and requeue or dead-letter tasks with new fencing on later acquisition.

### Task 5: Fenced Artifact and Side-Effect Admission

**Files:**
- Modify: `crates/agentd-store/src/fleet_scheduler.rs`
- Modify: `crates/agentd-store/src/execution_evidence_control_plane.rs`
- Modify: `crates/agentd-store/tests/enterprise_fleet_scheduler.rs`
- Modify: `crates/agentd-store/tests/enterprise_execution_artifacts.rs`

- [x] Record retryable upload attempts and idempotent acknowledgements without artifact bytes.
- [x] Require current lease/fencing and revocation checkpoint before artifact acceptance and protected side effects.
- [x] Preserve stale-report audit evidence before returning rejection.

### Task 6: Authenticated Worker Service and Explain API

**Files:**
- Create: `crates/agentd-bin/src/fleet.rs`
- Modify: `crates/agentd-bin/src/lib.rs`
- Create: `crates/agentd-bin/tests/enterprise_fleet.rs`

- [x] Compose workload mTLS, fleet scheduler, revocation, and trusted clock without standalone-token fallback.
- [x] Expose product-neutral heartbeat/pull/renew/result/artifact/explain handlers with stable structured errors.
- [x] Prove missing providers and stale workload identity fail before queue/lease mutation.

### Task 7: Candidate Evidence

**Files:**
- Create: `crates/agentctl/tests/ad_e2_completion_contract.rs`
- Modify: `docs/plans/2026-07-09-agentd-native-runtime-roadmap.md`
- Modify: `docs/parity/agent-chat-capability-map.md`
- Modify: `docs/acceptance/ad-e-roadmap-manual-checklist.md`

- [x] Bind every AD-E2 work item to code and tests.
- [x] Record code-complete candidate without AD-E2/FSF-3 PASS.
- [x] Retain restart, worker-loss, partition, duplicate, and partial-upload scenarios as final manual checklist items.
