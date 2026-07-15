# Secure Matrix Storage Dependency Baseline Plan

1. [x] Reproduce the protected `cargo deny` failure locally.
2. [x] Upgrade Matrix SDK to the lowest version covering known runtime advisories.
3. [x] Resolve the native SQLite links conflict with stable SQLx 0.9.
4. [x] Adapt SQLx and Matrix SDK API changes without changing behavior.
5. [x] Eliminate wildcard/yanked failures and scope advisory/license exceptions.
6. [x] Add manifest and governance regression tests.
7. [x] Verify fresh dependency resolution, all feature lint, focused suites, and workspace tests.
8. [x] Run the P155 agent-spec lifecycle and guard.
9. [x] Address independent review findings with runtime workspace discovery and a Matrix SQLite reopen test.
10. [ ] Commit, push, and confirm PR #21 protected checks.
