-- 0002: per-run worktree (P2 C1/Foundation B). The engine's per-run worktree
-- (verified safe under per-run delivery serialization, P2 Foundation A) is a
-- RUN-level resource allocated at run start, before any task_run exists — so it
-- lives on `runs`, not `task_runs.worktree_path` (which is per-task).
--
-- Additive + nullable: a deployed run created before this migration reads NULL,
-- so it is back-compat-safe (proven by tests/migration_backcompat.rs). Nothing
-- reads or writes this column yet — C1b allocates/persists/reads/releases it.
ALTER TABLE runs ADD COLUMN worktree_path TEXT;
