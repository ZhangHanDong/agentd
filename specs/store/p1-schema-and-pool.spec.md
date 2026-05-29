spec: task
name: "SQLite schema migration + connection pool"
tags: [store, mvp, p0]
---

## Intent

`agentd-store` opens a SQLite database at a configurable path, applies the
embedded `0001_init.sql` migration (the full §3.3 schema, reconciled to what the
P0.1 `Store` trait can supply), and exposes a `SqlitePool` for the repos. WAL
journaling and foreign-key enforcement are set at connection time. This task
lands the schema + connection skeleton; the `ports::Store` trait impl is wired
across Tasks 2–5.

## Decisions

- sqlx runtime mode — no `query!` macros, no `.sqlx` metadata, so no `DATABASE_URL` at build time. Migrations embed via `sqlx::migrate!`.
- Connection options: `create_if_missing`, WAL journal, `foreign_keys(true)`, 5s busy timeout, max 8 pool connections.
- **P0.1-trait ↔ P0.2-schema reconciliation** (documented in the migration header): columns the engine-facing trait can't supply are relaxed in three buckets — store-self-supplied stay NOT NULL (timestamps, status, attempt, JSON blobs); genuinely-deferred features become nullable (`review_runs.{task_run_id,bundle_path,visibility,aggregator}`, `task_runs.{agent_id,worktree_path,base_commit}`, `human_waits.{interviewer,options}`); daemon-has-it/engine-doesn't become nullable (`runs.{project_id,workflow_path}`). Added: `review_runs.expected` and a `checkpoints` table (1:1 with `engine::Checkpoint`). The `agents` FK on `review_verdicts.reviewer_id`/`task_runs.agent_id` is dropped (re-added in P0.3).
- `~/.agentd` is the home dir (overridable via `AGENTD_HOME`); default db is `<home>/agentd.db`.

## Boundaries

### Allowed Changes

- crates/agentd-store/**
- scripts/check.sh (include specs/store in the lifecycle loop)

### Forbidden

- Do not use sqlx compile-time query macros (`sqlx::query!`); repos call the runtime query functions instead, so the build needs no compile-time database URL or offline metadata cache.
- Do not store anything Specify owns (issues are a per-run cache, boundary Δ3; no spec table, Δ4).

## Completion Criteria

Scenario: The migration creates the expected tables
  Test: migration_creates_expected_tables
  Given a fresh database path
  When SqliteStore::connect opens and migrates it
  Then sqlite_master lists runs, node_outcomes, checkpoints, review_runs, review_verdicts, human_waits, task_runs, artifacts, projects, agents, issues, mempal_outbox, matrix_events, events, and schema_meta

Scenario: Reopening an existing database applies no new migrations
  Test: migration_is_idempotent_on_reopen
  Given a database that was already created and migrated
  When SqliteStore::connect opens it again
  Then it succeeds and schema_meta version is still 1

Scenario: Foreign keys are enforced on the connection
  Test: foreign_keys_are_enforced
  Given a migrated database
  When a node_outcome is inserted referencing a run id that does not exist
  Then the insert is rejected
