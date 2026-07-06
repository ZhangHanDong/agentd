spec: task
name: "agentd CLI maintenance options stay compatible"
tags: [e2e, daemon, cli, p2, worktree, cleanup]
---

## Intent

P106 changed `agentd` from a daemon-only parser into a top-level parser with an
optional maintenance subcommand. That must not regress the existing no-subcommand
daemon mode, and operators must be able to pass the shared store/repo/worktree
options when they run `cleanup-worktrees`.

This slice hardens only the CLI contract around that entrypoint. It does not
change failed-run cleanup semantics, store candidate selection, or worktree pool
release rules.

## Decisions

- Omitted subcommand remains daemon mode: `AgentdCli.command` is `None` and the
  existing daemon defaults still parse.
- `cleanup-worktrees` remains dry-run unless `--execute` is supplied.
- Shared daemon/store/worktree options must parse both before and after the
  maintenance subcommand. Operators commonly type maintenance commands as
  `agentd cleanup-worktrees --db-path ... --worktree-base ... --execute`; this
  must be accepted instead of forcing all shared options before the subcommand.
- Unknown subcommands remain parser errors; the maintenance parser must not
  silently fall back to daemon mode for misspelled commands.

## Boundaries

### Allowed Changes

- crates/agentd-bin/src/cli.rs
- specs/e2e/**

### Forbidden

- Do not change failed-run cleanup candidate rules.
- Do not change worktree release ordering or store mutation ordering.
- Do not make cleanup execute by default.
- Do not add a second top-level binary or split the daemon composition root.

## Out of Scope

- Changing stdout/stderr formatting for `cleanup-worktrees`.
- Adding JSON output for maintenance commands.
- Adding new maintenance subcommands.
- Changing daemon serving behavior after parsing succeeds.

## Completion Criteria

Scenario: no subcommand keeps daemon defaults
  Test: agentd_cli_without_subcommand_uses_daemon_defaults
  Level: CLI unit
  Test Double: clap parser
  Given the argv ["agentd"]
  When the CLI parses it
  Then command is None
  And db_path is "agentd.db"
  And port is 8787
  And worktree_base is ".agentd/worktrees"

<!-- lint-ack: testability — cleanup/clean appears only in the exact command and enum names asserted by parser tests. -->
Scenario: cleanup accepts shared options before the subcommand
  Test: agentd_cli_cleanup_accepts_shared_options_before_subcommand
  Level: CLI unit
  Test Double: clap parser
  Given argv with --db-path, --repo-dir, and --worktree-base before "cleanup-worktrees"
  When the CLI parses it
  Then cmd variant equals CleanupWorktrees
  And the parsed config contains the supplied paths
  And execute is true when --execute is supplied

<!-- lint-ack: testability — cleanup/clean appears only in the exact command and enum names asserted by parser tests. -->
Scenario: cleanup accepts shared options after the subcommand
  Test: agentd_cli_cleanup_accepts_shared_options_after_subcommand
  Level: CLI unit
  Test Double: clap parser
  Given argv with "cleanup-worktrees" before --db-path, --repo-dir, and --worktree-base
  When the CLI parses it
  Then cmd variant equals CleanupWorktrees
  And the parsed config contains the supplied paths
  And execute is true when --execute is supplied

Scenario: unknown maintenance command is rejected
  Test: agentd_cli_rejects_unknown_subcommand
  Level: CLI unit
  Test Double: clap parser
  Given the argv ["agentd", "cleanup-worktree"]
  When the CLI tries to parse it
  Then parsing returns an error
