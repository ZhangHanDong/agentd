spec: task
name: "secure Matrix storage dependency baseline"
tags: [dependencies, security, matrix, sqlite, ci, p155]
---

## Intent

Restore the protected dependency-governance check without retaining the known
Matrix SDK 0.9 vulnerabilities or dropping the existing persistent Matrix SDK
SQLite store. SQLx and Matrix SDK must share one compatible native SQLite
binding in the agentd process.

## Decisions

- Raise the workspace MSRV to Rust 1.94, the minimum supported by stable SQLx
  0.9, synchronize Clippy's MSRV, and use the resulting Rust 1.94 let-chain
  lint form where required.
- Upgrade SQLx to 0.9 and Matrix SDK to 0.16.1 so both accept
  `libsqlite3-sys` 0.35 and the Matrix security fixes are present.
- Keep Matrix SDK encryption and SQLite features enabled.
- Adapt SQLx 0.9 dynamic queries with `AssertSqlSafe` only where every dynamic
  fragment is repository-controlled; production select helpers accept only
  `&'static str` tails and continue binding data values separately.
- Mark `agentd-project-authority` unpublished because it is an internal
  workspace boundary with a version-less path dependency.
- Keep yanked dependencies denied globally; fresh dependency resolution must
  select the non-yanked `spin` release allowed by flume 0.12.
- Keep advisories denied globally, with one reasoned exception for the
  compile-time-only `RUSTSEC-2026-0173` dependency retained by supported Matrix
  SDK releases because no safe upgrade exists.
- Scope `CDLA-Permissive-2.0` and `BSL-1.0` license exceptions to
  `webpki-roots@1` and `xxhash-rust@0.8` respectively.
- Discover artifact-test workspace paths at runtime so shared Cargo caches do
  not retain deleted worktree paths.
- Destroy the Matrix SDK client while its owned Tokio runtime is entered so
  the SQLite connection pool can finish runtime-backed cleanup before that
  runtime is dropped.

## Boundaries

### Allowed Changes

- specs/e2e/p155-secure-matrix-storage-dependency-baseline.spec.md
- docs/superpowers/specs/2026-07-15-secure-matrix-storage-dependency-baseline-design.md
- docs/superpowers/plans/2026-07-15-secure-matrix-storage-dependency-baseline.md
- ./Cargo.toml
- ./clippy.toml
- README.md
- ./deny.toml
- crates/agentd-project-authority/Cargo.toml
- crates/agentd-core/src/engine/execute.rs
- crates/agentd-core/src/graph/edge_select.rs
- crates/agentd-store/src/agent_chat_import.rs
- crates/agentd-store/src/agent_chat_task_graph_repo.rs
- crates/agentd-store/src/agent_chat_task_repo.rs
- crates/agentd-store/src/agent_repo.rs
- crates/agentd-store/src/message_repo.rs
- crates/agentd-store/src/pool.rs
- crates/agentd-store/tests/agent_chat_import.rs
- crates/agentd-store/tests/agent_chat_task_graphs.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-store/tests/migration_backcompat.rs
- crates/agentd-store/tests/outbox.rs
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/sdk_adapter.rs
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/matrix_client_bridge_service.rs
- crates/agentd-specify/tests/client.rs
- crates/agentd-specify/tests/events.rs

### Forbidden

- Do not disable Matrix encryption or the Matrix SDK SQLite store.
- Do not ignore a known runtime vulnerability with an available fixed version.
- Do not lower the global advisory, yanked, wildcard, license, or source policy.
- Do not interpolate user-controlled data into SQL text.
- Do not invoke Claude in tests.

## Out of Scope

- Upgrading Matrix SDK beyond the lowest release that resolves the known
  runtime advisories.
- Replacing Matrix SDK's documentation macro dependency.
- Tracking `Cargo.lock` for this workspace.

## Completion Criteria

Scenario: secure Matrix and SQLite dependency versions are explicit
  Test:
    Package: agentd-matrix
    Filter: secure_matrix_storage_dependency_baseline_is_pinned
  Level: artifact inspection
  Test Double: workspace manifest text
  Given the workspace dependency manifest, Clippy configuration, and README
  When the dependency and MSRV declarations are inspected
  Then Rust 1.94, its Clippy MSRV, SQLx 0.9, and Matrix SDK 0.16.1 are explicit
  And Matrix encryption and SQLite support remain enabled
  And the internal agentd-project-authority crate is unpublished

Scenario: dependency governance exceptions remain narrow and reasoned
  Test:
    Package: agentd-matrix
    Filter: dependency_governance_exceptions_are_scoped
  Level: artifact inspection
  Test Double: cargo-deny configuration text
  Given the dependency governance configuration
  When advisory, yanked, and license policy is inspected
  Then global denial remains enabled
  And each unavoidable exception identifies one advisory, crate, or version
  And every exception records its reason or constrained package

Scenario: all feature dependency governance passes
  Test:
    Package: agentd-matrix
    Filter: sdk_adapter_feature_path_compiles_with_matrix_sdk_enabled
  Level: compile and dependency governance
  Test Double: local Cargo build and advisory database
  Given the secure dependency baseline
  When the Matrix SDK feature path and `cargo deny --all-features check` run
  Then SQLx and Matrix SDK share a compatible SQLite binding
  And no non-exempt advisory, yanked crate, wildcard, license, or source failure remains

Scenario: persistent Matrix SQLite state survives client reopen
  Test:
    Package: agentd-matrix
    Filter: sdk_matrix_client_sqlite_store_reopens_persisted_state
  Level: local storage integration
  Test Double: temporary Matrix SDK SQLite store
  Given a Matrix SDK client configured with a temporary SQLite store
  When repository-owned state is persisted and the client is rebuilt on the same path
  Then the reopened Matrix SDK client reads the persisted state
  And both clients close without requiring an external Tokio runtime

Scenario: SQLx migration preserves store behavior
  Test:
    Package: agentd-store
    Filter: all
  Level: storage integration
  Test Double: temporary SQLite databases
  Given SQLx 0.9 and audited static query fragments
  When the agentd-store test suite runs
  Then migrations, reads, writes, imports, and outbox transactions retain their behavior
