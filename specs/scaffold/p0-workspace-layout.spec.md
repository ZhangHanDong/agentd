spec: task
name: "P0 workspace layout"
tags: [scaffold, mvp, p0]
---

## Intent

Establish a Cargo workspace with the current twelve crates described in design §1.2.
All crates must compile cleanly. The workspace lint table forbids unsafe and
warns at clippy::all + clippy::pedantic; production crates opt into
clippy::unwrap_used / clippy::panic per-crate so test code stays clean. Major
external dependencies (tokio, sqlx, axum, matrix-sdk, octocrab, rmcp) are
pinned at the workspace root and inherited.

## Decisions

- Ten library crates: agentd-core, agentd-worktree, agentd-store, agentd-mempal, agentd-github, agentd-matrix, agentd-project-authority, agentd-security, agentd-runtime, agentd-surface
- Two binary crates: agentd-bin (the `agentd` binary) and agentctl
- Rust edition 2024, MSRV 1.85, resolver 3
- Workspace `[lints]` table forbids unsafe_code and does NOT include unwrap_used (that is opted in per-crate)
- Major external deps pinned at the workspace root via `workspace.dependencies`

## Boundaries

### Allowed Changes

- Cargo.toml
- crates/**/Cargo.toml
- crates/**/src/lib.rs
- crates/**/src/main.rs
- rust-toolchain.toml
- .cargo/config.toml

### Forbidden

- Do not add functional dependencies to P0.0 placeholder crates
- Do not relax the workspace lint table
- Do not put unwrap_used at the workspace lint level

## Completion Criteria

Scenario: Workspace builds cleanly
  Test: scaffold_workspace_builds
  Given the agentd repository at HEAD
  When cargo build for the whole workspace runs
  Then the build succeeds with no errors

Scenario: Workspace lints are inherited by every crate
  Test: scaffold_workspace_lints_inherited
  Given any crate manifest under crates/
  When the manifest is parsed
  Then it contains a lints table that inherits from the workspace

Scenario: Major dependencies are pinned at the workspace root
  Test: scaffold_workspace_deps_pinned
  Given the root Cargo.toml
  When the workspace dependencies table is read
  Then it pins tokio, sqlx, axum, matrix-sdk, octocrab, and rmcp
