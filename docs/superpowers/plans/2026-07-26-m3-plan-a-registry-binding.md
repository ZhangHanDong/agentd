# M3 Plan A — Agent Registry and Project↔Room↔Repo Binding Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the agentd-owned agent registry (import/update, profile management, offline-recovery) and make the project↔room↔repo binding a durable first-class record with an operator API, while fixing the carried-over fleet HTTP error-status mapping so the whole fleet path honors the error convention.

**Architecture:** Three independent slices land on top of the merged M1/M2 work. (1) A local `ControlPlaneErrorStatus` trait in `agentd-surface` maps `WorkerFleetError`/`TaskLeaseError`/`ProjectBindingError` variants onto real HTTP statuses, and the worker client gains a single `is_retryable` decision point so a transient 503 is retried and every 4xx stays terminal. (2) The existing `agent_repo` (p213/p214/p234 lifecycle) gains a non-destructive roster import upsert, a runtime-profile read/patch API on `RunHost` + `/api/agents/:name/profile`, and a stale-heartbeat sweep that mirrors `worker_repo::mark_stale_workers_offline` and runs in the daemon maintenance tick. (3) A new migration `0025` creates `project_room_repo_bindings`, a first-class agentd-owned record, exposed through a new `ProjectBindingPort` in `agentd-core`, a SQLite implementation in `agentd-store`, and a mountable operator router in `agentd-surface` — the same port/store/router shape the worker fleet already uses.

**Tech Stack:** Rust (edition 2024), tokio, axum 0.7, sqlx (SQLite, embedded `sqlx::migrate!("./migrations")`), serde/serde_json, thiserror, async-trait, tower (`oneshot`) for in-process router tests.

## Non-Goals

These are explicitly **out of scope for Plan A** and must not be started here:

- **Messaging parity (M3 item 2)** — direct inbox, group messaging, read cursors, mentions, dedup. That is **M3 Plan B**.
- **Task graph CRUD/migration and coordination semantics (M3 item 3)** — that is **M3 Plan C**.
- Specify network integration, authority-backed RBAC/quota enforcement, Matrix command normalization, and cutover — those belong to M4/M5 and must remain listed as pending in the parity map.
- Wiring the new binding into `resolve_target_repository_binding` (the daemon's `"unspecified"` evidence sentinel). Plan A creates and exposes the durable record; consuming it on the execution-evidence path is a follow-up, deliberately not bundled here.
- Any change to `agent_profile_repo.rs` (the P267 enterprise profile/worker identity store). "Profile management" in this plan means the agent-chat-compatible `agents.runtime_profile` JSON document, not the enterprise profile record.

## Global Constraints

Every task's requirements implicitly include this section.

- **Error classification is a contract.** `Invalid` → HTTP **400**, `NotFound` → **404**, `Conflict` → **409**, `Unavailable` → **503**. Only `Unavailable` is retryable; every other variant is terminal for the caller. Never collapse variants onto one status.
- **Multi-row mutations use `BEGIN IMMEDIATE` + `rows_affected` guards.** Acquire a connection, run `sqlx::query("BEGIN IMMEDIATE")`, do the work, `COMMIT` on `Ok` and best-effort `ROLLBACK` on `Err`. Every `UPDATE`/`INSERT` that must affect exactly one row asserts `result.rows_affected() == 1` and returns `StoreError::Conflict` otherwise. Copy the shape from `crates/agentd-store/src/durable_scheduler.rs:171-190`.
- **Schema changes are one new migration: `0025`.** File `crates/agentd-store/migrations/0025_project_room_repo_binding.sql`, ending with `UPDATE schema_meta SET value = '25' WHERE key = 'version';`. The **same task** that adds the migration must sweep every `assert_eq!(version, "24")` in `crates/agentd-store/tests/migration.rs` to `"25"` **and** `assert_eq!(report.schema_version, 24);` in `crates/agentd-store/tests/operational_doctor.rs:23` to `25`. Migrations are auto-discovered by `sqlx::migrate!("./migrations")` (`crates/agentd-store/src/pool.rs:14`) — there is no list to register in.
- **Workers and agents never open the daemon DB from a remote path.** All new store access goes through the daemon-side pool that the daemon already owns (`host.store().pool()`), never through a path handed over the wire.
- **Test gates are narrow `--test` runs only.** Use `cargo test -p <crate> --test <file> <test_name>`. Never run workspace `nextest` inside a task, never run two `nextest` invocations concurrently, and never combine multiple `-p` packages in one command (rebuild trap). A single-package `cargo nextest run -p <crate>` at the end of a task is allowed.
- **Rust edition 2024.** Prefer explicit error handling over `.unwrap()` in production code (`agentd-surface` lints `clippy::unwrap_used` and `clippy::panic` as warnings for non-test code); `.expect("…")` with a message is the accepted idiom inside tests.
- **Parity map status changes and their contract tests move together.** Three test files assert on parity rows this plan touches: `crates/agentctl/tests/parity_cli.rs`, `crates/agentctl/tests/worktree_reconciliation_contract.rs`, and `crates/agentctl/tests/enterprise_project_authority_contract.rs`. Any edit to `docs/parity/agent-chat-capability-map.md` runs all three in the same task.

---

### Task 1: Fleet control-plane error → HTTP status mapping

Carried over from the M2 Plan B final review ("Minor 9", high priority). Today `respond()` in both fleet routers returns **400 for every error**, and the worker client classifies 4xx as terminal — so a transient SQLite-busy `Unavailable` is never retried and the worker drops out of the fleet on a database hiccup. This task makes the server emit the right status and gives the client a single retry-decision function.

Note the behavior change this creates on purpose: `SqliteWorkerFleet::authorize` returns `WorkerFleetError::Unavailable` for a bad auth proof (`crates/agentd-store/src/worker_fleet.rs:67-70`), so a wrong worker token now yields 503 and is retried instead of failing immediately. That is correct — the fleet supports overlapping proofs during operator token rotation (`with_auth_proofs`), and the retry loops are bounded (`policy.max_attempts`), so a genuinely wrong token still fails after the bounded retries rather than looping forever.

**Files:**
- Create: `crates/agentd-surface/src/control_plane_status.rs`
- Modify: `crates/agentd-surface/src/lib.rs:11-18` (add the module)
- Modify: `crates/agentd-surface/src/worker_fleet_http.rs:130-139` (`respond`)
- Modify: `crates/agentd-surface/src/worker_fleet_mtls_http.rs:183-192` (`respond`)
- Modify: `crates/agentd-bin/src/worker_fleet_client.rs:96-107, 142-153, 169-180, 200-210, 250-262, 356-391`
- Test: `crates/agentd-bin/tests/worker_fleet_http.rs` (new test appended)
- Test: `crates/agentd-surface/src/control_plane_status.rs` (in-file `#[cfg(test)] mod tests`)
- Test: `crates/agentd-bin/src/worker_fleet_client.rs` (extend the existing in-file `mod tests`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `agentd_surface::control_plane_status::ControlPlaneErrorStatus` — `pub trait ControlPlaneErrorStatus { fn http_status(&self) -> axum::http::StatusCode; }`, implemented for `agentd_core::ports::WorkerFleetError` and `agentd_core::ports::TaskLeaseError`. **Task 6 implements it for `ProjectBindingError`.**
  - `agentd_bin::worker_fleet_client::is_retryable(error: &WorkerFleetError) -> bool` (crate-private `fn is_retryable`).

- [ ] **Step 1: Write the failing status-mapping unit test**

Create `crates/agentd-surface/src/control_plane_status.rs` containing **only** the test module for now:

```rust
//! Maps control-plane port errors onto the project-wide HTTP status
//! convention: Invalid -> 400, NotFound -> 404, Conflict -> 409,
//! Unavailable -> 503. Only 503 is retryable by a client; every other
//! status is terminal. Collapsing variants onto one status is what made
//! transient database contention look like a permanent worker failure.

#[cfg(test)]
mod tests {
    use super::ControlPlaneErrorStatus;
    use agentd_core::ports::{TaskLeaseError, TaskLeaseRejectionReason, WorkerFleetError};
    use axum::http::StatusCode;

    #[test]
    fn worker_fleet_error_variants_map_to_distinct_statuses() {
        assert_eq!(
            WorkerFleetError::Invalid("bad".into()).http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            WorkerFleetError::NotFound("gone".into()).http_status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            WorkerFleetError::Conflict("stale".into()).http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            WorkerFleetError::Unavailable("busy".into()).http_status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn task_lease_error_variants_map_to_distinct_statuses() {
        assert_eq!(
            TaskLeaseError::Invalid("bad".into()).http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            TaskLeaseError::NotFound("gone".into()).http_status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            TaskLeaseError::Conflict("fenced".into()).http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            TaskLeaseError::Rejected {
                reason: TaskLeaseRejectionReason::FencingTokenStale,
                message: "stale".into(),
            }
            .http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            TaskLeaseError::Unavailable("busy".into()).http_status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
}
```

Register the module in `crates/agentd-surface/src/lib.rs` by inserting the line in alphabetical position (before `pub mod error;`):

```rust
pub mod control_plane_status;
pub mod error;
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentd-surface --lib control_plane_status`
Expected: FAIL to compile with `cannot find trait ControlPlaneErrorStatus in this scope` (and, if `TaskLeaseRejectionReason` is not re-exported, adjust the import to `agentd_core::ports::task_lease::TaskLeaseRejectionReason` — check `crates/agentd-core/src/ports/mod.rs:57-60` and use whichever path resolves).

- [ ] **Step 3: Write the trait and impls**

Insert above the `#[cfg(test)] mod tests` block in `crates/agentd-surface/src/control_plane_status.rs`:

```rust
use agentd_core::ports::{TaskLeaseError, WorkerFleetError};
use axum::http::StatusCode;

/// The HTTP status a control-plane port error maps to. Implemented in this
/// crate for foreign port error types (legal: the trait is local).
pub trait ControlPlaneErrorStatus {
    fn http_status(&self) -> StatusCode;
}

impl ControlPlaneErrorStatus for WorkerFleetError {
    fn http_status(&self) -> StatusCode {
        match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

impl ControlPlaneErrorStatus for TaskLeaseError {
    fn http_status(&self) -> StatusCode {
        match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            // A rejected claim is an ownership/fencing conflict: the worker
            // must not retry it, it must re-acquire.
            Self::Conflict(_) | Self::Rejected { .. } => StatusCode::CONFLICT,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentd-surface --lib control_plane_status`
Expected: PASS (2 tests).

- [ ] **Step 5: Wire both fleet routers to the mapping**

In `crates/agentd-surface/src/worker_fleet_http.rs`, replace the `respond` function (lines 130-139) with:

```rust
fn respond<T: serde::Serialize, E: std::fmt::Display + ControlPlaneErrorStatus>(
    result: Result<T, E>,
) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => (
            error.http_status(),
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}
```

and add to its imports (after `use crate::http::AuthConfig;` on line 5):

```rust
use crate::control_plane_status::ControlPlaneErrorStatus;
```

Apply the **identical** replacement to `crates/agentd-surface/src/worker_fleet_mtls_http.rs` (its `respond` is at lines 183-192, byte-identical to the one above), adding the same `use crate::control_plane_status::ControlPlaneErrorStatus;` import.

- [ ] **Step 6: Write the failing end-to-end status test**

Append to `crates/agentd-bin/tests/worker_fleet_http.rs`:

```rust
#[tokio::test]
async fn worker_fleet_http_maps_error_variants_to_distinct_statuses() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = Arc::new(SqliteWorkerFleet::new(store.pool().clone()).with_auth_proof("secret"));
    let mut auth = AuthConfig::open();
    auth.api_token = Some("operator-secret".into());
    let app = worker_fleet_router(fleet, auth);

    let send = |app: axum::Router, path: &'static str, body: serde_json::Value| async move {
        app.oneshot(
            Request::post(path)
                .header("content-type", "application/json")
                .header("authorization", "Bearer operator-secret")
                .body(Body::from(serde_json::to_vec(&body).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response")
    };

    // Invalid -> 400: a worker below the protocol floor.
    let worker_id = WorkerId::new();
    let first_incarnation = WorkerIncarnationId::new();
    let below_floor = json!({
        "auth_proof": "secret",
        "worker_id": worker_id,
        "trust_domain": "local",
        "labels": {},
        "incarnation_id": first_incarnation,
        "daemon_version": "test",
        "host_name": "host",
        "network_zone": serde_json::Value::Null,
        "capabilities": {"runtime": ["native"]},
        "capacity": 1,
        "protocol_version": 0
    });
    let response = send(app.clone(), "/api/worker-fleet/register", below_floor).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // NotFound -> 404: pull for an incarnation that was never registered.
    let unknown_pull = json!({
        "auth_proof": "secret",
        "worker_incarnation_id": WorkerIncarnationId::new(),
        "observed_at": 10,
        "expires_at": 20
    });
    let response = send(app.clone(), "/api/worker-fleet/pull", unknown_pull).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Conflict -> 409: pull on an incarnation superseded by a newer one.
    let register = |incarnation: WorkerIncarnationId| {
        json!({
            "auth_proof": "secret",
            "worker_id": worker_id,
            "trust_domain": "local",
            "labels": {},
            "incarnation_id": incarnation,
            "daemon_version": "test",
            "host_name": "host",
            "network_zone": serde_json::Value::Null,
            "capabilities": {"runtime": ["native"]},
            "capacity": 1,
            "protocol_version": agentd_core::ports::WORKER_PROTOCOL_VERSION
        })
    };
    let response = send(
        app.clone(),
        "/api/worker-fleet/register",
        register(first_incarnation.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = send(
        app.clone(),
        "/api/worker-fleet/register",
        register(WorkerIncarnationId::new()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let stale_pull = json!({
        "auth_proof": "secret",
        "worker_incarnation_id": first_incarnation,
        "observed_at": 10,
        "expires_at": 20
    });
    let response = send(app.clone(), "/api/worker-fleet/pull", stale_pull).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);

    // Unavailable -> 503: a wrong auth proof is transient (token rotation),
    // so the worker must retry it rather than treat it as terminal.
    let wrong_proof = json!({
        "auth_proof": "wrong",
        "worker_incarnation_id": WorkerIncarnationId::new(),
        "observed_at": 10,
        "expires_at": 20
    });
    let response = send(app, "/api/worker-fleet/pull", wrong_proof).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}
```

- [ ] **Step 7: Run the end-to-end test**

Run: `cargo test -p agentd-bin --test worker_fleet_http worker_fleet_http_maps_error_variants_to_distinct_statuses`
Expected: PASS.

- [ ] **Step 8: Write the failing client retry-classification test**

In `crates/agentd-bin/src/worker_fleet_client.rs`, replace the whole existing `#[cfg(test)] mod tests` block (lines 367-391) with:

```rust
#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::{classify_http_error, is_retryable};
    use agentd_core::ports::WorkerFleetError;

    #[test]
    fn transient_http_statuses_trigger_reconnect() {
        assert!(matches!(
            classify_http_error(503, "down"),
            WorkerFleetError::Unavailable(_)
        ));
        assert!(matches!(
            classify_http_error(429, "busy"),
            WorkerFleetError::Unavailable(_)
        ));
        assert!(matches!(
            classify_http_error(409, "stale"),
            WorkerFleetError::Conflict(_)
        ));
        assert!(matches!(
            classify_http_error(404, "missing"),
            WorkerFleetError::NotFound(_)
        ));
        assert!(matches!(
            classify_http_error(400, "bad input"),
            WorkerFleetError::Invalid(_)
        ));
    }

    #[test]
    fn only_service_unavailable_is_retried() {
        assert!(is_retryable(&classify_http_error(503, "down")));
        assert!(is_retryable(&classify_http_error(429, "busy")));
        for status in [400_u16, 404, 409, 422] {
            assert!(
                !is_retryable(&classify_http_error(status, "terminal")),
                "status {status} must be terminal for the worker"
            );
        }
    }
}
```

- [ ] **Step 9: Run the client test to verify it fails**

Run: `cargo test -p agentd-bin --lib worker_fleet_client`
Expected: FAIL to compile with `cannot find function is_retryable in this scope`.

- [ ] **Step 10: Add `is_retryable`, sharpen `classify_http_error`, and use it at every retry site**

In `crates/agentd-bin/src/worker_fleet_client.rs`, replace `classify_http_error` (lines 356-366) with:

```rust
fn classify_http_error(status: u16, body: &str) -> WorkerFleetError {
    let message = body.to_string();
    match status {
        400 => WorkerFleetError::Invalid(message),
        404 => WorkerFleetError::NotFound(message),
        408 | 425 | 429 | 500..=599 => WorkerFleetError::Unavailable(message),
        401..=499 => WorkerFleetError::Conflict(message),
        _ => WorkerFleetError::Unavailable(message),
    }
}

/// The single retry decision for every worker-side control-plane call. Only
/// `Unavailable` is transient; identity, validation, and lease conflicts are
/// terminal and must surface to the supervisor instead of spinning.
const fn is_retryable(error: &WorkerFleetError) -> bool {
    matches!(error, WorkerFleetError::Unavailable(_))
}
```

Then replace each of the three retry guards — in `pull_with_retry` (line 99-102), `heartbeat_with_retry` (line 145-148), and `register_with_retry` (line 172-175) — which currently read:

```rust
                Err(error)
                    if matches!(error, WorkerFleetError::Unavailable(_))
                        && attempt + 1 < attempts =>
```

with:

```rust
                Err(error) if is_retryable(&error) && attempt + 1 < attempts =>
```

- [ ] **Step 11: Run both client and surface tests**

Run: `cargo test -p agentd-bin --lib worker_fleet_client && cargo test -p agentd-surface --lib control_plane_status`
Expected: PASS.

- [ ] **Step 12: Run the task gate**

Run: `cargo test -p agentd-bin --test worker_fleet_http && cargo test -p agentd-bin --test worker_main && cargo test -p agentd-bin --test native_dispatch`
Expected: PASS. (`worker_main` and `native_dispatch` exercise the pull/heartbeat paths whose classification just changed.)

Run: `cargo nextest run -p agentd-surface`
Expected: PASS.

- [ ] **Step 13: Commit**

```bash
git add crates/agentd-surface/src/control_plane_status.rs \
        crates/agentd-surface/src/lib.rs \
        crates/agentd-surface/src/worker_fleet_http.rs \
        crates/agentd-surface/src/worker_fleet_mtls_http.rs \
        crates/agentd-bin/src/worker_fleet_client.rs \
        crates/agentd-bin/tests/worker_fleet_http.rs
git commit -m "fix(fleet): map control-plane error variants to real HTTP statuses"
```

---

### Task 2: Agent registry import/update (non-destructive roster upsert)

`agent_repo::register_agent` is a full overwrite: it recomputes `status`/`offline_reason`/`last_seen_at` from the supplied `tmux_target` and replaces `runtime_profile` wholesale. That is right for a live registration but wrong for an **import/update**, which re-applies a roster (p216's `agents.json` import calls `register_agent` today, `crates/agentd-store/src/agent_chat_import.rs:703-720`) and must not knock a running agent offline or discard profile keys the operator added later (for example `identity`, written by `update_agent_identity`).

This task adds an idempotent import upsert that updates roster identity fields and **merges** the runtime profile, while never touching liveness state (`status`, `offline_reason`, `tmux_target`, `backend_target`, `last_seen_at`, `registered_at`), and rewires the agent-chat import to use it.

**Files:**
- Modify: `crates/agentd-store/src/agent_repo.rs` (add types + `import_agent_profile`, after `register_agent` which ends at line 190)
- Modify: `crates/agentd-store/src/agent_chat_import.rs:703-720` (`import_agent`)
- Test: `crates/agentd-store/tests/agent_registry.rs` (new tests appended)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `agentd_store::agent_repo::AgentImport` — `pub struct AgentImport { pub name: String, pub role: Option<String>, pub capability: Option<String>, pub runtime: Option<String>, pub model: Option<String>, pub home_dir: Option<String>, pub workdir: Option<String>, pub state_dir: Option<String>, pub server: Option<String>, pub runtime_profile: Value }`
  - `agentd_store::agent_repo::AgentImportOutcome` — `pub enum AgentImportOutcome { Created, Updated }` (derives `Debug, Clone, Copy, PartialEq, Eq`)
  - `agentd_store::agent_repo::import_agent_profile(pool: &SqlitePool, input: AgentImport) -> Result<(AgentRecord, AgentImportOutcome), StoreError>`

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-store/tests/agent_registry.rs`:

```rust
#[tokio::test]
async fn agent_import_creates_then_updates_without_disturbing_liveness() {
    let (store, _dir) = open_temp().await;

    let (created, outcome) = agent_repo::import_agent_profile(
        store.pool(),
        agent_repo::AgentImport {
            name: "codex-dev".to_string(),
            role: Some(text("implementer")),
            capability: Some(text("strong")),
            runtime: Some(text("codex")),
            model: Some(text("gpt-5")),
            home_dir: Some(text("/tmp/homes/codex-dev")),
            workdir: Some(text("/tmp/homes/codex-dev/workdir")),
            state_dir: Some(text("/tmp/homes/codex-dev/state")),
            server: Some(text("local")),
            runtime_profile: json!({ "primary": { "framework": "codex" } }),
        },
    )
    .await
    .expect("import create");

    assert_eq!(outcome, agent_repo::AgentImportOutcome::Created);
    assert_eq!(created.name, "codex-dev");
    // An import never claims liveness it did not observe.
    assert_eq!(created.status, "offline");
    assert_eq!(created.offline_reason.as_deref(), Some("imported"));
    assert_eq!(created.tmux_target, None);
    assert_eq!(created.last_seen_at, None);

    // The agent comes online and the operator sets an identity.
    agent_repo::mark_agent_started(
        store.pool(),
        "codex-dev",
        agent_repo::StartedAgent {
            tmux_target: "codex-dev:0.0".to_string(),
        },
    )
    .await
    .expect("start")
    .expect("agent exists");
    agent_repo::update_agent_identity(store.pool(), "codex-dev", "Be concise")
        .await
        .expect("identity")
        .expect("agent exists");

    // Re-importing the roster with a changed model must update the roster
    // fields and merge the profile, without knocking the agent offline or
    // discarding the operator's identity key.
    let (updated, outcome) = agent_repo::import_agent_profile(
        store.pool(),
        agent_repo::AgentImport {
            name: "codex-dev".to_string(),
            role: Some(text("reviewer")),
            capability: None,
            runtime: Some(text("codex")),
            model: Some(text("gpt-5.1")),
            home_dir: None,
            workdir: None,
            state_dir: None,
            server: None,
            runtime_profile: json!({ "primary": { "framework": "codex" }, "extraArgs": ["-q"] }),
        },
    )
    .await
    .expect("import update");

    assert_eq!(outcome, agent_repo::AgentImportOutcome::Updated);
    assert_eq!(updated.role.as_deref(), Some("reviewer"));
    assert_eq!(updated.model.as_deref(), Some("gpt-5.1"));
    // Omitted roster fields are preserved, not nulled.
    assert_eq!(updated.capability.as_deref(), Some("strong"));
    assert_eq!(updated.server.as_deref(), Some("local"));
    // Liveness is untouched.
    assert_eq!(updated.status, "online");
    assert_eq!(updated.tmux_target.as_deref(), Some("codex-dev:0.0"));
    // Profile merge: imported keys win, operator-owned keys survive.
    assert_eq!(updated.runtime_profile["identity"], "Be concise");
    assert_eq!(updated.runtime_profile["extraArgs"][0], "-q");
    assert_eq!(updated.runtime_profile["primary"]["framework"], "codex");
}

#[tokio::test]
async fn agent_import_rejects_a_blank_name() {
    let (store, _dir) = open_temp().await;
    let error = agent_repo::import_agent_profile(
        store.pool(),
        agent_repo::AgentImport {
            name: "   ".to_string(),
            role: None,
            capability: None,
            runtime: None,
            model: None,
            home_dir: None,
            workdir: None,
            state_dir: None,
            server: None,
            runtime_profile: json!({}),
        },
    )
    .await
    .expect_err("blank name must be rejected");
    assert!(error.to_string().contains("agent name required"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentd-store --test agent_registry agent_import_`
Expected: FAIL to compile with `cannot find function import_agent_profile in module agent_repo`.

- [ ] **Step 3: Implement the import upsert**

Insert into `crates/agentd-store/src/agent_repo.rs` immediately after `register_agent` (after line 190):

```rust
/// Roster-shaped import input. Unlike [`RegisterAgent`], this never carries
/// liveness state: an import re-applies who an agent *is*, not whether it is
/// currently running.
#[derive(Debug, Clone)]
pub struct AgentImport {
    pub name: String,
    pub role: Option<String>,
    pub capability: Option<String>,
    pub runtime: Option<String>,
    pub model: Option<String>,
    pub home_dir: Option<String>,
    pub workdir: Option<String>,
    pub state_dir: Option<String>,
    pub server: Option<String>,
    pub runtime_profile: Value,
}

/// Whether an import created a new registry row or updated an existing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentImportOutcome {
    Created,
    Updated,
}

/// Idempotently import or update one agent and its runtime profile.
///
/// Roster fields are only overwritten when the import supplies them (`None`
/// preserves the stored value). The runtime profile is merged key-by-key with
/// the import winning, so operator-owned keys such as `identity` survive a
/// re-import. Liveness columns (`status`, `offline_reason`, `tmux_target`,
/// `backend_target`, `last_seen_at`, `registered_at`) are never written here —
/// only heartbeat/start/offline own those.
///
/// # Errors
/// [`StoreError::Invariant`] for a blank name, [`StoreError::Conflict`] if the
/// row changed concurrently, [`StoreError::Sqlx`] on a database failure.
pub async fn import_agent_profile(
    pool: &SqlitePool,
    input: AgentImport,
) -> Result<(AgentRecord, AgentImportOutcome), StoreError> {
    let name = normalize_name(&input.name)?;
    let role = clean_opt(input.role);
    let capability = clean_opt(input.capability);
    let runtime = clean_opt(input.runtime);
    let model = clean_opt(input.model);
    let home_dir = clean_opt(input.home_dir);
    let workdir = clean_opt(input.workdir);
    let state_dir = clean_opt(input.state_dir);
    let server = clean_opt(input.server);
    let import_profile = normalize_runtime_profile(input.runtime_profile);
    let now = now_unix();

    let mut connection = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *connection).await?;
    let result = import_in_transaction(
        &mut connection,
        &name,
        ImportColumns {
            role: role.as_deref(),
            capability: capability.as_deref(),
            runtime: runtime.as_deref(),
            model: model.as_deref(),
            home_dir: home_dir.as_deref(),
            workdir: workdir.as_deref(),
            state_dir: state_dir.as_deref(),
            server: server.as_deref(),
        },
        &import_profile,
        now,
    )
    .await;
    let outcome = match result {
        Ok(outcome) => {
            sqlx::query("COMMIT").execute(&mut *connection).await?;
            outcome
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            return Err(error);
        }
    };
    drop(connection);

    let record = get_agent(pool, &name)
        .await?
        .ok_or_else(|| StoreError::Invariant(format!("imported agent '{name}' is missing")))?;
    Ok((record, outcome))
}

struct ImportColumns<'a> {
    role: Option<&'a str>,
    capability: Option<&'a str>,
    runtime: Option<&'a str>,
    model: Option<&'a str>,
    home_dir: Option<&'a str>,
    workdir: Option<&'a str>,
    state_dir: Option<&'a str>,
    server: Option<&'a str>,
}

async fn import_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    name: &str,
    columns: ImportColumns<'_>,
    import_profile: &Value,
    now: i64,
) -> Result<AgentImportOutcome, StoreError> {
    let existing: Option<String> =
        sqlx::query_scalar("SELECT runtime_profile FROM agents WHERE name = ? OR id = ?")
            .bind(name)
            .bind(name)
            .fetch_optional(&mut *connection)
            .await?;

    let Some(existing_profile_text) = existing else {
        let profile_text = serde_json::to_string(import_profile)?;
        let inserted = sqlx::query(
            "INSERT INTO agents \
             (id, mxid, role, backend, backend_target, prompt_profile, enabled, created_at, \
              name, capability, runtime, model, tmux_target, home_dir, workdir, state_dir, \
              server, status, offline_reason, last_seen_at, registered_at, updated_at, runtime_profile) \
             VALUES (?, ?, ?, ?, NULL, NULL, 1, ?, ?, ?, ?, ?, NULL, ?, ?, ?, ?, 'offline', \
              'imported', NULL, ?, ?, ?)",
        )
        .bind(name)
        .bind(local_mxid(name))
        .bind(columns.role.unwrap_or("agent"))
        .bind(columns.runtime.unwrap_or("agent"))
        .bind(now)
        .bind(name)
        .bind(columns.capability)
        .bind(columns.runtime)
        .bind(columns.model)
        .bind(columns.home_dir)
        .bind(columns.workdir)
        .bind(columns.state_dir)
        .bind(columns.server)
        .bind(now)
        .bind(now)
        .bind(profile_text)
        .execute(&mut *connection)
        .await?;
        if inserted.rows_affected() != 1 {
            return Err(StoreError::Conflict(format!(
                "agent '{name}' was created concurrently"
            )));
        }
        return Ok(AgentImportOutcome::Created);
    };

    let merged = merge_runtime_profile(&existing_profile_text, import_profile);
    let merged_text = serde_json::to_string(&merged)?;
    let updated = sqlx::query(
        "UPDATE agents SET \
          role = COALESCE(?, role), \
          capability = COALESCE(?, capability), \
          runtime = COALESCE(?, runtime), \
          backend = COALESCE(?, backend), \
          model = COALESCE(?, model), \
          home_dir = COALESCE(?, home_dir), \
          workdir = COALESCE(?, workdir), \
          state_dir = COALESCE(?, state_dir), \
          server = COALESCE(?, server), \
          runtime_profile = ?, \
          updated_at = ? \
         WHERE name = ? OR id = ?",
    )
    .bind(columns.role)
    .bind(columns.capability)
    .bind(columns.runtime)
    .bind(columns.runtime)
    .bind(columns.model)
    .bind(columns.home_dir)
    .bind(columns.workdir)
    .bind(columns.state_dir)
    .bind(columns.server)
    .bind(merged_text)
    .bind(now)
    .bind(name)
    .bind(name)
    .execute(&mut *connection)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "agent '{name}' changed concurrently"
        )));
    }
    Ok(AgentImportOutcome::Updated)
}

/// Merge an imported profile document over the stored one, key by key at the
/// top level. Imported keys win; keys only the store has (an operator-set
/// `identity`, for example) survive.
fn merge_runtime_profile(existing_text: &str, import_profile: &Value) -> Value {
    let mut merged = match serde_json::from_str::<Value>(existing_text) {
        Ok(Value::Object(map)) => Value::Object(map),
        _ => json!({}),
    };
    let Some(import_map) = import_profile.as_object() else {
        return merged;
    };
    let target = merged
        .as_object_mut()
        .expect("merged runtime_profile is an object");
    for (key, value) in import_map {
        target.insert(key.clone(), value.clone());
    }
    merged
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentd-store --test agent_registry agent_import_`
Expected: PASS (2 tests).

- [ ] **Step 5: Route the agent-chat roster import through the new upsert**

In `crates/agentd-store/src/agent_chat_import.rs`, replace the whole `import_agent` function (lines 703-736) with the version below. The only change is the first call: `register_agent` becomes `import_agent_profile` and `tmux_target` is dropped, because an imported roster does not assert a live tmux pane. The `online == Some(false)` tail is preserved verbatim — an explicit "this agent is offline" statement in `agents.json` is still honored:

```rust
async fn import_agent(pool: &SqlitePool, agent: &ImportAgent) -> Result<(), StoreError> {
    agent_repo::import_agent_profile(
        pool,
        agent_repo::AgentImport {
            name: agent.name.clone(),
            role: agent.role.clone(),
            capability: agent.capability.clone(),
            runtime: agent.runtime.clone(),
            model: agent.model.clone(),
            home_dir: agent.home_dir.clone(),
            workdir: agent.workdir.clone(),
            state_dir: agent.state_dir.clone(),
            server: agent.server.clone(),
            runtime_profile: agent.runtime_profile.clone(),
        },
    )
    .await?;

    if agent.online == Some(false) {
        agent_repo::mark_agent_offline(
            pool,
            &agent.name,
            OfflineAgent {
                reason: agent
                    .offline_reason
                    .clone()
                    .or_else(|| Some("agent-chat-offline".to_string())),
                clear_tmux: false,
            },
        )
        .await?;
    }
    Ok(())
}
```

`RegisterAgent` is now unreferenced in this file. Change the import on line 11 from:

```rust
use crate::agent_repo::{self, OfflineAgent, RegisterAgent};
```

to:

```rust
use crate::agent_repo::{self, OfflineAgent};
```

- [ ] **Step 6: Run the import tests**

Run: `cargo test -p agentd-store --test agent_chat_import`
Expected: PASS unchanged. These tests assert import counts, drift, and task/message rows; none of them assert an imported agent's `status` or `tmux_target`, so the liveness change is invisible to them. If one does fail on a status expectation, the expectation is now wrong by design — change it to `status == "offline"` with `offline_reason == Some("imported")` and say so in the commit body.

- [ ] **Step 7: Run the task gate**

Run: `cargo test -p agentd-store --test agent_registry && cargo test -p agentd-store --test agent_chat_import && cargo test -p agentd-store --test migration_backcompat`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/agentd-store/src/agent_repo.rs \
        crates/agentd-store/src/agent_chat_import.rs \
        crates/agentd-store/tests/agent_registry.rs
git commit -m "feat(registry): non-destructive agent roster import with profile merge"
```

---

### Task 3: Agent runtime profile management API

`agents.runtime_profile` can currently only be written wholesale by `register_agent` or one key at a time by `update_agent_identity`. Operators have no way to read it back as a document or patch arbitrary keys. This task adds a store-level read/patch pair, exposes them on `RunHost`, and mounts `GET`/`PATCH /api/agents/:name/profile`.

**Files:**
- Modify: `crates/agentd-store/src/agent_repo.rs` (add `get_agent_profile`, `update_agent_profile`)
- Modify: `crates/agentd-surface/src/host.rs:104-108` (add `AgentProfilePatch`) and `:1013-1017` (add two trait methods after `update_agent_identity`)
- Modify: `crates/agentd-bin/src/host.rs:1479-1489` (implement both on `ProductionRunHost`)
- Modify: `crates/agentd-surface/src/test_support.rs:415-436` (implement both on `FakeRunHost`)
- Modify: `crates/agentd-surface/src/http.rs:34-43` (import), `:185-189` (route), and add two handlers after `update_agent_identity` (ends line 1314)
- Test: `crates/agentd-store/tests/agent_registry.rs`, `crates/agentd-surface/tests/http.rs`

**Interfaces:**
- Consumes: `agentd_store::agent_repo::AgentRecord` (existing).
- Produces:
  - `agentd_store::agent_repo::get_agent_profile(pool: &SqlitePool, name: &str) -> Result<Option<Value>, StoreError>`
  - `agentd_store::agent_repo::update_agent_profile(pool: &SqlitePool, name: &str, patch: Value, replace: bool) -> Result<Option<AgentRecord>, StoreError>`
  - `agentd_surface::host::AgentProfilePatch { pub profile: Value, pub replace: bool }`
  - `RunHost::get_agent_profile(&self, name: &str) -> Result<Option<Value>, CoreError>`
  - `RunHost::update_agent_profile(&self, name: &str, patch: Value, replace: bool) -> Result<Option<AgentRecord>, CoreError>`
  - HTTP `GET /api/agents/:name/profile` → `200 {"agent": "<name>", "runtimeProfile": {…}}` / `404 {"error":"agent_not_found"}`
  - HTTP `PATCH /api/agents/:name/profile` body `{"profile": {…}, "replace": false}` → `200 {"ok": true, "agent": {…}}` / `400` on a non-object profile / `404` on unknown agent

- [ ] **Step 1: Write the failing store test**

Append to `crates/agentd-store/tests/agent_registry.rs`:

```rust
#[tokio::test]
async fn agent_profile_reads_back_and_patches_by_merge_or_replace() {
    let (store, _dir) = open_temp().await;
    agent_repo::register_agent(
        store.pool(),
        agent_repo::RegisterAgent {
            name: "codex-prof".to_string(),
            role: None,
            capability: None,
            runtime: Some(text("codex")),
            model: None,
            tmux_target: None,
            home_dir: None,
            workdir: None,
            state_dir: None,
            server: None,
            runtime_profile: json!({ "primary": { "framework": "codex" }, "identity": "Terse" }),
        },
    )
    .await
    .expect("register");

    let profile = agent_repo::get_agent_profile(store.pool(), "codex-prof")
        .await
        .expect("read")
        .expect("agent exists");
    assert_eq!(profile["primary"]["framework"], "codex");
    assert_eq!(profile["identity"], "Terse");

    let merged = agent_repo::update_agent_profile(
        store.pool(),
        "codex-prof",
        json!({ "extraArgs": ["--json"] }),
        false,
    )
    .await
    .expect("merge patch")
    .expect("agent exists");
    assert_eq!(merged.runtime_profile["identity"], "Terse");
    assert_eq!(merged.runtime_profile["extraArgs"][0], "--json");

    let replaced = agent_repo::update_agent_profile(
        store.pool(),
        "codex-prof",
        json!({ "primary": { "framework": "claude" } }),
        true,
    )
    .await
    .expect("replace patch")
    .expect("agent exists");
    assert_eq!(replaced.runtime_profile["primary"]["framework"], "claude");
    assert_eq!(replaced.runtime_profile.get("identity"), None);

    assert!(
        agent_repo::get_agent_profile(store.pool(), "ghost")
            .await
            .expect("read missing")
            .is_none()
    );

    let error =
        agent_repo::update_agent_profile(store.pool(), "codex-prof", json!(["not an object"]), false)
            .await
            .expect_err("non-object profile must be rejected");
    assert!(error.to_string().contains("runtime profile must be a JSON object"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentd-store --test agent_registry agent_profile_reads_back`
Expected: FAIL to compile with `cannot find function get_agent_profile in module agent_repo`.

- [ ] **Step 3: Implement the store functions**

Insert into `crates/agentd-store/src/agent_repo.rs` immediately after `update_agent_identity` (after line 109):

```rust
/// Read one agent's runtime profile document. `None` means unknown agent.
///
/// # Errors
/// [`StoreError::Invariant`] for a blank name, [`StoreError::Sqlx`] on a
/// database failure.
pub async fn get_agent_profile(
    pool: &SqlitePool,
    name: &str,
) -> Result<Option<Value>, StoreError> {
    Ok(get_agent(pool, name).await?.map(|agent| agent.runtime_profile))
}

/// Patch one agent's runtime profile. `replace = false` merges the patch over
/// the stored document at the top level; `replace = true` swaps the document
/// wholesale. `None` means unknown agent.
///
/// # Errors
/// [`StoreError::Invariant`] for a blank name or a non-object patch,
/// [`StoreError::Conflict`] if the row changed concurrently,
/// [`StoreError::Sqlx`] on a database failure.
pub async fn update_agent_profile(
    pool: &SqlitePool,
    name: &str,
    patch: Value,
    replace: bool,
) -> Result<Option<AgentRecord>, StoreError> {
    let name = normalize_name(name)?;
    if !patch.is_object() {
        return Err(StoreError::Invariant(
            "runtime profile must be a JSON object".to_string(),
        ));
    }
    let Some(agent) = get_agent(pool, &name).await? else {
        return Ok(None);
    };
    let next = if replace {
        patch
    } else {
        let existing_text = serde_json::to_string(&agent.runtime_profile)?;
        merge_runtime_profile(&existing_text, &patch)
    };
    let next_text = serde_json::to_string(&next)?;
    let now = now_unix();
    let updated = sqlx::query(
        "UPDATE agents SET runtime_profile = ?, updated_at = ? WHERE name = ? OR id = ?",
    )
    .bind(next_text)
    .bind(now)
    .bind(&name)
    .bind(&name)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "agent '{name}' changed concurrently"
        )));
    }
    get_agent(pool, &name).await
}
```

`merge_runtime_profile` is the helper added in Task 2; it lives in the same module, so no import is needed. If Task 2 has not landed, add it now from Task 2 Step 3.

- [ ] **Step 4: Run the store test to verify it passes**

Run: `cargo test -p agentd-store --test agent_registry agent_profile_reads_back`
Expected: PASS.

- [ ] **Step 5: Write the failing HTTP test**

Append to `crates/agentd-surface/tests/http.rs`:

```rust
#[tokio::test]
async fn http_agent_profile_reads_and_patches_runtime_profile() {
    let app = app(FakeRunHost::new());
    let register = post(
        app.clone(),
        "/api/agents",
        &json!({
            "name": "codex-worker",
            "runtime": "codex",
            "runtime_profile": { "primary": { "framework": "codex" } }
        })
        .to_string(),
    )
    .await;
    assert_eq!(register.status(), StatusCode::OK);

    let read = get(app.clone(), "/api/agents/codex-worker/profile").await;
    assert_eq!(read.status(), StatusCode::OK);
    let read: Value = serde_json::from_str(&body_string(read).await).expect("profile json");
    assert_eq!(read["agent"], "codex-worker");
    assert_eq!(read["runtimeProfile"]["primary"]["framework"], "codex");

    let patched = patch(
        app.clone(),
        "/api/agents/codex-worker/profile",
        &json!({ "profile": { "extraArgs": ["--json"] } }).to_string(),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::OK);
    let patched: Value = serde_json::from_str(&body_string(patched).await).expect("patch json");
    assert_eq!(patched["ok"], true);
    assert_eq!(patched["agent"]["runtime_profile"]["extraArgs"][0], "--json");
    assert_eq!(
        patched["agent"]["runtime_profile"]["primary"]["framework"],
        "codex"
    );

    let replaced = patch(
        app.clone(),
        "/api/agents/codex-worker/profile",
        &json!({ "profile": { "primary": { "framework": "claude" } }, "replace": true }).to_string(),
    )
    .await;
    assert_eq!(replaced.status(), StatusCode::OK);
    let replaced: Value = serde_json::from_str(&body_string(replaced).await).expect("replace json");
    assert_eq!(
        replaced["agent"]["runtime_profile"]["primary"]["framework"],
        "claude"
    );
    assert_eq!(replaced["agent"]["runtime_profile"].get("extraArgs"), None);

    let bad = patch(
        app.clone(),
        "/api/agents/codex-worker/profile",
        &json!({ "profile": ["not an object"] }).to_string(),
    )
    .await;
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);

    let missing_read = get(app.clone(), "/api/agents/ghost/profile").await;
    assert_eq!(missing_read.status(), StatusCode::NOT_FOUND);
    let missing_patch = patch(
        app,
        "/api/agents/ghost/profile",
        &json!({ "profile": { "a": 1 } }).to_string(),
    )
    .await;
    assert_eq!(missing_patch.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 6: Run the HTTP test to verify it fails**

Run: `cargo test -p agentd-surface --test http http_agent_profile_reads_and_patches_runtime_profile`
Expected: FAIL with `assertion `left == right` failed: left: 405, right: 200` (no such route).

- [ ] **Step 7: Add the `RunHost` surface**

In `crates/agentd-surface/src/host.rs`, insert after `AgentIdentityPatch` (after line 108):

```rust
/// Operator-managed runtime-profile update input for
/// `PATCH /api/agents/:name/profile`. `replace` swaps the whole document;
/// the default merges the patch over the stored one at the top level.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentProfilePatch {
    pub profile: Value,
    #[serde(default)]
    pub replace: bool,
}
```

and insert into the `RunHost` trait immediately after `update_agent_identity` (after line 1017):

```rust
    /// Read one local agent's runtime profile document. `None` means unknown
    /// agent.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn get_agent_profile(&self, name: &str) -> Result<Option<Value>, CoreError>;

    /// Patch one local agent's runtime profile. `None` means unknown agent.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn update_agent_profile(
        &self,
        name: &str,
        patch: Value,
        replace: bool,
    ) -> Result<Option<AgentRecord>, CoreError>;
```

- [ ] **Step 8: Implement on the production host**

In `crates/agentd-bin/src/host.rs`, insert immediately after the `update_agent_identity` implementation (after line 1489):

```rust
    async fn get_agent_profile(
        &self,
        name: &str,
    ) -> Result<Option<serde_json::Value>, CoreError> {
        Ok(agent_repo::get_agent_profile(self.store.pool(), name).await?)
    }

    async fn update_agent_profile(
        &self,
        name: &str,
        patch: serde_json::Value,
        replace: bool,
    ) -> Result<Option<SurfaceAgentRecord>, CoreError> {
        Ok(
            agent_repo::update_agent_profile(self.store.pool(), name, patch, replace)
                .await?
                .map(surface_agent_record),
        )
    }
```

- [ ] **Step 9: Implement on the fake host**

In `crates/agentd-surface/src/test_support.rs`, insert immediately after the `update_agent_identity` implementation (after line 436):

```rust
    async fn get_agent_profile(&self, name: &str) -> Result<Option<serde_json::Value>, CoreError> {
        let name = normalize_agent_name(name)?;
        let agents = self.agents.lock().expect("agents lock");
        Ok(agents.get(&name).map(|record| record.runtime_profile.clone()))
    }

    async fn update_agent_profile(
        &self,
        name: &str,
        patch: serde_json::Value,
        replace: bool,
    ) -> Result<Option<AgentRecord>, CoreError> {
        let name = normalize_agent_name(name)?;
        if !patch.is_object() {
            return Err(CoreError::Invariant(
                "runtime profile must be a JSON object".to_string(),
            ));
        }
        let mut agents = self.agents.lock().expect("agents lock");
        let Some(record) = agents.get_mut(&name) else {
            return Ok(None);
        };
        if replace {
            record.runtime_profile = patch;
        } else {
            if !record.runtime_profile.is_object() {
                record.runtime_profile = serde_json::json!({});
            }
            let target = record
                .runtime_profile
                .as_object_mut()
                .expect("runtime_profile normalized to object");
            for (key, value) in patch.as_object().expect("patch is an object") {
                target.insert(key.clone(), value.clone());
            }
        }
        record.updated_at = 5;
        Ok(Some(record.clone()))
    }
```

- [ ] **Step 10: Add the route and handlers**

In `crates/agentd-surface/src/http.rs`, add `AgentProfilePatch` to the `use crate::host::{…}` list (line 37, alphabetically between `AgentOffline` and `AgentRegistration` — the list is sorted, so it goes right after `AgentOffline,`).

Add the route immediately after the `/api/agents/:name/launch-env` route (line 189):

```rust
        .route(
            "/api/agents/:name/profile",
            get(get_agent_profile).patch(patch_agent_profile),
        )
```

Add the handlers immediately after `update_agent_identity` (after line 1314):

```rust
async fn get_agent_profile(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.get_agent_profile(&name).await {
        Ok(Some(profile)) => {
            Json(json!({ "agent": name, "runtimeProfile": profile })).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "agent_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}

async fn patch_agent_profile(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<AgentProfilePatch>,
) -> Response {
    if let Err(err) = require_local_operator(&state.auth, &headers) {
        return err.into_response();
    }
    match state
        .host
        .update_agent_profile(&name, req.profile, req.replace)
        .await
    {
        Ok(Some(agent)) => Json(json!({ "ok": true, "agent": agent })).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "agent_not_found" })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}
```

`agent_error_response` already maps `CoreError::Invariant` to 400 (`crates/agentd-surface/src/http.rs:1716-1727`), which is how the non-object-profile case returns 400.

- [ ] **Step 11: Run the tests to verify they pass**

Run: `cargo test -p agentd-surface --test http http_agent_profile_reads_and_patches_runtime_profile`
Expected: PASS.

- [ ] **Step 12: Run the task gate**

Run: `cargo test -p agentd-store --test agent_registry && cargo test -p agentd-surface --test http && cargo test -p agentd-bin --test daemon_http`
Expected: PASS.

Run: `cargo nextest run -p agentd-surface`
Expected: PASS.

- [ ] **Step 13: Commit**

```bash
git add crates/agentd-store/src/agent_repo.rs \
        crates/agentd-store/tests/agent_registry.rs \
        crates/agentd-surface/src/host.rs \
        crates/agentd-surface/src/http.rs \
        crates/agentd-surface/src/test_support.rs \
        crates/agentd-surface/tests/http.rs \
        crates/agentd-bin/src/host.rs
git commit -m "feat(registry): read and patch agent runtime profiles over the operator API"
```

---

### Task 4: Agent offline-recovery hardening (stale-heartbeat sweep)

**The concrete gap.** The worker fleet fences a worker that stops heartbeating: `worker_repo::mark_stale_workers_offline` is called every 5 seconds from `worker_fleet_tick` via `WorkerFleetPort::recover_offline` (`crates/agentd-bin/src/daemon.rs:114-124`). The **agent registry has no equivalent.** `agents.last_seen_at` is written by heartbeat/start/runtime updates but nothing ever reads it: an agent that dies silently stays `status = 'online'` forever, so `agentctl agent ls`, the scheduler's pool view, and the Matrix `!agents` command all keep advertising a dead agent. The only way back to `offline` today is an explicit operator `POST /api/agents/:name/offline` (p234's `down`).

This task closes that: a `mark_stale_agents_offline` sweep with the same `BEGIN IMMEDIATE` + `rows_affected` discipline, wired into the existing maintenance tick. The sweep deliberately **preserves `tmux_target`** so p234's `rebind`/session recovery can still reattach a pane that outlived the heartbeat, and it stamps a distinguishable `offline_reason = 'heartbeat-timeout'` so an operator can tell a swept agent from a manually downed one.

**Files:**
- Modify: `crates/agentd-store/src/agent_repo.rs` (add `mark_stale_agents_offline`)
- Modify: `crates/agentd-bin/src/daemon.rs:113-124` (`worker_fleet_tick`) and add `AGENT_HEARTBEAT_TIMEOUT_SECS`
- Test: `crates/agentd-store/tests/agent_registry.rs`
- Test: `crates/agentd-bin/tests/agent_registry_recovery.rs` (create)

**Interfaces:**
- Consumes: `agentd_store::agent_repo::{heartbeat_agent, HeartbeatAgent, get_agent, mark_agent_offline, OfflineAgent}` (existing).
- Produces:
  - `agentd_store::agent_repo::mark_stale_agents_offline(pool: &SqlitePool, cutoff: i64) -> Result<u64, StoreError>` — returns the number of agents transitioned.
  - `agentd_bin::daemon::AGENT_HEARTBEAT_TIMEOUT_SECS: i64` (value `300`).

- [ ] **Step 1: Write the failing store test**

Append to `crates/agentd-store/tests/agent_registry.rs`:

```rust
#[tokio::test]
async fn stale_agents_are_swept_offline_without_losing_their_runtime_target() {
    let (store, _dir) = open_temp().await;

    for name in ["fresh-agent", "stale-agent"] {
        agent_repo::heartbeat_agent(
            store.pool(),
            name,
            agent_repo::HeartbeatAgent {
                server: Some(text("local")),
                tmux_target: Some(format!("{name}:0.0")),
                workspace_path: None,
            },
        )
        .await
        .expect("heartbeat");
    }

    // Backdate one agent's heartbeat past the cutoff.
    sqlx::query("UPDATE agents SET last_seen_at = 100 WHERE name = ?")
        .bind("stale-agent")
        .execute(store.pool())
        .await
        .expect("backdate");

    let swept = agent_repo::mark_stale_agents_offline(store.pool(), 500)
        .await
        .expect("sweep");
    assert_eq!(swept, 1);

    let stale = agent_repo::get_agent(store.pool(), "stale-agent")
        .await
        .expect("get stale")
        .expect("agent exists");
    assert_eq!(stale.status, "offline");
    assert_eq!(stale.offline_reason.as_deref(), Some("heartbeat-timeout"));
    // Preserved so `rebind` can still reattach a surviving pane.
    assert_eq!(stale.tmux_target.as_deref(), Some("stale-agent:0.0"));

    let fresh = agent_repo::get_agent(store.pool(), "fresh-agent")
        .await
        .expect("get fresh")
        .expect("agent exists");
    assert_eq!(fresh.status, "online");

    // The sweep is idempotent: an already-offline agent is not re-counted.
    let swept_again = agent_repo::mark_stale_agents_offline(store.pool(), 500)
        .await
        .expect("second sweep");
    assert_eq!(swept_again, 0);
}
```

Add `use sqlx::Executor as _;` only if the compiler asks for it; `sqlx::query(...).execute(store.pool())` needs no extra trait import in this crate's test setup.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentd-store --test agent_registry stale_agents_are_swept_offline`
Expected: FAIL to compile with `cannot find function mark_stale_agents_offline in module agent_repo`.

- [ ] **Step 3: Implement the sweep**

Insert into `crates/agentd-store/src/agent_repo.rs` immediately after `mark_agent_offline` (after line 317):

```rust
/// Fence agents whose heartbeat is older than `cutoff`. Mirrors
/// [`crate::worker_repo::mark_stale_workers_offline`] for the agent registry:
/// without it a silently dead agent stays `online` forever and keeps being
/// advertised to schedulers, operators, and Matrix commands.
///
/// `tmux_target` is deliberately preserved so p234's `rebind` can still
/// reattach a pane that outlived the heartbeat, and the reason is
/// `heartbeat-timeout` so a swept agent is distinguishable from one an
/// operator took down.
///
/// # Errors
/// [`StoreError::Conflict`] if a selected row changed concurrently,
/// [`StoreError::Sqlx`] on a database failure.
pub async fn mark_stale_agents_offline(
    pool: &SqlitePool,
    cutoff: i64,
) -> Result<u64, StoreError> {
    let mut connection = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *connection).await?;
    let result = sweep_stale_agents(&mut connection, cutoff).await;
    match result {
        Ok(count) => {
            sqlx::query("COMMIT").execute(&mut *connection).await?;
            Ok(count)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

async fn sweep_stale_agents(
    connection: &mut sqlx::SqliteConnection,
    cutoff: i64,
) -> Result<u64, StoreError> {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM agents \
         WHERE name IS NOT NULL AND status = 'online' \
         AND last_seen_at IS NOT NULL AND last_seen_at < ?",
    )
    .bind(cutoff)
    .fetch_all(&mut *connection)
    .await?;

    let now = now_unix();
    let mut swept = 0_u64;
    for name in names {
        // The status guard is defensive: BEGIN IMMEDIATE already serializes
        // writers, but a per-row guard keeps the count honest if this ever
        // runs outside the transaction.
        let updated = sqlx::query(
            "UPDATE agents SET status = 'offline', offline_reason = 'heartbeat-timeout', \
             updated_at = ? WHERE name = ? AND status = 'online'",
        )
        .bind(now)
        .bind(&name)
        .execute(&mut *connection)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::Conflict(format!(
                "agent '{name}' changed during the stale sweep"
            )));
        }
        swept += 1;
    }
    Ok(swept)
}
```

- [ ] **Step 4: Run the store test to verify it passes**

Run: `cargo test -p agentd-store --test agent_registry stale_agents_are_swept_offline`
Expected: PASS.

- [ ] **Step 5: Write the failing daemon-tick test**

Create `crates/agentd-bin/tests/agent_registry_recovery.rs`:

```rust
//! The daemon maintenance tick must fence agents that stopped heartbeating,
//! not only workers. Without this an agent that dies silently stays `online`
//! in the registry forever.

use agentd_bin::daemon::{AGENT_HEARTBEAT_TIMEOUT_SECS, agent_registry_tick};
use agentd_store::{SqliteStore, agent_repo};

#[tokio::test]
async fn the_maintenance_tick_fences_agents_that_stopped_heartbeating() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");

    agent_repo::heartbeat_agent(
        store.pool(),
        "codex-dev",
        agent_repo::HeartbeatAgent {
            server: Some("local".to_string()),
            tmux_target: Some("codex-dev:0.0".to_string()),
            workspace_path: None,
        },
    )
    .await
    .expect("heartbeat");

    let observed_at = 1_000_000_i64;
    sqlx::query("UPDATE agents SET last_seen_at = ? WHERE name = ?")
        .bind(observed_at - AGENT_HEARTBEAT_TIMEOUT_SECS - 1)
        .bind("codex-dev")
        .execute(store.pool())
        .await
        .expect("backdate");

    let swept = agent_registry_tick(store.pool(), observed_at).await;
    assert_eq!(swept, 1);

    let agent = agent_repo::get_agent(store.pool(), "codex-dev")
        .await
        .expect("get")
        .expect("agent exists");
    assert_eq!(agent.status, "offline");
    assert_eq!(agent.offline_reason.as_deref(), Some("heartbeat-timeout"));

    // An agent inside the window is left alone.
    agent_repo::heartbeat_agent(
        store.pool(),
        "codex-dev",
        agent_repo::HeartbeatAgent {
            server: None,
            tmux_target: None,
            workspace_path: None,
        },
    )
    .await
    .expect("re-heartbeat");
    assert_eq!(agent_registry_tick(store.pool(), observed_at).await, 0);
}
```

- [ ] **Step 6: Run the daemon test to verify it fails**

Run: `cargo test -p agentd-bin --test agent_registry_recovery`
Expected: FAIL to compile with `unresolved imports agentd_bin::daemon::AGENT_HEARTBEAT_TIMEOUT_SECS, agentd_bin::daemon::agent_registry_tick`.

- [ ] **Step 7: Wire the sweep into the maintenance tick**

In `crates/agentd-bin/src/daemon.rs`, replace `worker_fleet_tick` (lines 113-124) with:

```rust
/// Seconds an agent may go without a heartbeat before the registry fences it.
/// Longer than the worker fleet's 30s window: an agent's heartbeat is driven
/// by interactive runtime activity, not a tight supervisor loop.
pub const AGENT_HEARTBEAT_TIMEOUT_SECS: i64 = 300;

/// Fence agents whose heartbeat is older than [`AGENT_HEARTBEAT_TIMEOUT_SECS`].
/// Returns how many agents were transitioned; store failures are swallowed so
/// one bad tick never stops the maintenance loop.
pub async fn agent_registry_tick(pool: &sqlx::SqlitePool, observed_at: i64) -> u64 {
    let cutoff = observed_at.saturating_sub(AGENT_HEARTBEAT_TIMEOUT_SECS);
    agentd_store::agent_repo::mark_stale_agents_offline(pool, cutoff)
        .await
        .unwrap_or(0)
}

/// Run one durable worker-fleet maintenance tick.
pub async fn worker_fleet_tick(
    fleet: &dyn WorkerFleetPort,
    recovery_registry: &NativeRecoveryRegistry,
    native_worker: &AgentdWorker,
    scheduler: &agentd_store::durable_scheduler::SqliteDurableScheduler,
    observed_at: i64,
) {
    let _ = fleet.recover_offline(observed_at - 30).await;
    let _ = fleet.expire_due(observed_at).await;
    let _ = scheduler.reconcile(observed_at).await;
    let _ = agent_registry_tick(native_worker.store().pool(), observed_at).await;
    let _ = recovery_registry.recover_one(native_worker).await;
}
```

- [ ] **Step 8: Run the daemon test to verify it passes**

Run: `cargo test -p agentd-bin --test agent_registry_recovery`
Expected: PASS (1 test).

- [ ] **Step 9: Run the task gate**

Run: `cargo test -p agentd-store --test agent_registry && cargo test -p agentd-bin --test agent_registry_recovery && cargo test -p agentd-bin --test worker_main`
Expected: PASS.

Run: `cargo nextest run -p agentd-store`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/agentd-store/src/agent_repo.rs \
        crates/agentd-store/tests/agent_registry.rs \
        crates/agentd-bin/src/daemon.rs \
        crates/agentd-bin/tests/agent_registry_recovery.rs
git commit -m "feat(registry): fence agents that stop heartbeating in the maintenance tick"
```

---

### Task 5: Migration 0025 and the durable project↔room↔repo binding store

**Why a new migration and not an extension of 0022.** Migration `0022_matrix_room_project_binding.sql` added exactly one nullable column, `matrix_bridge_rooms.project_id`, on a table whose primary key is the Matrix room and whose purpose is bridge trust state — it carries no repository at all. The only repository-ish columns that exist, `projects.repo_path` and `projects.github_repo`, are explicitly classified **non-authoritative** ("locator") by the P266 contract, alongside `projects.matrix_room_id` ("transport hint") — see `crates/agentctl/tests/enterprise_project_authority_contract.rs:311-341`, which asserts the document states *"None of these base fields is a project authority record, canonical `RepositoryRef`, or canonical `ProjectRoomBindingRef`."* Widening either of those tables would make a non-authoritative row look authoritative. So: a new table in a new migration, `0025`, is the correct home for a first-class agentd-owned binding.

**Files:**
- Create: `crates/agentd-store/migrations/0025_project_room_repo_binding.sql`
- Create: `crates/agentd-core/src/ports/project_binding.rs`
- Create: `crates/agentd-store/src/project_binding_repo.rs`
- Create: `crates/agentd-store/tests/project_binding.rs`
- Modify: `crates/agentd-core/src/ports/mod.rs:6-19` (module) and the `pub use` block (re-export)
- Modify: `crates/agentd-store/src/lib.rs:39-40` (module, alphabetically before `project_repo`)
- Modify: `crates/agentd-store/tests/migration.rs` (every `"24"` → `"25"`)
- Modify: `crates/agentd-store/tests/operational_doctor.rs:23` (`24` → `25`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces (all `pub`, re-exported from `agentd_core::ports`):
  - `ProjectRoomRepoBinding { project_id: String, room_id: String, repository_id: String, repository_url: String, default_branch: String, record_version: i64, created_at: i64, updated_at: i64 }` (derives `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`)
  - `ProjectRoomRepoBindingRequest { project_id: String, room_id: String, repository_id: String, repository_url: String, default_branch: String }` (derives `Debug, Clone, PartialEq, Eq, Serialize, Deserialize`)
  - `ProjectBindingError { Invalid(String), NotFound(String), Conflict(String), Unavailable(String) }` (derives `Debug, thiserror::Error, Clone, PartialEq, Eq`)
  - `#[async_trait] trait ProjectBindingPort: Send + Sync` with `put_binding(&self, request: &ProjectRoomRepoBindingRequest) -> Result<ProjectRoomRepoBinding, ProjectBindingError>`, `get_binding_for_project(&self, project_id: &str) -> Result<ProjectRoomRepoBinding, ProjectBindingError>`, `get_binding_for_room(&self, room_id: &str) -> Result<ProjectRoomRepoBinding, ProjectBindingError>`
  - `agentd_store::project_binding_repo::SqliteProjectBindingStore::new(pool: SqlitePool) -> Self`

- [ ] **Step 1: Write the failing store test**

Create `crates/agentd-store/tests/project_binding.rs`:

```rust
//! The project ↔ room ↔ repository binding is an agentd-owned durable record,
//! not a projection of the non-authoritative `projects` locator columns.

use agentd_core::ports::{
    ProjectBindingError, ProjectBindingPort, ProjectRoomRepoBindingRequest,
};
use agentd_store::SqliteStore;
use agentd_store::project_binding_repo::SqliteProjectBindingStore;

async fn open_temp() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect + migrate");
    (store, dir)
}

fn request(project: &str, room: &str, repo: &str) -> ProjectRoomRepoBindingRequest {
    ProjectRoomRepoBindingRequest {
        project_id: project.to_string(),
        room_id: room.to_string(),
        repository_id: repo.to_string(),
        repository_url: format!("https://github.com/example/{repo}.git"),
        default_branch: "main".to_string(),
    }
}

#[tokio::test]
async fn put_binding_creates_then_updates_in_place() {
    let (store, _dir) = open_temp().await;
    let bindings = SqliteProjectBindingStore::new(store.pool().clone());

    let created = bindings
        .put_binding(&request("proj-1", "!room-1:example.org", "agentd"))
        .await
        .expect("create");
    assert_eq!(created.project_id, "proj-1");
    assert_eq!(created.room_id, "!room-1:example.org");
    assert_eq!(created.repository_id, "agentd");
    assert_eq!(created.default_branch, "main");
    assert_eq!(created.record_version, 1);

    let mut moved = request("proj-1", "!room-2:example.org", "agentd");
    moved.default_branch = "trunk".to_string();
    let updated = bindings.put_binding(&moved).await.expect("update");
    assert_eq!(updated.room_id, "!room-2:example.org");
    assert_eq!(updated.default_branch, "trunk");
    assert_eq!(updated.record_version, 2);
    assert_eq!(updated.created_at, created.created_at);

    let by_project = bindings
        .get_binding_for_project("proj-1")
        .await
        .expect("lookup by project");
    assert_eq!(by_project, updated);

    let by_room = bindings
        .get_binding_for_room("!room-2:example.org")
        .await
        .expect("lookup by room");
    assert_eq!(by_room, updated);
}

#[tokio::test]
async fn a_room_cannot_be_bound_to_two_projects() {
    let (store, _dir) = open_temp().await;
    let bindings = SqliteProjectBindingStore::new(store.pool().clone());
    bindings
        .put_binding(&request("proj-1", "!shared:example.org", "agentd"))
        .await
        .expect("first binding");

    let error = bindings
        .put_binding(&request("proj-2", "!shared:example.org", "other"))
        .await
        .expect_err("a room binds to at most one project");
    assert!(matches!(error, ProjectBindingError::Conflict(_)));

    // The first binding is untouched.
    let kept = bindings
        .get_binding_for_room("!shared:example.org")
        .await
        .expect("lookup");
    assert_eq!(kept.project_id, "proj-1");
    assert_eq!(kept.record_version, 1);
}

#[tokio::test]
async fn missing_and_blank_lookups_are_classified() {
    let (store, _dir) = open_temp().await;
    let bindings = SqliteProjectBindingStore::new(store.pool().clone());

    assert!(matches!(
        bindings.get_binding_for_project("ghost").await,
        Err(ProjectBindingError::NotFound(_))
    ));
    assert!(matches!(
        bindings.get_binding_for_room("!ghost:example.org").await,
        Err(ProjectBindingError::NotFound(_))
    ));
    assert!(matches!(
        bindings.get_binding_for_project("  ").await,
        Err(ProjectBindingError::Invalid(_))
    ));
    assert!(matches!(
        bindings.put_binding(&request("proj-1", "  ", "agentd")).await,
        Err(ProjectBindingError::Invalid(_))
    ));
    assert!(matches!(
        bindings.put_binding(&request("proj-1", "!r:example.org", " ")).await,
        Err(ProjectBindingError::Invalid(_))
    ));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentd-store --test project_binding`
Expected: FAIL to compile with `unresolved import agentd_store::project_binding_repo`.

- [ ] **Step 3: Add the migration and sweep the version asserts**

Create `crates/agentd-store/migrations/0025_project_room_repo_binding.sql`:

```sql
-- M3 Plan A: the project ↔ room ↔ repository binding as a durable,
-- agentd-owned first-class record. `projects.repo_path`/`github_repo`
-- ("locator") and `projects.matrix_room_id` ("transport hint") are classified
-- non-authoritative by the P266 contract, and 0022's
-- `matrix_bridge_rooms.project_id` carries no repository — so neither can host
-- this. No FK to `projects`: this table is the authority and a binding may be
-- declared before the legacy `projects` import alias row exists.
CREATE TABLE project_room_repo_bindings (
    project_id     TEXT PRIMARY KEY CHECK (length(trim(project_id)) > 0),
    room_id        TEXT NOT NULL UNIQUE CHECK (length(trim(room_id)) > 0),
    repository_id  TEXT NOT NULL CHECK (length(trim(repository_id)) > 0),
    repository_url TEXT NOT NULL CHECK (length(trim(repository_url)) > 0),
    default_branch TEXT NOT NULL CHECK (length(trim(default_branch)) > 0),
    record_version INTEGER NOT NULL DEFAULT 1 CHECK (record_version > 0),
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

CREATE INDEX idx_project_room_repo_bindings_repository
    ON project_room_repo_bindings(repository_id);

UPDATE schema_meta SET value = '25' WHERE key = 'version';
```

Then sweep the version asserts in the **same commit**:

```bash
sed -i '' 's/assert_eq!(version, "24")/assert_eq!(version, "25")/g' crates/agentd-store/tests/migration.rs
sed -i '' 's/assert_eq!(report.schema_version, 24)/assert_eq!(report.schema_version, 25)/' crates/agentd-store/tests/operational_doctor.rs
grep -n '"24"' crates/agentd-store/tests/migration.rs
```

Expected: the final `grep` prints nothing. If it prints a line, that assert uses a different spelling — fix it by hand.

- [ ] **Step 4: Run the migration gates**

Run: `cargo test -p agentd-store --test migration && cargo test -p agentd-store --test operational_doctor`
Expected: PASS.

- [ ] **Step 5: Define the port**

Create `crates/agentd-core/src/ports/project_binding.rs`:

```rust
//! The durable project ↔ room ↔ repository binding boundary. This record is
//! agentd-owned: it is the answer to "which repository and which room does
//! this project execute against", independent of any external authority.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A stored binding. `record_version` increments on every accepted write, so
/// an operator can tell a re-binding from the original declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRoomRepoBinding {
    pub project_id: String,
    pub room_id: String,
    pub repository_id: String,
    pub repository_url: String,
    pub default_branch: String,
    pub record_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Operator-supplied binding declaration. Writing the same project twice
/// re-binds it; writing a room that another project already holds is a
/// conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRoomRepoBindingRequest {
    pub project_id: String,
    pub room_id: String,
    pub repository_id: String,
    pub repository_url: String,
    pub default_branch: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ProjectBindingError {
    #[error("project binding input is invalid: {0}")]
    Invalid(String),
    #[error("project binding not found: {0}")]
    NotFound(String),
    #[error("project binding conflict: {0}")]
    Conflict(String),
    #[error("project binding store is unavailable: {0}")]
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait ProjectBindingPort: Send + Sync {
    /// Declare or re-declare the binding for one project.
    async fn put_binding(
        &self,
        request: &ProjectRoomRepoBindingRequest,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError>;

    /// Read the binding a project holds.
    async fn get_binding_for_project(
        &self,
        project_id: &str,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError>;

    /// Read the binding a Matrix room is covered by.
    async fn get_binding_for_room(
        &self,
        room_id: &str,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError>;
}
```

In `crates/agentd-core/src/ports/mod.rs`, add the module in alphabetical position (between `pub mod native_runtime;` on line 13 and `pub mod project_authority;` on line 14):

```rust
pub mod project_binding;
```

and add the re-export next to the other `pub use` lines (after the `project_authority` re-export at line 47-50):

```rust
pub use project_binding::{
    ProjectBindingError, ProjectBindingPort, ProjectRoomRepoBinding,
    ProjectRoomRepoBindingRequest,
};
```

- [ ] **Step 6: Implement the store**

Create `crates/agentd-store/src/project_binding_repo.rs`:

```rust
//! SQLite implementation of [`ProjectBindingPort`] over
//! `project_room_repo_bindings` (migration 0025).

use agentd_core::ports::{
    ProjectBindingError, ProjectBindingPort, ProjectRoomRepoBinding,
    ProjectRoomRepoBindingRequest,
};
use sqlx::{Row, SqlitePool};

use crate::util::now_unix;

#[derive(Debug, Clone)]
pub struct SqliteProjectBindingStore {
    pool: SqlitePool,
}

impl SqliteProjectBindingStore {
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

fn required(value: &str, field: &str) -> Result<String, ProjectBindingError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ProjectBindingError::Invalid(format!("{field} is required")));
    }
    Ok(trimmed.to_string())
}

fn unavailable(error: &sqlx::Error) -> ProjectBindingError {
    ProjectBindingError::Unavailable(error.to_string())
}

fn row_to_binding(row: &sqlx::sqlite::SqliteRow) -> ProjectRoomRepoBinding {
    ProjectRoomRepoBinding {
        project_id: row.get("project_id"),
        room_id: row.get("room_id"),
        repository_id: row.get("repository_id"),
        repository_url: row.get("repository_url"),
        default_branch: row.get("default_branch"),
        record_version: row.get("record_version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

const SELECT_BINDING: &str = "SELECT project_id, room_id, repository_id, repository_url, \
     default_branch, record_version, created_at, updated_at FROM project_room_repo_bindings";

struct BindingColumns<'a> {
    project_id: &'a str,
    room_id: &'a str,
    repository_id: &'a str,
    repository_url: &'a str,
    default_branch: &'a str,
}

/// The write half of `put_binding`, run inside the caller's `BEGIN IMMEDIATE`.
async fn put_binding_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    columns: BindingColumns<'_>,
    now: i64,
) -> Result<ProjectRoomRepoBinding, ProjectBindingError> {
    let BindingColumns {
        project_id,
        room_id,
        repository_id,
        repository_url,
        default_branch,
    } = columns;

    let room_owner: Option<String> =
        sqlx::query_scalar("SELECT project_id FROM project_room_repo_bindings WHERE room_id = ?")
            .bind(room_id)
            .fetch_optional(&mut *connection)
            .await
            .map_err(|e| unavailable(&e))?;
    if room_owner.is_some_and(|owner| owner != project_id) {
        return Err(ProjectBindingError::Conflict(format!(
            "room '{room_id}' is already bound to another project"
        )));
    }

    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT record_version FROM project_room_repo_bindings WHERE project_id = ?",
    )
    .bind(project_id)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|e| unavailable(&e))?;

    let affected = if existing.is_some() {
        sqlx::query(
            "UPDATE project_room_repo_bindings SET \
              room_id = ?, repository_id = ?, repository_url = ?, default_branch = ?, \
              record_version = record_version + 1, updated_at = ? \
             WHERE project_id = ?",
        )
        .bind(room_id)
        .bind(repository_id)
        .bind(repository_url)
        .bind(default_branch)
        .bind(now)
        .bind(project_id)
        .execute(&mut *connection)
        .await
        .map_err(|e| unavailable(&e))?
    } else {
        sqlx::query(
            "INSERT INTO project_room_repo_bindings \
             (project_id, room_id, repository_id, repository_url, default_branch, \
              record_version, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(project_id)
        .bind(room_id)
        .bind(repository_id)
        .bind(repository_url)
        .bind(default_branch)
        .bind(now)
        .bind(now)
        .execute(&mut *connection)
        .await
        .map_err(|e| unavailable(&e))?
    };
    if affected.rows_affected() != 1 {
        return Err(ProjectBindingError::Conflict(format!(
            "binding for project '{project_id}' changed concurrently"
        )));
    }

    let row = sqlx::query(&format!("{SELECT_BINDING} WHERE project_id = ?"))
        .bind(project_id)
        .fetch_optional(&mut *connection)
        .await
        .map_err(|e| unavailable(&e))?
        .ok_or_else(|| {
            ProjectBindingError::Conflict(format!(
                "binding for project '{project_id}' vanished mid-write"
            ))
        })?;
    Ok(row_to_binding(&row))
}

#[async_trait::async_trait]
impl ProjectBindingPort for SqliteProjectBindingStore {
    async fn put_binding(
        &self,
        request: &ProjectRoomRepoBindingRequest,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError> {
        let project_id = required(&request.project_id, "project_id")?;
        let room_id = required(&request.room_id, "room_id")?;
        let repository_id = required(&request.repository_id, "repository_id")?;
        let repository_url = required(&request.repository_url, "repository_url")?;
        let default_branch = required(&request.default_branch, "default_branch")?;
        let now = now_unix();

        let mut connection = self.pool.acquire().await.map_err(|e| unavailable(&e))?;
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut *connection)
            .await
            .map_err(|e| unavailable(&e))?;

        let result = put_binding_in_transaction(
            &mut connection,
            BindingColumns {
                project_id: &project_id,
                room_id: &room_id,
                repository_id: &repository_id,
                repository_url: &repository_url,
                default_branch: &default_branch,
            },
            now,
        )
        .await;

        match result {
            Ok(binding) => {
                sqlx::query("COMMIT")
                    .execute(&mut *connection)
                    .await
                    .map_err(|e| unavailable(&e))?;
                Ok(binding)
            }
            Err(error) => {
                let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
                Err(error)
            }
        }
    }

    async fn get_binding_for_project(
        &self,
        project_id: &str,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError> {
        let project_id = required(project_id, "project_id")?;
        sqlx::query(&format!("{SELECT_BINDING} WHERE project_id = ?"))
            .bind(&project_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| unavailable(&e))?
            .as_ref()
            .map(row_to_binding)
            .ok_or_else(|| {
                ProjectBindingError::NotFound(format!("no binding for project '{project_id}'"))
            })
    }

    async fn get_binding_for_room(
        &self,
        room_id: &str,
    ) -> Result<ProjectRoomRepoBinding, ProjectBindingError> {
        let room_id = required(room_id, "room_id")?;
        sqlx::query(&format!("{SELECT_BINDING} WHERE room_id = ?"))
            .bind(&room_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| unavailable(&e))?
            .as_ref()
            .map(row_to_binding)
            .ok_or_else(|| {
                ProjectBindingError::NotFound(format!("no binding for room '{room_id}'"))
            })
    }
}
```

In `crates/agentd-store/src/lib.rs`, add the module alphabetically (between `pub mod project_authority_repo;` on line 39 and `pub mod project_repo;` on line 40):

```rust
pub mod project_binding_repo;
```

- [ ] **Step 7: Run the store test to verify it passes**

Run: `cargo test -p agentd-store --test project_binding`
Expected: PASS (3 tests).

- [ ] **Step 8: Run the task gate**

Run: `cargo test -p agentd-store --test project_binding && cargo test -p agentd-store --test migration && cargo test -p agentd-store --test operational_doctor && cargo test -p agentd-store --test migration_backcompat && cargo check -p agentd-core --all-targets`
Expected: PASS.

Run: `cargo nextest run -p agentd-store`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/agentd-store/migrations/0025_project_room_repo_binding.sql \
        crates/agentd-store/src/project_binding_repo.rs \
        crates/agentd-store/src/lib.rs \
        crates/agentd-store/tests/project_binding.rs \
        crates/agentd-store/tests/migration.rs \
        crates/agentd-store/tests/operational_doctor.rs \
        crates/agentd-core/src/ports/project_binding.rs \
        crates/agentd-core/src/ports/mod.rs
git commit -m "feat(binding): durable project-room-repo binding record (schema 25)"
```

---

### Task 6: Operator HTTP API for the project↔room↔repo binding

Mount the binding as a mountable router, the same shape `worker_fleet_router` uses, and merge it into the daemon surface. Reuses the `ControlPlaneErrorStatus` trait from Task 1 so the binding endpoints honor the same status convention for free.

**Files:**
- Create: `crates/agentd-surface/src/project_binding_http.rs`
- Modify: `crates/agentd-surface/src/lib.rs` (add `pub mod project_binding_http;` alphabetically after `pub mod native_runtime_http;`)
- Modify: `crates/agentd-surface/src/control_plane_status.rs` (impl for `ProjectBindingError` + unit test)
- Modify: `crates/agentd-bin/src/daemon.rs` (add `daemon_project_binding_router`, merge it in `serve`)
- Test: `crates/agentd-bin/tests/project_binding_http.rs` (create)

**Interfaces:**
- Consumes: `agentd_surface::control_plane_status::ControlPlaneErrorStatus` (Task 1); `agentd_core::ports::{ProjectBindingPort, ProjectBindingError, ProjectRoomRepoBindingRequest}` and `agentd_store::project_binding_repo::SqliteProjectBindingStore` (Task 5).
- Produces:
  - `agentd_surface::project_binding_http::project_binding_router(bindings: Arc<dyn ProjectBindingPort>, auth: AuthConfig) -> Router`
  - `agentd_bin::daemon::daemon_project_binding_router(store: &SqliteStore, token: Option<String>) -> Router`
  - HTTP `PUT /api/projects/:project_id/binding` (body is `ProjectRoomRepoBindingRequest` minus `project_id`, which comes from the path), `GET /api/projects/:project_id/binding`, `GET /api/rooms/:room_id/binding`

- [ ] **Step 1: Write the failing HTTP test**

Create `crates/agentd-bin/tests/project_binding_http.rs`:

```rust
//! The operator-facing binding API. Statuses follow the project convention:
//! Invalid -> 400, NotFound -> 404, Conflict -> 409, Unavailable -> 503.

use agentd_store::SqliteStore;
use agentd_store::project_binding_repo::SqliteProjectBindingStore;
use agentd_surface::http::AuthConfig;
use agentd_surface::project_binding_http::project_binding_router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

async fn app() -> (axum::Router, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let bindings = Arc::new(SqliteProjectBindingStore::new(store.pool().clone()));
    let mut auth = AuthConfig::open();
    auth.api_token = Some("operator-secret".into());
    (project_binding_router(bindings, auth), dir)
}

async fn send(
    app: axum::Router,
    builder: axum::http::request::Builder,
    body: Option<Value>,
) -> axum::http::Response<Body> {
    let builder = builder.header("authorization", "Bearer operator-secret");
    let request = match body {
        Some(value) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&value).expect("json")))
            .expect("request"),
        None => builder.body(Body::empty()).expect("request"),
    };
    app.oneshot(request).await.expect("response")
}

async fn body_json(response: axum::http::Response<Body>) -> Value {
    let bytes = response.into_body().collect().await.expect("body").to_bytes();
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn binding_api_declares_reads_and_classifies_errors() {
    let (app, _dir) = app().await;

    let declare = json!({
        "room_id": "!room-1:example.org",
        "repository_id": "agentd",
        "repository_url": "https://github.com/example/agentd.git",
        "default_branch": "main"
    });
    let response = send(
        app.clone(),
        Request::put("/api/projects/proj-1/binding"),
        Some(declare.clone()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let created = body_json(response).await;
    assert_eq!(created["project_id"], "proj-1");
    assert_eq!(created["room_id"], "!room-1:example.org");
    assert_eq!(created["record_version"], 1);

    let response = send(
        app.clone(),
        Request::get("/api/projects/proj-1/binding"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["repository_id"], "agentd");

    let response = send(
        app.clone(),
        Request::get("/api/rooms/!room-1:example.org/binding"),
        None,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["project_id"], "proj-1");

    // NotFound -> 404.
    let response = send(app.clone(), Request::get("/api/projects/ghost/binding"), None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    // Invalid -> 400.
    let mut blank = declare.clone();
    blank["repository_id"] = json!("   ");
    let response = send(
        app.clone(),
        Request::put("/api/projects/proj-9/binding"),
        Some(blank),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // Conflict -> 409: another project claiming the same room.
    let response = send(
        app.clone(),
        Request::put("/api/projects/proj-2/binding"),
        Some(declare),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn binding_api_requires_the_operator_bearer_token() {
    let (app, _dir) = app().await;
    let response = app
        .oneshot(
            Request::get("/api/projects/proj-1/binding")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentd-bin --test project_binding_http`
Expected: FAIL to compile with `unresolved import agentd_surface::project_binding_http`.

- [ ] **Step 3: Extend the status mapping to the binding error**

In `crates/agentd-surface/src/control_plane_status.rs`, change the import line to:

```rust
use agentd_core::ports::{ProjectBindingError, TaskLeaseError, WorkerFleetError};
```

and append after the `TaskLeaseError` impl:

```rust
impl ControlPlaneErrorStatus for ProjectBindingError {
    fn http_status(&self) -> StatusCode {
        match self {
            Self::Invalid(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}
```

and append inside `mod tests` (also add `ProjectBindingError` to the test module's `use agentd_core::ports::{…}` list):

```rust
    #[test]
    fn project_binding_error_variants_map_to_distinct_statuses() {
        assert_eq!(
            ProjectBindingError::Invalid("bad".into()).http_status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ProjectBindingError::NotFound("gone".into()).http_status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ProjectBindingError::Conflict("taken".into()).http_status(),
            StatusCode::CONFLICT
        );
        assert_eq!(
            ProjectBindingError::Unavailable("busy".into()).http_status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }
```

- [ ] **Step 4: Write the router**

Create `crates/agentd-surface/src/project_binding_http.rs`:

```rust
//! Operator HTTP transport for the durable project ↔ room ↔ repository
//! binding. Mounted independently, like the worker-fleet transport.

use std::sync::Arc;

use agentd_core::ports::{ProjectBindingPort, ProjectRoomRepoBindingRequest};
use axum::{
    Json, Router,
    extract::{Path as AxumPath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
};
use serde::Deserialize;
use serde_json::json;

use crate::control_plane_status::ControlPlaneErrorStatus;
use crate::http::AuthConfig;

#[derive(Clone)]
pub struct ProjectBindingHttpState {
    pub bindings: Arc<dyn ProjectBindingPort>,
    pub auth: AuthConfig,
}

impl std::fmt::Debug for ProjectBindingHttpState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ProjectBindingHttpState")
            .finish_non_exhaustive()
    }
}

/// Body of `PUT /api/projects/:project_id/binding`. The project id comes from
/// the path, so the body never contradicts the URL.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectBindingBody {
    pub room_id: String,
    pub repository_id: String,
    pub repository_url: String,
    pub default_branch: String,
}

/// Build the independently mountable project-binding transport.
pub fn project_binding_router(
    bindings: Arc<dyn ProjectBindingPort>,
    auth: AuthConfig,
) -> Router {
    let state = ProjectBindingHttpState { bindings, auth };
    Router::new()
        .route(
            "/api/projects/:project_id/binding",
            put(put_binding).get(get_project_binding),
        )
        .route("/api/rooms/:room_id/binding", get(get_room_binding))
        .with_state(state)
}

async fn put_binding(
    State(state): State<ProjectBindingHttpState>,
    AxumPath(project_id): AxumPath<String>,
    headers: HeaderMap,
    Json(body): Json<ProjectBindingBody>,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    let request = ProjectRoomRepoBindingRequest {
        project_id,
        room_id: body.room_id,
        repository_id: body.repository_id,
        repository_url: body.repository_url,
        default_branch: body.default_branch,
    };
    respond(state.bindings.put_binding(&request).await)
}

async fn get_project_binding(
    State(state): State<ProjectBindingHttpState>,
    AxumPath(project_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.bindings.get_binding_for_project(&project_id).await)
}

async fn get_room_binding(
    State(state): State<ProjectBindingHttpState>,
    AxumPath(room_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.bindings.get_binding_for_room(&room_id).await)
}

fn respond<T: serde::Serialize, E: std::fmt::Display + ControlPlaneErrorStatus>(
    result: Result<T, E>,
) -> Response {
    match result {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(error) => (
            error.http_status(),
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

/// Returns the rejection response when the bearer token is missing or wrong.
fn authenticate(auth: &AuthConfig, headers: &HeaderMap) -> Option<Response> {
    let expected = auth
        .api_token
        .as_deref()
        .filter(|token| !token.trim().is_empty())?;
    let valid = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|token| token == expected);
    if valid {
        None
    } else {
        Some(
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error": "operator bearer token required"})),
            )
                .into_response(),
        )
    }
}
```

In `crates/agentd-surface/src/lib.rs`, add the module after `pub mod native_runtime_http;`:

```rust
pub mod project_binding_http;
```

- [ ] **Step 5: Run the HTTP test to verify it passes**

Run: `cargo test -p agentd-bin --test project_binding_http`
Expected: PASS (2 tests).

- [ ] **Step 6: Mount the router in the daemon**

In `crates/agentd-bin/src/daemon.rs`, add next to `daemon_native_runtime_router` (after line 188):

```rust
/// Mount the operator project-binding transport over the daemon's own pool.
#[must_use]
pub fn daemon_project_binding_router(store: &SqliteStore, token: Option<String>) -> Router {
    let bindings = Arc::new(
        agentd_store::project_binding_repo::SqliteProjectBindingStore::new(store.pool().clone()),
    );
    let auth = AuthConfig {
        api_token: token,
        ..AuthConfig::default()
    };
    agentd_surface::project_binding_http::project_binding_router(bindings, auth)
}
```

and in `serve`, extend the existing merge chain (line 1491-1494) so it reads:

```rust
    let app = app.merge(daemon_native_runtime_router(
        &host_store,
        auth.api_token.clone(),
    ));
    let app = app.merge(daemon_project_binding_router(
        &host_store,
        auth.api_token.clone(),
    ));
```

- [ ] **Step 7: Run the task gate**

Run: `cargo test -p agentd-bin --test project_binding_http && cargo test -p agentd-surface --lib control_plane_status && cargo test -p agentd-bin --test daemon_http`
Expected: PASS.

Run: `cargo nextest run -p agentd-bin`
Expected: PASS. `native_runtime_can_terminate_a_running_child` (in `agentd-tmux`, not this package) is a known load-sensitive flake; if any other test fails, fix it before committing.

- [ ] **Step 8: Commit**

```bash
git add crates/agentd-surface/src/project_binding_http.rs \
        crates/agentd-surface/src/control_plane_status.rs \
        crates/agentd-surface/src/lib.rs \
        crates/agentd-bin/src/daemon.rs \
        crates/agentd-bin/tests/project_binding_http.rs
git commit -m "feat(binding): operator API for the project-room-repo binding"
```

---

### Task 7: Parity evidence and the contract-test sweep

Record what M3 Plan A actually delivered in `docs/parity/agent-chat-capability-map.md`, and run every contract test that asserts on the touched rows **in this same task** — Plan A and Plan B both shipped stale contract tests by separating these.

**Decision on statuses: all three rows stay `partial`.** `agent_registry_lifecycle` and `agent_runtime_profiles` still need messaging, dashboard, Matrix/relay, cutover, and token provisioning. `project_room_repo_binding` still needs Specify network integration, authority-backed RBAC/quota enforcement, and cutover — all M4/M5 work, and all explicitly out of Plan A's scope. Three tests hard-assert `partial` on these rows (`parity_cli.rs:474-508`, `parity_cli.rs:507-536`, `parity_cli.rs:1848-1870`, `worktree_reconciliation_contract.rs:124-153`, `enterprise_project_authority_contract.rs:343-366`), and one of them requires the literal phrase *"Specify network and durable pinning integration remain pending"* to survive. **Do not flip any status and do not delete that phrase.** Append evidence sentences only.

**Files:**
- Modify: `docs/parity/agent-chat-capability-map.md` (rows `agent_registry_lifecycle`, `agent_runtime_profiles`, `project_room_repo_binding`)
- Test: `crates/agentctl/tests/parity_cli.rs`, `crates/agentctl/tests/worktree_reconciliation_contract.rs`, `crates/agentctl/tests/enterprise_project_authority_contract.rs` (run, not edited — unless a run proves otherwise)

**Interfaces:**
- Consumes: the delivered behavior from Tasks 1-6.
- Produces: no code interfaces.

- [ ] **Step 1: Confirm the current assertions before editing**

Run: `cargo test -p agentctl --test parity_cli && cargo test -p agentctl --test worktree_reconciliation_contract && cargo test -p agentctl --test enterprise_project_authority_contract`
Expected: PASS (this is the baseline; if anything fails now, it is pre-existing and must be reported before you edit the map).

- [ ] **Step 2: Append evidence to `agent_registry_lifecycle`**

In `docs/parity/agent-chat-capability-map.md`, in the `agent_registry_lifecycle` row's decision cell, insert the following sentence **immediately before** the existing sentence that begins "This remains partial until auth/import hardening…", keeping that sentence and the trailing `| Phase C |` intact:

```
M3 Plan A adds non-destructive roster import/update through `agent_repo::import_agent_profile` (roster fields upsert, runtime profile merges key-by-key, liveness columns untouched) and offline-recovery hardening through `agent_repo::mark_stale_agents_offline`, a `BEGIN IMMEDIATE` heartbeat-timeout sweep run from the daemon maintenance tick that fences silently dead agents as `heartbeat-timeout` while preserving their tmux target for rebind.
```

- [ ] **Step 3: Append evidence to `agent_runtime_profiles`**

In the `agent_runtime_profiles` row, insert the following **immediately before** "This remains partial until import/update/profile-management and auth semantics are covered.", keeping that sentence intact:

```
M3 Plan A adds profile management: `agent_repo::get_agent_profile`/`update_agent_profile` (merge or replace) behind `GET`/`PATCH /api/agents/:name/profile` on `RunHost`, and profile-merging roster import through `agent_repo::import_agent_profile` so a re-import never discards operator-owned profile keys.
```

- [ ] **Step 4: Append evidence to `project_room_repo_binding`**

In the `project_room_repo_binding` row, insert the following **immediately before** "Specify network and durable pinning integration remain pending, together with…", keeping that sentence and its `P266`/`P269` mentions intact:

```
M3 Plan A makes the binding a durable agentd-owned first-class record: migration `0025_project_room_repo_binding.sql` adds `project_room_repo_bindings` (project primary key, unique room, repository id/url/default branch, monotonic `record_version`), `ProjectBindingPort` plus `SqliteProjectBindingStore` implement declare/read with a `BEGIN IMMEDIATE` write and a room-already-bound conflict, and `project_binding_router` exposes `PUT`/`GET /api/projects/:project_id/binding` and `GET /api/rooms/:room_id/binding` under the operator bearer token.
```

- [ ] **Step 5: Run the contract tests**

Run: `cargo test -p agentctl --test parity_cli`
Expected: PASS. If a test fails on a missing keyword, the row lost a required substring during editing — restore it; do **not** relax the assertion.

Run: `cargo test -p agentctl --test worktree_reconciliation_contract`
Expected: PASS (it asserts `project_room_repo_binding` is `partial` and mentions `P269` — both still true).

Run: `cargo test -p agentctl --test enterprise_project_authority_contract`
Expected: PASS (it asserts `| partial |`, `P266`, `P269`, and the literal "Specify network and durable pinning integration remain pending").

- [ ] **Step 6: Verify the table is still well-formed**

Run: `grep -c '^| ' docs/parity/agent-chat-capability-map.md`
Expected: the same count as before your edit — you appended prose inside existing cells, so no row was added or split. If the count changed, a newline leaked into a table cell; rejoin the row onto one line.

- [ ] **Step 7: Run the whole-branch gate**

Run: `cargo nextest run -p agentd-store`
Expected: PASS.

Run: `cargo nextest run -p agentd-bin`
Expected: PASS.

Run: `cargo nextest run -p agentd-surface`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: no warnings.

(Run these sequentially — never two `nextest` invocations at once, and never a multi-package `-p` combo.)

- [ ] **Step 8: Commit**

```bash
git add docs/parity/agent-chat-capability-map.md
git commit -m "docs(parity): record M3 Plan A registry and binding evidence"
```
