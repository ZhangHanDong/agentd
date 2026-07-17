# AD-E6 Final Cutover Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make agent-chat an offline import source, activate agentd as the sole production authority, and remove tmux from production runtime and operations.

**Architecture:** A durable SQLite cutover state machine coordinates canonical import, decision shadowing, drain, cursor handoff, activation, backup, service installation, and rollback. Production process ownership moves to the AD-E5 native runtime while git worktree allocation becomes an independent crate.

**Tech Stack:** Rust 2024, SQLite/sqlx, clap, portable-pty, SHA-256 manifests, launchd, systemd, Docker Compose.

## Global Constraints

- Do not run behavior or acceptance tests until AD-E1 through AD-E7 candidate code is complete.
- Final real-agent verification uses Codex only and must not invoke Claude.
- Agent-chat is read only and offline during import; no production dual-write remains.
- Input text, credentials, raw transcripts, and secret material never enter cutover evidence.

---

### Task 1: Durable Cutover Ledger

**Files:**
- Create: `crates/agentd-core/src/ports/cutover.rs`
- Create: `crates/agentd-store/migrations/0022_final_cutover.sql`
- Create: `crates/agentd-store/src/cutover.rs`
- Create: `crates/agentd-store/tests/cutover.rs`

**Interfaces:**
- Produces `CutoverLedgerPort`, `CutoverRun`, `CutoverStepReceipt`, `LegacyIdMapping`, `ShadowDecision`, `CursorHandoff`, `BackupManifest`, and `ServiceInstallation`.

- [ ] Define bounded IDs, states, requests, reports, and the ledger port.
- [ ] Add immutable/idempotent SQLite records and state-transition constraints.
- [ ] Implement exact-replay and conflicting-replay behavior.
- [ ] Author migration and ledger tests for final execution.
- [ ] Run only `cargo check --workspace`, then commit.

### Task 2: Unified Import, Shadow, Drain, And Handoff

**Files:**
- Modify: `crates/agentd-store/src/agent_chat_import.rs`
- Create: `crates/agentd-store/src/cutover_service.rs`
- Create: `crates/agentd-store/tests/cutover_service.rs`

**Interfaces:**
- Consumes `CutoverLedgerPort` and existing agent/message/task imports.
- Produces `CutoverService::{plan,import,shadow,drain,handoff,activate,rollback}`.

- [ ] Canonicalize and hash the complete supported source snapshot.
- [ ] Import all supported surfaces under one cutover id and persist stable mappings.
- [ ] Compare normalized routing, audience, task, graph, and cursor decisions.
- [ ] Gate drain/handoff/activation on zero drift and zero legacy in-flight work.
- [ ] Author replay, drift, and rollback tests for final execution.
- [ ] Run only `cargo check --workspace`, then commit.

### Task 3: Operator CLI, Doctor, Backup, And Restore

**Files:**
- Modify: `crates/agentctl/src/cli.rs`
- Modify: `crates/agentctl/src/main.rs`
- Create: `crates/agentctl/src/cutover.rs`
- Create: `crates/agentctl/tests/cutover_cli.rs`

**Interfaces:**
- Produces `agentctl cutover plan|import|shadow|drain|handoff|activate|inspect|rollback|doctor|backup|restore|service-render`.

- [ ] Wire every state transition through the cutover service.
- [ ] Implement bounded structured doctor checks without raw-log inspection.
- [ ] Implement SQLite online backup, digest manifest, verified atomic restore, and running-service refusal.
- [ ] Render local/team/fleet service assets with native-only startup commands.
- [ ] Author CLI/backup tests for final execution.
- [ ] Run only `cargo check --workspace`, then commit.

### Task 4: Native Production Runtime And Independent Worktrees

**Files:**
- Create: `crates/agentd-worktree/Cargo.toml`
- Create: `crates/agentd-worktree/src/lib.rs`
- Create: `crates/agentd-bin/src/native_backend.rs`
- Modify: `crates/agentd-bin/src/daemon.rs`
- Modify: `crates/agentd-bin/Cargo.toml`
- Modify: `Cargo.toml`

**Interfaces:**
- Produces `WorktreePool`, `GitWorktreeProvider`, `NativeAgentBackend`, and `NativeAgentLifecycle`.

- [ ] Extract git worktree ownership without runtime dependencies.
- [ ] Adapt workflow spawn/dispatch/lifecycle to native logical sessions and attempts.
- [ ] Compose startup recovery and idle reaping into daemon service lifetime.
- [ ] Remove `agentd-tmux` from the workspace and production dependency graph.
- [ ] Author native composition/recovery tests for final execution.
- [ ] Run only `cargo check --workspace`, then commit.

### Task 5: Services, Documentation, And Legacy Deletion

**Files:**
- Create: `deploy/local/io.agentd.plist`
- Create: `deploy/team/agentd.service`
- Create: `deploy/team/compose.yaml`
- Create: `docs/operations/final-cutover-runbook.md`
- Modify: `docs/parity/agent-chat-capability-map.md`
- Modify: `docs/plans/2026-07-09-agentd-native-runtime-roadmap.md`
- Modify: `docs/acceptance/ad-e-roadmap-manual-checklist.md`
- Delete: `crates/agentd-tmux/`
- Delete: `scripts/agentd_real_claude_smoke.sh`

**Interfaces:**
- Produces native-only production artifacts and the final AD-E6 manual evidence list.

- [ ] Add local/team service definitions and fleet handoff metadata.
- [ ] Document preflight, health, doctor, import, drain, handoff, backup, restore, rollback, and retirement.
- [ ] Remove tmux/agent-chat production startup paths and mark compatibility code offline-only.
- [ ] Record explicit product-scope decisions for every parity row still awaiting operator acceptance.
- [ ] Defer all acceptance execution to the final checklist, run `cargo check --workspace`, and commit the AD-E6 candidate.
