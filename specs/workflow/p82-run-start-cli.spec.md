spec: task
name: "agentctl run start — the standalone run trigger (dry-run + deferred live path)"
tags: [workflow, cli, mvp, p0, path-b]
---

## Intent

Add `agentctl run start`, the standalone trigger for the Path-B workflows. It
selects a workflow by `--flow {draft|execute}`, resolves it under a workflows
directory, and — with `--dry-run` — parses + validates the graph and prints the
resolved plan. This spec covers the dry-run / validation CLI shell; the live
(non-dry-run) path that posts to the daemon is specified in P0.9's
`p95-live-run-start.spec.md`.

## Decisions

- `agentctl run start --flow {draft|execute} <id> [--context-file <f>] [--workflows-dir <dir>] [--dry-run]`. `--flow` is a value-enum (`draft`/`execute`); `<id>` is the issue id (draft) or frozen-spec id (execute); `--workflows-dir` defaults to `workflows`.
- The workflow file is `<workflows-dir>/<flow>.dot` (`draft.dot` / `execute.dot`), parsed with `dot::parser` + validated with `NodeGraph::from_ast` (the same path as `flow validate`).
- `--dry-run`: print the resolved flow/id/path and the plan (graph name, node count, edge count, and each node with its handler/shape) to stdout; exit 0.
- A missing/unreadable or invalid workflow file exits non-zero with the reason on stderr; an unknown `--flow` is a clap usage error (non-zero).
- The live (non-`--dry-run`) path posts to the daemon — see `p95-live-run-start.spec.md`.

## Boundaries

### Allowed Changes

- crates/agentctl/src/**
- crates/agentctl/tests/**
- specs/workflow/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not launch a live run / spawn an agent / open a socket (P0.9).

## Out of Scope

- Live execution of the workflow (the production RunHost / daemon / cross-process unpark) — P0.9.
- The `--context-file` content schema beyond being an optional path accepted by the parser.

## Completion Criteria

Scenario: dry-run of the draft flow validates and prints the plan
  Test: run_start_dry_run_draft_validates_and_prints_plan
  Given the built agentctl binary and the repo workflows directory
  When `run start --flow draft --dry-run <id>` is invoked
  Then it exits 0 and stdout names the draft workflow and its propose_spec node

Scenario: dry-run of the execute flow validates and prints the plan
  Test: run_start_dry_run_execute_validates_and_prints_plan
  Given the built agentctl binary and the repo workflows directory
  When `run start --flow execute --dry-run <id>` is invoked
  Then it exits 0 and stdout names the execute workflow and its open_pr node

Scenario: an unknown flow value is a usage error
  Test: run_start_unknown_flow_is_error
  When `run start --flow bogus <id>` is invoked
  Then it exits non-zero

Scenario: a missing workflow file is a non-zero error
  Test: run_start_missing_workflow_file_is_error
  Given a workflows directory that does not contain the flow file
  When `run start --flow draft --dry-run <id>` is invoked against it
  Then it exits non-zero and stderr reports the unreadable file
