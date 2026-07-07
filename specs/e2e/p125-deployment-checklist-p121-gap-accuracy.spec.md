spec: task
name: "Deployment checklist reflects P121 assign_task ownership"
tags: [e2e, p0.9, docs, deployment, assign-task]
---

## Intent

Keep the real-environment deployment checklist aligned with the completed P121
ownership work. P121 resolved the old `agent_id` assignment gap by persisting
task-run ownership; P136 later resolves the `spec_path` / `plan_path` /
`context_pack` task-assignment metadata bridge by reading checkpoint context
strings when present.

## Decisions

- Update only the P0.9 deployment checklist wording and a static test that
  protects the corrected known-gap status.
- Treat `agent_id` as resolved by P121 and do not list it among remaining
  `TaskAssignment` gaps.
- Keep the line compatible with P136: `spec_path`, `plan_path`, and
  `context_pack` are runtime metadata values supplied from checkpoint context,
  not remaining `TaskAssignment` schema gaps.

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

Scenario: deployment checklist marks runtime metadata bridge resolved
  Test:
    Package: agentd-bin
    Filter: deployment_checklist_marks_p136_task_assignment_metadata_resolved
  Level: static docs regression test
  Given docs/p0.9-deployment-checklist.md
  When the TaskAssignment known-gap line is inspected
  Then it names P136 as the runtime metadata bridge
  And it does not list `spec_path` or `plan_path` as remaining gaps

## Out of Scope

- Changing the P121 implementation.
- Persisting `spec_path`, `plan_path`, or `context_pack` as schema columns.
- Executing the real Claude smoke, full `execute.dot`, SIGKILL drill, or 90
  second demo.
