spec: task
name: "Tool cmd ${...} variable substitution — restore the design's mechanism (P2 C1' R2)"
tags: [core, handler, tool, p2, substitution]
---

## Intent

Restore `${...}` variable substitution in the `tool` handler — the mechanism the
design intended (design §7 `cmd="agent-spec lifecycle $spec_path --code $worktree
…"`) but the P0 MVP deliberately cut (static whitespace-split argv only). This is
the key to the design-faithful C1 redirect: a code tool receives the worktree as
an explicit `--code ${worktree}` ARGUMENT (and the spec as `${spec_path}`), so the
cwd stays where the `.agentd/run/` runtime-state convention lives — resolving the
worktree↔cwd conflict that cwd-threading (the reverted C1a) created.

R2 is the MECHANISM. The shipped workflows still use static paths; they migrate to
`${worktree}` in R3, once the per-task_run allocation makes that value real.
The old Engine-level C1a worktree threading is superseded by the task-run worktree path,
so `${worktree}` is now just another staged context variable.

## Decisions

- Substitute `${name}` tokens PER ARGV ELEMENT, AFTER the existing whitespace
  split (program + each arg) — so a substituted value containing spaces stays ONE
  argv element, and there is no shell (argv exec), no re-split hazard, no shell
  injection surface.
- The variable map for a tool node is every top-level STRING entry in
  `ctx.context` (e.g. `spec_path`, `task_run_id`, and `worktree` once staged by
  a prior node). There is no hidden `ctx.worktree()` fallback.
- An unknown `${name}` (not in the map) or an unterminated `${` is a LOUD error
  (`CoreError::Invariant` naming the node + the offending token) — never a silent
  passthrough or empty string. Substitution is a SINGLE pass: a value that itself
  contains `${...}` is NOT re-expanded.
- A `cmd` with no `${` is returned UNCHANGED — the shipped static workflows
  (`cat .agentd/run/...`, `--code .`) are unaffected; substitution is a no-op for
  them.
- Remove the now-obsolete `Do not use ${...} / $run_dir substitution` Forbidden
  rule from the draft/execute workflow specs (`p80`, `p81`) — the handler now
  implements substitution. Those workflows still use static cmds (their migration
  to `${worktree}` is R3); the rule's source-wide grep is what the relaxation lifts.

## Boundaries

### Allowed Changes

- crates/agentd-core/** (the `tool` handler + its tests)
- specs/core/**
- specs/workflow/p80-draft-dot.spec.md, specs/workflow/p81-execute-dot.spec.md
  (remove the obsolete no-substitution Forbidden rule)

### Forbidden

- Do not change any shipped workflow `cmd=` to use `${...}` (that is R3, which
  needs the real per-task_run worktree value).
- Do not reintroduce Engine-level or HandlerCtx-level worktree threading.

## Out of Scope

- Per-task_run worktree allocation + migrating draft.dot/execute.dot to
  `${worktree}` (R3).
- Shell quoting / escaping beyond argv: values are exec argv elements, not a
  shell string, so no `sh -c` quoting is needed (or supported).

## Completion Criteria

Scenario: known variables are substituted in place
  Test: substitute_replaces_known_vars
  Given a substitution over the string "x ${a} y ${b}" with a=1 and b=2
  When it is substituted
  Then the result is "x 1 y 2"

Scenario: an unknown variable is a loud error
  Test: substitute_unknown_var_is_error
  Given a substitution over "${nope}" with an empty variable map
  When it is substituted
  Then it returns an error naming the undefined variable, not a silent passthrough

Scenario: text without a substitution token is unchanged
  Test: substitute_leaves_plain_text_unchanged
  Given a substitution over "cat .agentd/run/frozen.spec.md" with any variable map
  When it is substituted
  Then the result is byte-identical to the input

Scenario: a tool node substitutes the staged worktree and a context var
  Test: tool_cmd_substitutes_worktree_and_context_var
  Given an Engine with a fake WorktreeAllocator returning W and a graph whose codergen stages worktree and task_run_id before a tool node runs cmd "verify --code ${worktree} --run ${task_run_id}"
  When the codergen outcome is delivered and the tool node runs
  Then the recorded tool call's args contain W and the staged task_run_id, with no literal "${" remaining
