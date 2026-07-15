spec: task
name: "Launcher artifacts use git exclude"
tags: [agent-chat-replacement, real-execute, tmux, worktree, p209]
---

## Intent

The p204 r5 real Codex execute smoke reached implementation and review, but all
reviewers blocked because the tmux launcher amended the task worktree's tracked
`.gitignore`. agentd-generated launcher and MCP config artifacts must be hidden
from git status without becoming part of the agent's submitted patch.

## Decisions

- The tmux backend writes launcher/MCP ignore patterns to git's local
  `info/exclude`, not the worktree's tracked `.gitignore`.
- Linked worktrees are supported by resolving the `.git` file's gitdir and
  using the common repository exclude file when the gitdir is under
  `.git/worktrees/<name>`.
- The exclude update remains idempotent: repeated spawns add one launcher
  pattern and, when needed, one MCP config pattern.
- Existing launcher script and Codex/Claude command behavior is unchanged.

## Boundaries

### Allowed Changes

- specs/e2e/p209-launcher-artifacts-use-git-exclude.spec.md
- specs/tmux/p2-spawn-flow.spec.md
- specs/e2e/p123-real-agent-stdio-smoke.spec.md
- crates/agentd-tmux/src/backend.rs
- crates/agentd-tmux/tests/spawn.rs

### Forbidden

- Do not start real Claude, Codex, tmux, Matrix, or remote relay processes in
  tests.
- Do not modify tracked `.gitignore` to hide agentd launcher artifacts.
- Do not change launcher command flags or MCP config contents.
- Do not change `scripts/agentd_real_execute_smoke.sh`.

## Out of Scope

- Cleaning up historical smoke evidence directories.
- Changing worktree allocation or release behavior.
- Changing reviewer verdict aggregation.

## Completion Criteria

Scenario: launcher artifact ignore uses git exclude
  Test:
    Package: agentd-tmux
    Filter: spawn_writes_launcher_and_amends_git_exclude
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner and tempfile worktree
  Given a spawn request in a worktree
  When the tmux backend writes the launcher
  Then `.git/info/exclude` contains `.agentd-launcher-*.sh`
  And the worktree `.gitignore` is not created or modified

Scenario: repeated spawns keep git exclude idempotent
  Test:
    Package: agentd-tmux
    Filter: spawn_twice_amends_git_exclude_once
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner and tempfile worktree
  Given two spawn attempts in the same worktree
  When the tmux backend writes launchers
  Then the local git exclude contains exactly one `.agentd-launcher-*.sh` line

Scenario: MCP config artifacts are also excluded locally
  Test:
    Package: agentd-tmux
    Filter: spawn_git_exclude_excludes_launcher_and_mcp_config_artifacts
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner and tempfile worktree
  Given repeated spawns with `AGENTD_MCP_STDIO_CMD`
  When the tmux backend writes launcher and MCP config artifacts
  Then the local git exclude contains exactly one `.agentd-launcher-*.sh` line
  And it contains exactly one `.agentd-mcp-*.json` line
  And the worktree `.gitignore` is not created or modified

Scenario: linked worktree gitdir pointer uses common exclude
  Test:
    Package: agentd-tmux
    Filter: spawn_linked_worktree_gitdir_pointer_uses_common_git_exclude
  Level: tmux launcher artifact
  Test Double: RecordingCommandRunner and tempfile linked-worktree layout
  Given a linked worktree whose `.git` file points at `.git/worktrees/<name>`
  When the tmux backend writes the launcher
  Then the common repository `info/exclude` contains `.agentd-launcher-*.sh`
  And the linked worktree `.gitignore` is not created or modified
