# P267 Agent Worker Store Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add typed, additive SQLite storage and repositories for enterprise agent profiles, workers/incarnations, and runtime sessions/attempts on the canonical base migration chain.

**Architecture:** Extend `agentd-core` with P265 ids/status enums. Add migration 0013 after the Matrix bridge migration without modifying base tables, then implement three focused repositories whose transactions enforce current-incarnation/current-attempt and terminal-state rules.

**Tech Stack:** Rust, SQLx runtime queries, SQLite migrations/check constraints/partial unique indexes, Tokio integration tests, agent-spec lifecycle.

## Global Constraints

- Preserve every existing P200-P266 table, row, API, and command behavior.
- Add no dependencies and no HTTP/CLI/MCP/Matrix/worker-protocol surface.
- Keep project authority data foreign and opaque; create no project authority table.
- Tests use temporary or in-memory SQLite and start no runtime or external service.

### Task 1: Contract and RED Tests

- [x] Validate the P267 spec at quality 1.0 with ten scenarios.
- [x] Add core, migration/backcompat, repository, and artifact tests.
- [x] Run focused selectors RED before production types, schema, or repositories exist.

### Task 2: Typed Identity and Status Contracts

- [x] Add five canonical id newtypes and four closed lifecycle enums.
- [x] Export types without changing existing ids and pass the core selector.

### Task 3: Additive Migration 0013

- [x] Add six constrained tables and partial unique indexes.
- [x] Update latest-version assertions from 12 to 13.
- [x] Prove fresh migration and real 0012-to-0013 backcompat.

### Task 4: Store Repositories

- [x] Implement profile create/get/status and explicit legacy alias mapping.
- [x] Implement worker enrollment, incarnation supersession, heartbeat, and retirement.
- [x] Implement runtime session, attempt, gone/resume, and terminal-state operations.
- [x] Pass all six repository scenarios.

### Task 5: Roadmap, Parity, and Verification

- [x] Record P267 schema evidence without claiming API/protocol/runtime parity.
- [x] Advance active work to P268 migration 0014.
- [x] Run workspace tests, Clippy, formatting, and scoped diff checks.
- [x] Run lifecycle with all allowed paths, then explain and stamp.
