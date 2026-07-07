spec: task
name: "Migration back-compat harness — proof a new migration preserves deployed rows (P2 Foundation B)"
tags: [store, migration, p2, back-compat]
---

## Intent

Provide the test net the 84 fresh-state core tests can't: proof that a NEW
migration preserves a DEPLOYED database's existing rows. Fresh-state tests run on
an empty DB migrated to the latest schema, so they stay green even if a migration
drops or mangles data a running daemon already wrote.

> REDIRECT NOTE (design-faithful C1): the `0002 runs.worktree_path` migration was
> REVERTED. The design's worktree model is PER-TASK_RUN (each agent gets its own
> worktree), which lives on the EXISTING `task_runs.worktree_path` (nullable in
> `0001`) — so the per-run `runs.worktree_path` column is not needed. The HARNESS
> below stands unchanged (model-agnostic, reusable). P108 now uses it for C2's
> `review_runs` round migration, while the self-test remains to prove the
> harness is not vacuous.

The harness applies the REAL migration `.sql` files from disk via raw SQL, seeds
rows, then applies a migration and asserts the rows survive — the test net the
84 fresh-state core tests can't provide (they run on an empty DB, so they stay
green even if a migration drops or mangles data a running daemon already wrote).

## Decisions

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

- Do not modify `agentd-core/**` (not needed for the harness).

## Out of Scope

- Additional migration semantics beyond row preservation for C2's
  `review_runs` round migration. P108 covers that real migration; this spec's
  self-test remains the reusable harness proof.
- Testing sqlx's own apply mechanics (the `_sqlx_migrations` ledger, checksum
  drift, transactional rollback) — upstream's responsibility.

## Completion Criteria

Scenario: the harness observes row loss (it is not vacuous)
  Test: backcompat_harness_detects_row_loss
  Given the harness seeds a run then applies a DESTRUCTIVE statement that deletes it
  When the seeded row is looked up after
  Then it is absent — proving the harness's preservation check actually observes data loss
