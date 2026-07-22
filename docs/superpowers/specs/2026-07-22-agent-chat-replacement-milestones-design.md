# Agent-chat Replacement — Milestone Re-plan

> **Date:** 2026-07-22
> **Status:** Design (approved shape; pending spec review)
> **Supersedes (as the working view):** the phase-layered reading of
> `docs/plans/2026-07-09-agentd-native-runtime-roadmap.md`. That roadmap remains
> the authoritative contract/boundary reference; this document re-organizes its
> *remaining* work into an outcome-ordered spine.

## 1. Why re-plan

The single active roadmap (`2026-07-09-agentd-native-runtime-roadmap.md`)
defines the work as eight enterprise phases, AD-E0 through AD-E7, organized by
engineering layer (security, scheduler, Matrix gateway, native runtime, OpenFab,
enterprise scale). In practice this is hard to operate against:

- The phases have cross-dependencies (E6 depends on E3+E4+E5; E7 on E2+E4+E6),
  so there is no obvious "what is next".
- Status is scattered across `P264`–`P272` specs, several `ad-e*` worktrees, and
  the main branch, with no single "done / in-progress / not-started" view.
- One document carries very different concerns at once, so it cannot be held in
  mind as a whole.
- The parity map lags reality: it marks `native_runtime_process` and
  `native_runtime_session_restore` as `missing`, but the control-plane port,
  authenticated HTTP adapter, and cross-process recovery for exactly those rows
  landed on main in July 2026.

This re-plan keeps every durable contract the roadmap established, but presents
the remaining work as a **single linear spine of deliverable milestones**. At
any moment the reader looks at one milestone, sees "you can now do X", and
verifies it against named parity rows.

## 2. Scope decision

The target is: **agentd fully replaces agent-chat** — runtime, scheduling,
Matrix/Robrix gateway, cutover, and rollback. It is **not** an enterprise
multi-region program.

This decision lets us park the heaviest, least-relevant work:

- **Parked — enterprise security (AD-E1 heavy):** OIDC/enterprise-principal
  mapping, workload-identity mTLS for every hop, per-tenant isolation and
  multi-tenant negative tests. Replacement uses the **existing operator bearer
  token + agent token** boundary, which is already in place. The required parity
  row `auth_rbac_quota` is right-sized to this token boundary for replacement;
  full multi-tenant RBAC and quota enforcement stay in the parked enterprise
  track (§6), not a replacement milestone.
- **Parked — AD-E7 entirely:** HA control plane, Kubernetes worker profile,
  per-zone pools, autoscaling, multi-region replication, DR/RPO/RTO, SLO
  dashboards.
- **Parked — deep AD-E4 (OpenFab):** only the minimum
  `artifact_audit_provenance` surface is kept; certification-authority
  integration is deferred.

Parked work is not deleted — it becomes a future "enterprise track" that can be
picked up after replacement is complete. The `2026-07-09` roadmap remains its
home.

## 3. Current baseline (2026-07-22)

Authoritative checklist: the **required** rows of
`docs/parity/agent-chat-capability-map.md`. Of 23 required rows: **1 covered,
3 missing, 19 partial.**

The one covered row, `real_codex_execution`, is the p204 Codex execution gate;
it is the replacement baseline and needs no further milestone.

Note the parity map is stale for recent native-runtime work; §5 states each
milestone's true starting point rather than trusting the map's row status.

Established since the `2026-07-09` roadmap and merged to main:

- Native runtime control-plane **port** (`NativeRuntimeControlPort`) with a
  SQLite adapter (daemon-side) and an authenticated HTTP adapter (worker-side).
- Daemon HTTP routes `/api/runtime/native/{session/validate, session/view,
  attempt/start, attempt/update, attempt/terminal}`.
- Session/attempt lifecycle, exit-code reconciliation, and **cross-process
  recovery**: a remote worker with no local DB resolves task identity and
  provider resume reference through the control plane and launches without
  reading local SQLite.
- Worker-fleet HTTP/mTLS transport, durable lease control plane with fencing,
  worker registry with incarnation/heartbeat/supersession, content-addressed
  artifact object store, `agentd-security` sandbox scaffolding, and matrix
  bridge outbox-cursor/relay baselines.

## 4. The spine

Six milestones, strictly linear. Each states its **Outcome** (what becomes
possible), **Building on** (what already exists), **Remaining** (the new work),
**Covers** (parity rows advanced to covered), and **Done when** (verifiable exit
gate). Auth stays at the operator/agent-token level throughout; no milestone
introduces enterprise identity.

```
M1 native execution ──▶ M2 scheduler+fleet ──▶ M3 coordination ──▶
   M4 Matrix gateway ──▶ M5 cutover+rollback ──▶ M6 remove agent-chat
```

### M1 — Native execution: remote, durable, tmux-free  ← current position

**Outcome.** A remote agentd worker with no local database and no tmux pulls a
task from the daemon, runs Codex/Claude natively, uploads its artifact, submits
an explicit outcome, and recovers after disconnect — entirely over the
authenticated HTTP control plane.

**Building on.** Session/attempt control-plane port, HTTP adapter, and
cross-process recovery (merged). Worker-fleet transport, lease control plane,
and content-addressed artifact store exist but are still reached through
daemon-local composition.

**Remaining.**
1. Lease `renew`/`release`/`cancel` over the control-plane port (today the
   native worker constructs `SqliteTaskLeaseControlPlane` directly).
2. Artifact `upload`/`acknowledge` over the control-plane port (today evidence
   ack is daemon-local).
3. `agentd worker` process entry (CLI) that runs the pull→execute→upload→report
   loop against a remote daemon.
4. Opt-in real smoke: worker pulls a lease, launches a supported agent, the
   agent calls MCP and submits an explicit outcome; daemon restart reconstructs
   or explicitly terminates the runtime state.

**Covers.** `native_runtime_process`, `native_runtime_session_restore`,
`runtime_launch_tmux` (→ native), `durable_runtime_identity`,
`worker_fleet_protocol` (worker side), `artifact_audit_provenance` (upload/ack).

**Done when.** Fake-process tests prove lifecycle and recovery; the real smoke
passes with a remote worker holding no local runtime-session DB; production
runtime control no longer depends on tmux.

### M2 — Durable scheduler and worker fleet

**Outcome.** The daemon durably queues a task graph and dispatches its nodes to
whichever workers are online; control-plane restart and worker loss never lose
task ownership; an operator can explain why any task is queued, running, or
blocked.

**Building on.** Lease control plane with fencing; worker registry with
heartbeat/supersession. Missing is their composition into one authority.

**Remaining.**
1. Canonical `execution_task_queue` + lease + scheduler `outbox`, committed in a
   single `BEGIN IMMEDIATE` transaction (select eligible work, verify the online
   incarnation, create the lease, transition the queue row, append the outbox
   event).
2. Worker fleet: capability/capacity inventory, zone, drain, offline, version
   negotiation.
3. Pull acquisition with request idempotency; retry policy and dead-letter.
4. Reaper for stale leases and worker incarnations (expire, release capacity,
   requeue eligible work, dead-letter exhausted work).
5. Operator explain API for queued/running/blocked/denied/retried tasks.

**Covers.** `pool_scheduler`, `worker_fleet_protocol` (full),
`durable_task_leases` (full), `task_graph_coordination` (dispatch side).

**Done when.** Control-plane restart loses no accepted task/lease; worker
disappearance does not corrupt ownership; duplicate acquire/release/upload is
idempotent; failure-injection covers reassignment and network partition.

### M3 — Coordination product complete

**Outcome.** Agents register with runtime profiles, exchange direct and group
messages, and drive task graphs entirely through agentd's native APIs — the
coordination features agent-chat provided are now agentd-owned.

**Building on.** `p213`/`p214`/`p234` registry lifecycle (register, list,
heartbeat, offline, down/rebind); partial messaging/inbox and task-graph
primitives.

**Remaining.**
1. Agent registry: import/update, profile management, offline-recovery hardening.
2. Messaging: direct inbox and group messaging reaching full agent-chat parity
   (read cursors, mentions, dedup).
3. Task graph: CRUD and migration, coordination semantics driven by agentd.
4. Project ↔ room ↔ repo binding as a durable agentd-owned record.

**Covers.** `agent_registry_lifecycle`, `agent_runtime_profiles`,
`messaging_inbox`, `group_messaging`, `task_graph_coordination` (CRUD/migration),
`project_room_repo_binding`.

**Done when.** A project's agents register, message, and run a task graph with no
agent-chat process in the path.

### M4 — Matrix / Robrix gateway

**Outcome.** Matrix room messages enter agentd through a gateway with a durable
cursor, deduplication, and trusted-sender enforcement; execution summaries
return to Matrix; Robrix shows project/run/task/artifact views; attachments are
ingested as content-addressed inputs.

**Building on.** Matrix bridge outbox-cursor, relay baseline, and bridge repo
(partial).

**Remaining.**
1. `AgentdMatrixGateway`-owned durable cursor and processed-event store.
2. Transactional command inbox/run/outbox handoff with a canonical `command_id`
   and a unique room/project dedup constraint.
3. Trusted inviter, ignored sender, appservice loop suppression, command
   normalization.
4. Attachment ingest as content-addressed project input.
5. Robrix project/run/task/artifact/approval/evidence views.

**Covers.** `matrix_bridge`, `remote_relay`, `attachments_media`,
`dashboard_cli_operations` (view side).

**Done when.** Robrix binds a project through Specify and dispatches through
agentd without agent-chat; restart/replay produces zero duplicate accepted
executions; raw transcripts never enter Matrix.

### M5 — Cutover and rollback

**Outcome.** Per project, migrate observe → shadow (read-only, side effects
disabled) → canary → cutover → drain → retire; any step can roll back without
losing authority, cursor, run, task, or artifact ownership.

**Building on.** M4 gateway; existing migration/relay baselines.

**Remaining.**
1. Per-project authority/cursor cutover state machine.
2. Shadow mode that produces no source/queue/message/certification side effects.
3. Canary and rollback triggers with state mapping for projects, rooms, agents,
   tasks, messages, cursors, and in-flight runs.

**Covers.** `migration_shadow_cutover`, `api_auth_boundary` (cutover boundary).

**Done when.** Shadow produces no side effects; canary rollback preserves all
ownership; a pilot project completes cutover and a rollback drill.

### M6 — Remove agent-chat / tmux and close operations

**Outcome.** Installation, health, doctor, backup, and restore are complete;
production configuration, startup entrypoints, and code paths contain no
agent-chat or tmux dependency; the parity audit has no required missing/partial
row.

**Building on.** M1–M5; existing `operational_doctor` diagnostics.

**Remaining.**
1. Local, team-server installation; operator preflight, health, doctor, backup,
   restore, rollback.
2. Remove agent-chat/tmux production configuration, documentation, and code;
   retain only explicitly scoped offline import tools.
3. Advance every remaining partial parity row to covered.

**Covers.** `operational_doctor_health`, `dashboard_cli_operations` (full), and
the closure of all remaining `partial` rows.

**Done when.** `agentctl parity audit --agent-chat` reports no required
missing/partial row without an explicit approved product-scope decision; after
human sign-off, no production path references agent-chat/tmux.

## 5. Old phase → new milestone map

For traceability against the `2026-07-09` roadmap:

| Old phase | New home |
|---|---|
| AD-E1 (security) | Right-sized to operator/agent tokens; heavy identity **parked** |
| AD-E2 (scheduler + fleet) | **M2**, plus M1 covers the worker side |
| AD-E3 (Matrix gateway + cutover) | Split into **M4** (gateway) and **M5** (cutover) |
| AD-E4 (OpenFab) | Minimum provenance folded into M1; deep integration **parked** |
| AD-E5 (native runtime) | **M1** |
| AD-E6 (final cutover + removal) | **M6** |
| AD-E7 (enterprise scale) | **Parked** (future enterprise track) |
| Coordination product (registry/messaging/task graph) | **M3** (was implicit across P2xx) |

## 6. Out of scope (parked enterprise track)

Enterprise security (OIDC, mTLS everywhere, multi-tenant isolation), Kubernetes
worker profiles, per-zone/multi-region workers, autoscaling, artifact
replication and tenant keys, DR/legal-hold, and SLO/capacity dashboards. These
remain in the `2026-07-09` roadmap as AD-E1 (heavy) and AD-E7, to be resumed
after replacement is complete.

## 7. How this document is used

Each milestone becomes its own implementation plan (one spec → plan →
implementation cycle) when it is picked up. Progress is tracked by advancing the
named parity rows to covered and by each milestone's "Done when" gate. The next
implementation cycle is **M1**, whose remaining items (lease/artifact over HTTP,
`agentd worker` CLI, real smoke) are the smallest coherent next step.
