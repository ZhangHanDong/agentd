# AD-E7 Enterprise Scale Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the storage-neutral enterprise scale contracts, durable reference control plane, authenticated Specify transport, Kubernetes fleet profile, operator surfaces, and final acceptance artifacts required by AD-E7.

**Architecture:** Extend the existing core-port/SQLite-adapter/surface composition. AD-E2 remains the scheduler and fencing authority; AD-E7 adds HA leadership, rollout and zone policy, replication/key/compliance state, DR, and bounded observability without creating a second execution model.

**Tech Stack:** Rust 2024, async traits, SQLite/sqlx reference adapter, reqwest/rustls, Axum, clap, Kubernetes YAML, Sigstore policy-controller resources.

## Global Constraints

- Do not run behavior, compile, acceptance, browser, or real-agent tests until every AD-E7 task is implemented.
- Final real-agent verification uses Codex only and must not invoke Claude.
- Never persist credentials, raw prompts/transcripts, secret bytes, or tenant encryption-key material.
- Specify and OpenFab ownership boundaries remain unchanged and fail closed.
- AD-E2 leases and fencing remain the only execution mutation authority.

---

### Task 1: Enterprise Scale Contracts And Ledger

**Files:**
- Create: `crates/agentd-core/src/ports/enterprise_scale.rs`
- Modify: `crates/agentd-core/src/ports/mod.rs`
- Modify: `crates/agentd-core/src/types/ids.rs`
- Modify: `crates/agentd-core/src/types/mod.rs`
- Create: `crates/agentd-store/migrations/0024_enterprise_scale.sql`
- Create: `crates/agentd-store/src/enterprise_scale.rs`
- Modify: `crates/agentd-store/src/lib.rs`
- Create: `crates/agentd-store/tests/enterprise_scale.rs`

**Interfaces:**
- Produces `EnterpriseScalePort`, `SqliteEnterpriseScaleControlPlane`, leadership, rollout, zone-pool, replication, tenant-key, compliance, DR, load-model, and snapshot contracts.

- [x] Define bounded typed resources, transitions, denials, and idempotency rules.
- [x] Add additive schema `0024` with immutable history and monotonic leadership fencing.
- [x] Implement transactionally fenced mutations and bounded read models.
- [x] Author focused tests for conflicting replay, stale leadership, rollout health, scaling bounds, legal hold, replicas, and DR.
- [x] Commit without executing tests.

### Task 2: Authenticated Specify HTTPS And HA Composition

**Files:**
- Create: `crates/agentd-project-authority/src/http.rs`
- Modify: `crates/agentd-project-authority/src/lib.rs`
- Modify: `crates/agentd-project-authority/Cargo.toml`
- Create: `crates/agentd-project-authority/tests/http_specify_authority.rs`
- Modify: `crates/agentd-bin/src/cli.rs`
- Modify: `crates/agentd-bin/src/daemon.rs`
- Create: `crates/agentd-bin/src/enterprise.rs`

**Interfaces:**
- Produces `HttpSpecifyAuthorityTransport`, explicit enterprise configuration validation, instance heartbeat/leadership lifecycle, and fail-closed startup.

- [x] Implement HTTPS-only bounded authenticated resolve/refresh/health transport.
- [x] Add explicit Specify URL/authority/workload-token and control-plane instance configuration.
- [x] Compose enterprise state without local-authority fallback and run leadership heartbeat/renewal.
- [x] Author transport and startup contract tests for final execution.
- [x] Commit without executing tests.

### Task 3: Kubernetes Fleet, Rollout, Autoscaling, And Regions

**Files:**
- Create: `deploy/enterprise/kustomization.yaml`
- Create: `deploy/enterprise/control-plane.yaml`
- Create: `deploy/enterprise/worker-pools.yaml`
- Create: `deploy/enterprise/network-policy.yaml`
- Create: `deploy/enterprise/autoscaling.yaml`
- Create: `deploy/enterprise/signed-images.yaml`
- Create: `deploy/enterprise/regions.yaml`
- Create: `config/enterprise/factory-load-model-v1.json`
- Create: `config/enterprise/retention-policy-v1.json`

**Interfaces:**
- Consumes the fleet handoff and enterprise scale contracts.
- Produces digest-only signed-image deployment, per-zone pull pools, HPA, workload identity, and region/retention inputs.

- [ ] Add three-replica control-plane and disruption/health policy.
- [ ] Add zone-labelled outbound-only workers and deny-by-default network policy.
- [ ] Add signed image admission and audited rollout annotations.
- [ ] Add queue-driven HPA and multi-region/tenant-key configuration contracts.
- [ ] Add a versioned load model covering every roadmap dimension.
- [ ] Commit without executing validation.

### Task 4: Enterprise HTTP, CLI, Dashboard, And Doctor

**Files:**
- Modify: `crates/agentd-surface/src/host.rs`
- Modify: `crates/agentd-surface/src/http.rs`
- Modify: `crates/agentd-surface/src/dashboard.html`
- Modify: `crates/agentd-bin/src/host.rs`
- Modify: `crates/agentd-bin/src/daemon.rs`
- Modify: `crates/agentctl/src/cli.rs`
- Modify: `crates/agentctl/src/main.rs`
- Create: `crates/agentctl/src/enterprise.rs`
- Modify: `crates/agentd-store/src/operator.rs`
- Create: `crates/agentd-surface/tests/enterprise.rs`
- Create: `crates/agentctl/tests/enterprise_cli.rs`

**Interfaces:**
- Produces `/api/enterprise/status`, `/api/enterprise/tasks/:id/explain`, `agentctl enterprise status|explain|...`, dashboard scale view, and AD-E7 doctor checks.

- [ ] Add bounded operator reads and authenticated mutations.
- [ ] Expose leadership, zones, backlog, rollout, replica, budget, failure, and SLO state.
- [ ] Add exact task/policy denial explanation using existing scheduler records.
- [ ] Add CLI mutation/import commands for rollout, zone policy, replicas, keys, holds, DR, and load model.
- [ ] Author route/CLI tests for final execution.
- [ ] Commit without executing tests.

### Task 5: Operations, Roadmap, And Final Evidence Contract

**Files:**
- Create: `docs/operations/enterprise-scale-runbook.md`
- Create: `docs/operations/disaster-recovery-runbook.md`
- Create: `scripts/agentd_enterprise_load_profile.sh`
- Modify: `docs/plans/2026-07-09-agentd-native-runtime-roadmap.md`
- Modify: `docs/parity/agent-chat-capability-map.md`
- Modify: `docs/acceptance/ad-e-roadmap-manual-checklist.md`

**Interfaces:**
- Produces the AD-E7 code-complete candidate and the single final verification sequence.

- [ ] Document instance/worker/zone loss, Specify outage, rollout, scaling, replication, key rotation, legal hold, and RPO/RTO drills.
- [ ] Add a guarded load-profile harness that records immutable result digests.
- [ ] Mark code candidate status without changing any gate to accepted.
- [ ] Record every final command and Codex-only real smoke in the manual checklist.
- [ ] Commit, then begin the unified verification pass.
