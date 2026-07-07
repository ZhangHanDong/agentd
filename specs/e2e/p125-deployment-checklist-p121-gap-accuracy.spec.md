spec: task
name: "Deployment checklist reflects P121 assign_task ownership"
tags: [e2e, p0.9, docs, deployment, assign-task]
---

## Intent

Keep the real-environment deployment checklist aligned with the completed P121
ownership work before the next real Claude smoke is executed. P121 resolved the
old `agent_id` assignment gap by persisting task-run ownership, while
`spec_path` and `plan_path` remain deferred task-assignment metadata gaps.

## Decisions

- Update only the P0.9 deployment checklist wording and a static test that
  protects the corrected known-gap status.
- Treat `agent_id` as resolved by P121 and do not list it among remaining
  `TaskAssignment` gaps.
- Keep `spec_path` and `plan_path` listed as remaining `TaskAssignment` gaps.

## Boundaries

### Allowed Changes
- specs/e2e/p125-deployment-checklist-p121-gap-accuracy.spec.md
- docs/p0.9-deployment-checklist.md
- crates/agentd-bin/tests/deployment_checklist.rs

### Forbidden
- Do not change production Rust code.
- Do not change `draft.dot`, `execute.dot`, MCP tool schemas, or smoke harness
  behavior.
- Do not run or automate a paid/authenticated Claude CLI call.

## Completion Criteria

Scenario: deployment checklist marks the agent_id gap resolved
  Test:
    Package: agentd-bin
    Filter: deployment_checklist_marks_p121_agent_id_gap_resolved
  Level: static docs regression test
  Given docs/p0.9-deployment-checklist.md and the P121 spec
  When the known gaps section is inspected
  Then it mentions that P121 resolved `agent_id` ownership
  And it does not say `agent_id` is populated from spawn context instead of the
  store

Scenario: deployment checklist keeps spec and plan path gaps explicit
  Test:
    Package: agentd-bin
    Filter: deployment_checklist_keeps_spec_and_plan_path_gaps
  Level: static docs regression test
  Given docs/p0.9-deployment-checklist.md
  When the TaskAssignment known-gap line is inspected
  Then it still names `spec_path` and `plan_path` as remaining gaps

## Out of Scope

- Changing the P121 implementation.
- Persisting `spec_path`, `plan_path`, or `context_pack`.
- Executing the real Claude smoke, full `execute.dot`, SIGKILL drill, or 90
  second demo.
