spec: task
name: "Real Claude smoke handles workspace trust and current ready prompt"
tags: [e2e, p0.9, real-agent, claude, tmux, smoke]
---

## Intent

The first P124 real Claude smoke reached the daemon and spawned Claude Code, but
`agentctl run start` failed before MCP tool use because Claude stopped at its
workspace trust confirmation in the freshly allocated worktree. After manual
trust confirmation, Claude Code v2.1.201 also showed `auto mode on` rather than
the older `? for shortcuts` ready hint, so the backend must handle both startup
surfaces before the smoke can reach `submit_outcome`.

## Decisions

- Add an explicit opt-in environment switch
  `AGENTD_CLAUDE_AUTO_TRUST_WORKSPACE=1`; only this switch lets the tmux backend
  confirm Claude Code's workspace trust prompt automatically.
- Keep normal Claude launches conservative: without that opt-in, the backend
  must not auto-confirm workspace trust.
- Treat `auto mode on` as a Claude Code ready pattern in addition to the legacy
  `? for shortcuts` hint.
- The real smoke harness sets the opt-in only for its guarded `--execute` path,
  which already requires `AGENTD_REAL_CLAUDE_SMOKE=1`.

## Boundaries

### Allowed Changes
- specs/e2e/p126-claude-workspace-trust-ready-smoke.spec.md
- crates/agentd-tmux/src/config.rs
- crates/agentd-tmux/src/backend.rs
- crates/agentd-tmux/tests/inject.rs
- crates/agentd-bin/tests/real_claude_smoke.rs
- scripts/agentd_real_claude_smoke.sh

### Forbidden
- Do not change MCP tool schemas or stdio JSON-RPC payload shapes.
- Do not remove the existing `? for shortcuts` ready pattern.
- Do not make ordinary Claude launches auto-confirm workspace trust without
  `AGENTD_CLAUDE_AUTO_TRUST_WORKSPACE=1`.
- Do not broaden the smoke harness to run full `execute.dot`, PR creation, or
  SIGKILL recovery.

## Completion Criteria

Scenario: Claude v2.1 ready prompt is recognized
  Test:
    Package: agentd-tmux
    Filter: wait_for_ready_accepts_claude_auto_mode_prompt
  Level: tmux backend unit with fake runner
  Given a captured Claude pane containing `auto mode on`
  When `wait_for_ready` checks the Claude Code pane
  Then it returns ready without timing out

Scenario: opted-in spawn confirms Claude workspace trust before prompt injection
  Test:
    Package: agentd-tmux
    Filter: spawn_auto_trusts_claude_workspace_when_env_opted_in
  Level: tmux backend unit with fake runner
  Given a Claude Code spawn request with `AGENTD_CLAUDE_AUTO_TRUST_WORKSPACE=1`
  And the pane first shows the workspace trust confirmation
  When the backend waits for readiness before injecting the initial prompt
  Then it sends a bare Enter to confirm trust before the paste-buffer prompt

Scenario: smoke execute mode opts into Claude workspace trust
  Test:
    Package: agentd-bin
    Filter: real_claude_smoke_execute_exports_claude_auto_trust
  Level: static script regression test
  Given scripts/agentd_real_claude_smoke.sh
  When the execute-mode daemon launch is inspected
  Then it exports `AGENTD_CLAUDE_AUTO_TRUST_WORKSPACE=1` for the daemon process

## Out of Scope

- Using non-interactive `claude -p` mode.
- Changing the real Claude prompt content or MCP tool instructions.
- Solving future Claude UI copy changes beyond the observed trust prompt and
  ready hint.
