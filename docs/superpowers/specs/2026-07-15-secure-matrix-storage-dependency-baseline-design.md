# Secure Matrix Storage Dependency Baseline Design

## Problem

The AD-E0 candidate's first protected `cargo deny --all-features` run exposed
known vulnerabilities in Matrix SDK 0.9, a vulnerable `time` release, a yanked
transitive dependency, two unlisted licenses, and one internal wildcard path
dependency. Upgrading Matrix SDK alone caused a native SQLite links conflict:
Matrix SDK 0.16.1 uses `libsqlite3-sys` 0.35 while SQLx 0.8 requires an older,
incompatible package version.

## Selected Approach

Upgrade to stable SQLx 0.9 and Matrix SDK 0.16.1 together. SQLx 0.9 accepts
`libsqlite3-sys >=0.30.1,<0.38.0`, so Cargo can resolve one 0.35 binding for
both persistent stores. Raise the workspace MSRV to SQLx 0.9's Rust 1.94
minimum, synchronize Clippy's configured MSRV, and preserve Matrix encryption
plus SQLite features. Synchronizing Clippy activates Rust 1.94's let-chain
guidance, so the few newly reported nested conditions are rewritten without
changing their control flow.

SQLx 0.9 requires dynamic SQL to opt in through `AssertSqlSafe`. Production
helpers constrain suffixes to `&'static str`; values remain prepared-statement
bindings. Tests wrap only repository-owned table names and migration files.

Workspace artifact tests discover the repository root at runtime. This avoids
reusing a cached test binary whose compile-time manifest path points at a
deleted smoke-test worktree.

The Matrix adapter test opens its configured SDK SQLite store, writes a custom
state value, drops the client, reopens the same path, and reads the value back.
This exercises persistent state-store schema creation and reuse without a
homeserver dependency; the same SDK builder path also initializes the crypto
store kept by the `e2e-encryption` feature.

That integration test also exposed an ownership issue: Matrix SDK's SQLite
pool performs Tokio-backed cleanup from its destructor, while
`SdkMatrixClient` could previously drop the SDK client after leaving its owned
runtime. The adapter now explicitly takes and destroys the SDK client while an
enter guard for that runtime is active, before the runtime field itself is
dropped. Callers therefore do not need to supply an ambient Tokio runtime for
normal synchronous teardown.

## Governance Exceptions

Global denial remains unchanged. The configuration records only these scoped
exceptions:

- `RUSTSEC-2026-0173`: compile-time `proc-macro-error2`, retained by
  aquamarine 0.6 in supported Matrix SDK releases with no safe upgrade.
- `CDLA-Permissive-2.0` for `webpki-roots@1`: root certificate data.
- `BSL-1.0` for `xxhash-rust@0.8`: Matrix SDK bloom-filter hashing.

These entries are removal targets when upstream releases permit it. They do not
authorize other crates, versions, licenses, or advisories.

A fresh lock resolution selects non-yanked `spin 0.9.9`; no yanked-package
exception is retained.

## Verification

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- `cargo test -p agentd-store`
- `cargo test -p agentd-matrix --features matrix-sdk-adapter`
- `cargo test -p agentd-matrix sdk_matrix_client_sqlite_store_reopens_persisted_state`
- `cargo test -p agentd-bin --features matrix-sdk-adapter`
- `cargo test --workspace`
- `cargo deny --all-features check`
- fresh dependency resolution without a tracked `Cargo.lock`
