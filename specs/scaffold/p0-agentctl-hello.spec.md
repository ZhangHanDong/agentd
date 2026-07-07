spec: task
name: "P0 agentctl hello"
tags: [agentctl, mvp, p0]
---

## Intent

Prove the end-to-end developer loop works: code compiles, the binary runs, and
its version output matches the package version. This is the smallest possible
proof that the workspace scaffold produces a runnable artifact.

## Decisions

- agentctl uses clap derive macros for argument parsing
- agentctl --version reads the version from the package metadata (0.0.0 in P0.0)
- P0.0 shipped no real subcommands; P0.1 Task 10 replaces the placeholder with the real `flow validate` subcommand (the `--version` contract is unchanged)

## Boundaries

### Allowed Changes

- crates/agentctl/**

### Forbidden

- Do not implement workflow logic in P0.0
- Do not add MCP, HTTP, or storage dependencies to agentctl in P0.0

## Completion Criteria

Scenario: The version flag prints the package version
  Test: agentctl_version_matches_cargo_metadata
  Given the built agentctl binary
  When it is invoked with the version flag
  Then standard output equals the string agentctl 0.0.0
  And the exit code is zero

Scenario: The help output lists the flow subcommand
  Test: agentctl_help_lists_flow_subcommand
  Given the built agentctl binary
  When it is invoked with the help flag
  Then standard output mentions the flow subcommand
  And the exit code is zero
