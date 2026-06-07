spec: task
name: "Migration back-compat harness + the runs.worktree_path migration (P2 Foundation B)"
tags: [store, migration, p2, back-compat]
---

## Intent

Provide the test net the 84 fresh-state core tests can't: proof that a NEW
migration preserves a DEPLOYED database's existing rows. Fresh-state tests run on
an empty DB migrated to the latest schema, so they stay green even if a migration
drops or mangles data a running daemon already wrote.

Build the harness against the FIRST real migration it must guard — `0002`, which
adds `runs.worktree_path` (verified absent from `0001`; C1's per-run worktree is
allocated at run START, before any `task_run`, so the existing `task_runs.
worktree_path` can't hold it). `0002` is purely ADDITIVE + nullable, so it is
back-compat-safe and behavior-preserving: nothing reads or writes the column yet
— C1b does. Foundation B lands the column + the harness + the proof; C1b consumes
the column.

## Decisions

- `migrations/0002_runs_worktree_path.sql` = `ALTER TABLE runs ADD COLUMN
  worktree_path TEXT;` — additive, nullable (a deployed run defaults to NULL),
  numbered after `0001`. Fresh `SqliteStore::connect` now applies `0001`+`0002`;
  the new column is unused (no code path reads/writes it), so every existing test
  stays green.
- The harness uses RAW-SQL APPLY (shape B): a test opens a raw pool (NOT the
  migrator), applies migration `.sql` FILES READ FROM DISK AT TEST TIME (the same
  files `sqlx::migrate!` embeds — never a hand-copied schema, which would test a
  fiction that rots when `0001` changes), seeds rows, then applies the migration
  under test and asserts the rows survived. `sqlx::raw_sql` runs the multi-
  statement files.
- The harness DELIBERATELY bypasses `MIGRATIONS.run()` / the `_sqlx_migrations`
  ledger: it guards the migration SQL's row-PRESERVATION (the thing that breaks
  deployed data). sqlx's apply mechanics — checksums, partial-failure rollback,
  the ledger — are upstream's job, well-tested there, and Out of Scope here.
- Protocol (recorded for future migrations): every migration that changes an
  existing table MUST add a harness test seeding pre-migration rows and asserting
  they survive the new migration.

## Boundaries

### Allowed Changes

- crates/agentd-store/migrations/**
- crates/agentd-store/tests/**
- specs/store/**

### Forbidden

- Do not make any code path read or write `runs.worktree_path` (that is C1b).
- Do not modify `agentd-core/**` (not needed for Foundation B).

## Out of Scope

- C1b — allocate / persist / read / release the per-run worktree on
  `runs.worktree_path`, plus the daemon wiring and the e2e test (consumes the
  column this lands).
- Testing sqlx's own apply mechanics (the `_sqlx_migrations` ledger, checksum
  drift, transactional rollback) — upstream's responsibility.

## Completion Criteria

Scenario: a new migration preserves a deployed DB's existing rows
  Test: migration_0002_preserves_existing_runs
  Given a database at the 0001 schema with a seeded run row (no worktree_path)
  When the 0002 migration is applied via the back-compat harness
  Then the run row is still present and its worktree_path is NULL (defaulted)

Scenario: the 0002 migration adds the worktree_path column
  Test: migration_0002_adds_worktree_path_column
  Given a fresh database migrated to the latest schema via SqliteStore::connect
  When a run is recorded and its worktree_path is selected
  Then the worktree_path column exists and is NULL

Scenario: the harness observes row loss (it is not vacuous)
  Test: backcompat_harness_detects_row_loss
  Given the harness seeds a run then applies a DESTRUCTIVE statement that deletes it
  When the seeded row is looked up after
  Then it is absent — proving the harness's preservation check actually observes data loss
