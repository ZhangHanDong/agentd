# M1 Remote Native Worker Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A remote agentd worker with no local daemon database and no tmux pulls a task over the authenticated HTTP control plane, runs it natively, uploads its artifact, acknowledges it under the fenced lease, and releases the lease.

**Architecture:** Everything the worker needs already has a daemon-side authority: runtime sessions/attempts (`NativeRuntimeControlPort`), leases (`TaskLeasePort`), worker fleet (`WorkerFleetPort`), artifacts (`ArtifactIndexPort` + `ArtifactObjectStore`). This plan (1) injects `TaskLeasePort` into the native worker so lease ops stop constructing SQLite planes inline, (2) exposes artifact upload/acknowledge over the existing recovery router (which already holds the content store), (3) adds a `session_for_task` lookup to the runtime control port, and (4) composes them into an `agentd worker` CLI loop plus an opt-in real smoke.

**Tech Stack:** Rust (existing workspace), axum (existing), raw-`TcpStream` HTTP clients (existing convention in `worker_fleet_client.rs` / `native_runtime_client.rs`), sqlx/SQLite, tokio, clap.

**Design reference:** `docs/superpowers/specs/2026-07-22-agent-chat-replacement-milestones-design.md` §M1.

## Global Constraints

- No new external dependencies in any `Cargo.toml`.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo nextest run --workspace` must pass after every task.
- Follow existing conventions: surface crates stay store-free; HTTP clients use the raw `TcpStream` pattern from `worker_fleet_client.rs`; only `Unavailable` maps to retryable 5xx (`408|425|429|5xx`), other 4xx are terminal.
- Tests never run real Claude/Codex/tmux/Matrix except the explicitly env-gated smoke in Task 6.
- Auth stays at the operator bearer / agent token boundary (design doc §2).
- Commit messages follow the existing `type(scope): summary` convention and end with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

## File Structure

| File | Role |
|---|---|
| `crates/agentd-bin/src/native_worker.rs` | Inject `Arc<dyn TaskLeasePort>`; all lease ops go through it |
| `crates/agentd-bin/src/daemon.rs` | Recovery router gains artifact `upload`/`acknowledge` routes; `WorkerFleetService` gains the two backing methods |
| `crates/agentd-core/src/ports/native_runtime.rs` | Port gains `session_for_task` |
| `crates/agentd-store/src/native_runtime_control_plane.rs` | SQLite impl of `session_for_task` |
| `crates/agentd-surface/src/native_runtime_http.rs` | Route `POST /api/runtime/native/session/for-task` |
| `crates/agentd-bin/src/native_runtime_client.rs` | Client: `session_for_task`, `upload_artifact`, `acknowledge_artifact` |
| `crates/agentd-bin/src/worker_main.rs` (new) | `run_worker_once` library fn: register → pull → execute → upload → ack → release |
| `crates/agentd-bin/src/cli.rs`, `src/main.rs` | `agentd worker` subcommand |
| `crates/agentd-bin/tests/native_worker.rs` | Task 1 unit test (fake lease port) |
| `crates/agentd-bin/tests/recovery_http.rs` | Task 2 route tests |
| `crates/agentd-bin/tests/native_runtime_client.rs` | Task 3/4 client integration tests |
| `crates/agentd-bin/tests/worker_main.rs` (new) | Task 5 end-to-end worker loop test; Task 6 env-gated real smoke |
| `docs/parity/agent-chat-capability-map.md` | Task 6 row updates |

---

### Task 1: Inject `TaskLeasePort` into the native worker

Today `native_worker.rs` constructs `SqliteTaskLeaseControlPlane::new(self.store.pool().clone())` inline at six sites (validate at `start_secured`, `renew_lease`, `release_lease`, `cancel_lease`, `spawn_lease_renewal`, plus the artifact-listing evidence composition). After this task, every **lease** operation goes through an injected `Arc<dyn TaskLeasePort>`; a remote worker injects `WorkerFleetHttpClient` (which already implements `TaskLeasePort` over `/api/worker-fleet/lease/*`) and gets remote lease ops with no new transport. The artifact-listing site (`list_artifacts_for_run`) is a daemon-side read and stays as-is.

**Files:**
- Modify: `crates/agentd-bin/src/native_worker.rs`
- Test: `crates/agentd-bin/tests/native_worker.rs`

**Interfaces:**
- Consumes: `agentd_core::ports::TaskLeasePort` (exists), `SqliteTaskLeaseControlPlane` (exists), `WorkerFleetHttpClient: TaskLeasePort` (exists).
- Produces: `AgentdWorker::with_control_planes(store, runtime_control: Arc<dyn NativeRuntimeControlPort>, lease_control: Arc<dyn TaskLeasePort>) -> Self`. `AgentdWorker::new` and `with_runtime_control` keep their signatures and default `lease_control` to the SQLite plane. `AgentdWorkerHandle` carries `lease_control: Arc<dyn TaskLeasePort>`; `renew_lease`/`release_lease`/`cancel_lease`/`spawn_lease_renewal` route through it. Task 5 relies on `with_control_planes`.

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-bin/tests/native_worker.rs`:

```rust
#[derive(Debug, Default)]
struct RecordingLeasePort {
    calls: std::sync::Mutex<Vec<&'static str>>,
}

#[async_trait::async_trait]
impl agentd_core::ports::TaskLeasePort for RecordingLeasePort {
    async fn dispatch(
        &self,
        _request: &agentd_core::ports::TaskLeaseDispatchRequest,
    ) -> Result<agentd_core::types::TaskLeaseGrant, agentd_core::ports::TaskLeaseError> {
        Err(agentd_core::ports::TaskLeaseError::Unavailable(
            "dispatch unused".into(),
        ))
    }

    async fn renew(
        &self,
        request: &agentd_core::ports::TaskLeaseRenewRequest,
    ) -> Result<agentd_core::types::TaskLeaseGrant, agentd_core::ports::TaskLeaseError> {
        self.calls.lock().expect("calls lock").push("renew");
        Ok(fake_grant(&request.claim))
    }

    async fn release(
        &self,
        request: &agentd_core::ports::TaskLeaseCloseRequest,
    ) -> Result<agentd_core::types::TaskLeaseGrant, agentd_core::ports::TaskLeaseError> {
        self.calls.lock().expect("calls lock").push("release");
        Ok(fake_grant(&request.claim))
    }

    async fn cancel(
        &self,
        request: &agentd_core::ports::TaskLeaseCloseRequest,
    ) -> Result<agentd_core::types::TaskLeaseGrant, agentd_core::ports::TaskLeaseError> {
        self.calls.lock().expect("calls lock").push("cancel");
        Ok(fake_grant(&request.claim))
    }

    async fn validate_claim(
        &self,
        claim: &agentd_core::types::TaskLeaseClaim,
        _observed_at: i64,
    ) -> Result<agentd_core::types::TaskLeaseGrant, agentd_core::ports::TaskLeaseError> {
        self.calls.lock().expect("calls lock").push("validate");
        Ok(fake_grant(claim))
    }

    async fn expire_due(
        &self,
        _observed_at: i64,
    ) -> Result<u64, agentd_core::ports::TaskLeaseError> {
        Ok(0)
    }
}

fn fake_grant(claim: &agentd_core::types::TaskLeaseClaim) -> agentd_core::types::TaskLeaseGrant {
    agentd_core::types::TaskLeaseGrant {
        lease_id: claim.lease_id.clone(),
        execution_task_id: claim.execution_task_id.clone(),
        worker_incarnation_id: claim.worker_incarnation_id.clone(),
        fencing_token: claim.fencing_token,
        status: agentd_core::types::LeaseStatus::Active,
        acquired_at: 1,
        expires_at: 100,
        renewed_at: None,
        terminal_at: None,
        terminal_reason: None,
        record_version: 1,
        execution_spec: None,
        security_scope: None,
        runtime_session_id: None,
    }
}

fn fake_claim() -> agentd_core::types::TaskLeaseClaim {
    agentd_core::types::TaskLeaseClaim {
        lease_id: agentd_core::types::LeaseId::from_string("ls_fake".into()),
        execution_task_id: agentd_core::types::TaskRunId::from_string("tr_fake".into()),
        worker_incarnation_id: agentd_core::types::WorkerIncarnationId::from_string(
            "wi_fake".into(),
        ),
        fencing_token: agentd_core::types::FencingToken::new(1).expect("token"),
    }
}

#[tokio::test]
async fn handle_lease_operations_use_the_injected_port() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    // Reuse the existing fixture seeding from this file (run/task/profile/
    // worker/incarnation/session) — copy the same setup as
    // agentd_worker_binds_native_process_to_durable_runtime_state up to and
    // including create_session, binding `session_id` and `incarnation_id`.
    let lease_port = std::sync::Arc::new(RecordingLeasePort::default());
    let runtime_control = std::sync::Arc::new(
        agentd_store::native_runtime_control_plane::SqliteNativeRuntimeControlPlane::new(
            store.pool().clone(),
        ),
    );
    let worker = AgentdWorker::with_control_planes(
        store.clone(),
        runtime_control,
        lease_port.clone(),
    );
    let handle = worker
        .start(
            session_id,
            incarnation_id,
            NativeProcessConfig {
                program: "sh".into(),
                args: vec!["-c".into(), "exit 0".into()],
                ..NativeProcessConfig::default()
            },
        )
        .await
        .expect("start");

    let claim = fake_claim();
    handle.renew_lease(&claim, 10, 100).await.expect("renew");
    handle
        .release_lease(&claim, 11, "test-release")
        .await
        .expect("release");
    handle.wait(Duration::from_secs(5)).await.expect("wait");

    assert_eq!(
        *lease_port.calls.lock().expect("calls lock"),
        vec!["renew", "release"],
        "lease operations must route through the injected TaskLeasePort"
    );
}
```

Note for the implementer: the fixture-seeding comment above means literally copying the ~60 lines of run/task/profile/worker/incarnation/session setup that already exist at the top of `agentd_worker_binds_native_process_to_durable_runtime_state` in this same file, ending with a bound `session_id` and `incarnation_id`. If the file already has a shared `fixture()` helper, use it instead.

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentd-bin --test native_worker handle_lease_operations_use_the_injected_port`
Expected: FAIL to compile with `no function or associated item named 'with_control_planes'`.

- [ ] **Step 3: Implement the injection**

In `crates/agentd-bin/src/native_worker.rs`:

3a. Add the field and constructors (`AgentdWorker` currently has `store` + `runtime_control`):

```rust
#[derive(Clone)]
pub struct AgentdWorker {
    store: SqliteStore,
    runtime_control: Arc<dyn NativeRuntimeControlPort>,
    lease_control: Arc<dyn TaskLeasePort>,
}
```

In `impl AgentdWorker`, make `new` and `with_runtime_control` delegate, and add the full constructor:

```rust
#[must_use]
pub fn new(store: SqliteStore) -> Self {
    let runtime_control = Arc::new(
        agentd_store::native_runtime_control_plane::SqliteNativeRuntimeControlPlane::new(
            store.pool().clone(),
        ),
    );
    Self::with_runtime_control(store, runtime_control)
}

#[must_use]
pub fn with_runtime_control(
    store: SqliteStore,
    runtime_control: Arc<dyn NativeRuntimeControlPort>,
) -> Self {
    let lease_control = Arc::new(
        agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane::new(
            store.pool().clone(),
        ),
    );
    Self::with_control_planes(store, runtime_control, lease_control)
}

/// Fully injected constructor for remote workers: runtime session/attempt
/// state and lease fencing both resolve through control-plane ports, never
/// through this process's SQLite store.
#[must_use]
pub fn with_control_planes(
    store: SqliteStore,
    runtime_control: Arc<dyn NativeRuntimeControlPort>,
    lease_control: Arc<dyn TaskLeasePort>,
) -> Self {
    Self {
        store,
        runtime_control,
        lease_control,
    }
}
```

(Keep the existing bodies of `new`/`with_runtime_control` only if they already match; the point is both end up calling `with_control_planes`.)

3b. Add `lease_control: Arc<dyn TaskLeasePort>` to `AgentdWorkerHandle` and pass `self.lease_control.clone()` at both places a handle is constructed (`start_for_task`'s `Ok(AgentdWorkerHandle { ... })` and any other constructor site — search for `AgentdWorkerHandle {`).

3c. Replace the inline SQLite planes in the lease methods. `renew_lease` becomes:

```rust
pub async fn renew_lease(
    &self,
    claim: &TaskLeaseClaim,
    observed_at: i64,
    expires_at: i64,
) -> Result<agentd_core::types::TaskLeaseGrant, NativeWorkerError> {
    self.lease_control
        .renew(&TaskLeaseRenewRequest {
            claim: claim.clone(),
            observed_at,
            expires_at,
        })
        .await
        .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))
}
```

Apply the same mechanical change to `release_lease` (call `self.lease_control.release`) and `cancel_lease` (call `self.lease_control.cancel`), keeping their existing `TaskLeaseCloseRequest` bodies.

3d. `spawn_lease_renewal` currently clones the store and builds a SQLite plane inside the spawned task. Change it to move a cloned port:

```rust
pub fn spawn_lease_renewal(
    &self,
    claim: TaskLeaseClaim,
    interval: Duration,
    lease_duration: Duration,
) -> NativeLeaseRenewal {
    let lease_control = Arc::clone(&self.lease_control);
    let runtime = Arc::clone(&self.runtime);
    let task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval.max(Duration::from_millis(10)));
        ticker.tick().await;
        loop {
            if runtime.is_terminal() {
                return Ok(());
            }
            ticker.tick().await;
            let observed_at = crate::clock::SystemClock.now_unix();
            let expires_at = observed_at.saturating_add(
                i64::try_from(lease_duration.as_secs()).unwrap_or(i64::MAX),
            );
            let renewal = lease_control
                .renew(&TaskLeaseRenewRequest {
                    claim: claim.clone(),
                    observed_at,
                    expires_at,
                })
                .await;
            if let Err(error) = renewal {
                let _ = runtime.terminate();
                return Err(NativeWorkerError::InvalidRecovery(error.to_string()));
            }
        }
    });
    NativeLeaseRenewal { task }
}
```

3e. In `start_secured`, replace the inline `SqliteTaskLeaseControlPlane...validate_claim(...)` with `self.lease_control.validate_claim(&binding.scope.lease_claim, observed_at)`. Note in a comment that `WorkerFleetHttpClient::validate_claim` intentionally returns `Unavailable` (claim validation is daemon-owned), so `start_secured` stays a daemon-side entry point; remote workers use `start_for_task` with fencing enforced at renew/release.

3f. Leave `list_artifacts_for_run`'s evidence composition (the site near the top of `impl AgentdWorker`) unchanged — it is a daemon-side read, called out in the design doc as out of M1's worker path.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p agentd-bin --test native_worker`
Expected: all tests pass, including `handle_lease_operations_use_the_injected_port`.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-bin --all-targets -- -D warnings
cargo test -p agentd-bin
git add crates/agentd-bin/src/native_worker.rs crates/agentd-bin/tests/native_worker.rs
git commit -m "feat(runtime): inject TaskLeasePort into the native worker

Lease renew/release/cancel/renewal-loop and secured-claim validation now
route through an injected TaskLeasePort instead of constructing
SqliteTaskLeaseControlPlane inline, so a remote worker can supply
WorkerFleetHttpClient and run fenced lease operations over HTTP.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Artifact upload and acknowledge over the recovery router

The daemon already holds the content store and evidence control plane inside `WorkerFleetService`; the recovery router already carries bearer auth. Add two routes there (the surface crate must stay store-free, so these live in `agentd-bin`):

- `POST /api/runtime/artifacts/upload` — body is raw bytes (`application/octet-stream`), response is the stored content descriptor.
- `POST /api/runtime/artifacts/acknowledge` — body is a `WorkerArtifactReport` (serde JSON, already derives), response is the `WorkerArtifactAcknowledgement`.

**Files:**
- Modify: `crates/agentd-bin/src/daemon.rs`
- Test: `crates/agentd-bin/tests/recovery_http.rs`

**Interfaces:**
- Consumes: `WorkerFleetService` fields `native_worker` (for the store pool) and `content_store: Arc<dyn ArtifactObjectStore>`; `SqliteExecutionEvidenceControlPlane`, `SqliteTaskLeaseControlPlane` (exist); `WorkerArtifactReport`/`WorkerArtifactAcknowledgement` (serde-ready).
- Produces: the two routes above, plus `WorkerFleetService::store_artifact_bytes(&self, bytes: &[u8]) -> Result<agentd_store::content_store::StoredContent, String>` and `WorkerFleetService::acknowledge_worker_artifact(&self, report: &WorkerArtifactReport) -> Result<WorkerArtifactAcknowledgement, ExecutionEvidenceError>`. Task 3's client calls the routes; Task 3's test reuses the fixture pattern here.

- [ ] **Step 1: Write the failing tests**

Append to `crates/agentd-bin/tests/recovery_http.rs` (it already builds a `recovery_router` app with `WorkerFleetService` and an operator token; follow the existing test's construction):

```rust
#[tokio::test]
async fn recovery_http_uploads_artifact_bytes_content_addressed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("store");
    let fleet = Arc::new(SqliteWorkerFleet::new(store.pool().clone()));
    let artifacts =
        Arc::new(LocalContentStore::new(dir.path().join("artifacts")).expect("content store"));
    let service = Arc::new(WorkerFleetService::new(
        fleet,
        AgentdWorker::new(store),
        artifacts,
    ));
    let app = recovery_router(service, "operator-secret".into());

    let unauthorized = app
        .clone()
        .oneshot(
            Request::post("/api/runtime/artifacts/upload")
                .header("content-type", "application/octet-stream")
                .body(Body::from("transcript bytes"))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let uploaded = app
        .clone()
        .oneshot(
            Request::post("/api/runtime/artifacts/upload")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/octet-stream")
                .body(Body::from("transcript bytes"))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(uploaded.status(), StatusCode::OK);
    let body = uploaded.into_body().collect().await.expect("body").to_bytes();
    let stored: serde_json::Value = serde_json::from_slice(&body).expect("json");
    // sha256 of "transcript bytes"
    assert_eq!(
        stored["content_sha256"],
        "0e2e50e5731d70b4a0b3993c88ea9d5cb9b268aabb0d3ff03cd2f1b1c7e7ecd2"
    );
    assert_eq!(stored["size_bytes"], 16);
    assert!(
        stored["storage_ref"].as_str().is_some_and(|r| !r.is_empty()),
        "{stored}"
    );
}
```

(Compute the real sha in Step 2 — if the literal above is wrong, replace it with the value from the failing assertion output; the test then pins it.)

```rust
#[tokio::test]
async fn recovery_http_acknowledges_worker_artifact_under_fenced_lease() {
    // Fixture: run + task + worker + incarnation + dispatched lease, exactly
    // as crates/agentd-store/tests/enterprise_task_leases.rs seeds them, but
    // through this crate's imports. End with `grant` from
    // SqliteTaskLeaseControlPlane::dispatch and its claim().
    // Then:
    let app = recovery_router(service, "operator-secret".into());
    let report = agentd_core::ports::WorkerArtifactReport {
        claim: grant.claim(),
        observed_at: grant.acquired_at + 1,
        artifact: agentd_core::ports::ExecutionArtifactPublish {
            id: agentd_core::types::ExecutionArtifactId::new(),
            kind: agentd_core::ports::ExecutionArtifactKind::Transcript,
            content_sha256: "a".repeat(64),
            size_bytes: 16,
            media_type: "text/plain".to_string(),
            storage_ref: "local://test".to_string(),
            provenance: serde_json::json!({"source": "test"}),
            links: agentd_core::ports::ExecutionEvidenceLinks {
                execution_run_id: run_id.clone(),
                execution_task_id: Some(task_id.clone()),
                runtime_session_id: None,
                runtime_attempt_id: None,
            },
        },
    };

    let acked = app
        .clone()
        .oneshot(
            Request::post("/api/runtime/artifacts/acknowledge")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&report).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(acked.status(), StatusCode::OK);

    // Duplicate acknowledge with the identical artifact id replays idempotently.
    let replay = app
        .clone()
        .oneshot(
            Request::post("/api/runtime/artifacts/acknowledge")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&report).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(replay.status(), StatusCode::OK);

    // A stale claim (wrong fencing token) is a terminal 409.
    let mut stale = report.clone();
    stale.claim.fencing_token =
        agentd_core::types::FencingToken::new(999).expect("token");
    let rejected = app
        .oneshot(
            Request::post("/api/runtime/artifacts/acknowledge")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&stale).expect("json")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(rejected.status(), StatusCode::CONFLICT);
}
```

Implementer notes: check `ExecutionEvidenceLinks`'s exact field set in
`crates/agentd-core/src/ports/execution_evidence.rs` and
`ExecutionArtifactKind`'s variants before writing the fixture; if
`Transcript` is not a variant, use the first existing kind. The lease fixture
seeding is the `fixture()` from `crates/agentd-store/tests/enterprise_task_leases.rs`
translated to this crate.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p agentd-bin --test recovery_http`
Expected: both new tests FAIL — upload with 404/405 (route missing), acknowledge with 404/405.

- [ ] **Step 3: Implement routes and service methods**

In `crates/agentd-bin/src/daemon.rs`:

3a. Service methods on `impl WorkerFleetService` (it already owns `native_worker` and `content_store`):

```rust
pub fn store_artifact_bytes(
    &self,
    bytes: &[u8],
) -> Result<agentd_store::content_store::StoredContent, String> {
    self.content_store
        .put_bytes(bytes)
        .map_err(|error| error.to_string())
}

pub async fn acknowledge_worker_artifact(
    &self,
    report: &agentd_core::ports::WorkerArtifactReport,
) -> Result<agentd_core::ports::WorkerArtifactAcknowledgement, agentd_core::ports::ExecutionEvidenceError>
{
    let pool = self.native_worker.store().pool().clone();
    let lease_port = SqliteTaskLeaseControlPlane::new(pool.clone());
    let evidence = SqliteExecutionEvidenceControlPlane::new(pool, lease_port);
    use agentd_core::ports::ArtifactIndexPort as _;
    evidence.acknowledge_worker_artifact(report).await
}
```

(`AgentdWorker::store()` is `pub(crate)` — same crate, fine. If `native_worker`
is a private field, these methods live next to the existing ones that already
use it.)

3b. Routes in `recovery_router` (same state, same bearer-check style as
`register_codex_recovery`):

```rust
.route("/api/runtime/artifacts/upload", post(upload_artifact))
.route("/api/runtime/artifacts/acknowledge", post(acknowledge_artifact))
```

```rust
async fn upload_artifact(
    State(state): State<RecoveryApiState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    if let Some(response) = recovery_unauthorized(&state, &headers) {
        return response;
    }
    match state.service.store_artifact_bytes(&body) {
        Ok(stored) => (
            StatusCode::OK,
            Json(json!({
                "storage_ref": stored.storage_ref,
                "content_sha256": stored.sha256,
                "size_bytes": stored.size_bytes,
            })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": error })),
        )
            .into_response(),
    }
}

async fn acknowledge_artifact(
    State(state): State<RecoveryApiState>,
    headers: HeaderMap,
    Json(report): Json<agentd_core::ports::WorkerArtifactReport>,
) -> Response {
    if let Some(response) = recovery_unauthorized(&state, &headers) {
        return response;
    }
    match state.service.acknowledge_worker_artifact(&report).await {
        Ok(acknowledgement) => (StatusCode::OK, Json(acknowledgement)).into_response(),
        Err(error) => {
            use agentd_core::ports::ExecutionEvidenceError as E;
            let status = match &error {
                E::Invalid(_) => StatusCode::BAD_REQUEST,
                E::NotFound(_) => StatusCode::NOT_FOUND,
                E::Conflict(_) | E::LeaseRejected { .. } => StatusCode::CONFLICT,
                E::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            };
            (status, Json(json!({ "error": error.to_string() }))).into_response()
        }
    }
}
```

3c. Extract the repeated bearer check into `fn recovery_unauthorized(state: &RecoveryApiState, headers: &HeaderMap) -> Option<Response>` (same logic as `register_codex_recovery`'s inline check) and use it in the two new handlers. Do not refactor the existing handlers in this task.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agentd-bin --test recovery_http`
Expected: PASS (fix the pinned sha literal from the first failure output if needed).

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-bin --all-targets -- -D warnings
cargo test -p agentd-bin
git add crates/agentd-bin/src/daemon.rs crates/agentd-bin/tests/recovery_http.rs
git commit -m "feat(daemon): expose artifact upload and fenced acknowledge over HTTP

Remote workers can now store content-addressed artifact bytes and submit
the WorkerArtifactReport acknowledgement through the recovery router; the
evidence control plane validates the lease claim and replays duplicates
idempotently.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Client methods — `upload_artifact` and `acknowledge_artifact`

**Files:**
- Modify: `crates/agentd-bin/src/native_runtime_client.rs`
- Test: `crates/agentd-bin/tests/native_runtime_client.rs`

**Interfaces:**
- Consumes: Task 2's routes.
- Produces on `NativeRuntimeHttpClient`:
  - `pub async fn upload_artifact(&self, bytes: Vec<u8>) -> Result<agentd_tmux::native::NativeSpoolRecord, NativeRuntimeControlError>`
  - `pub async fn acknowledge_artifact(&self, report: &WorkerArtifactReport) -> Result<WorkerArtifactAcknowledgement, NativeRuntimeControlError>`
  Task 5's worker loop calls both.

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-bin/tests/native_runtime_client.rs`. First extend the file's `serve_daemon` helper so the served app also mounts the recovery router (artifact routes) — merge, keeping the same bearer token:

```rust
async fn serve_daemon(store: SqliteStore, token: &str) -> String {
    let fleet = std::sync::Arc::new(agentd_store::worker_fleet::SqliteWorkerFleet::new(
        store.pool().clone(),
    ));
    let artifacts = std::sync::Arc::new(
        agentd_store::content_store::LocalContentStore::new(
            std::env::temp_dir().join(format!("agentd-m1-artifacts-{}", std::process::id())),
        )
        .expect("content store"),
    );
    let service = std::sync::Arc::new(agentd_bin::daemon::WorkerFleetService::new(
        fleet,
        agentd_bin::native_worker::AgentdWorker::new(store.clone()),
        artifacts,
    ));
    let app = daemon_native_runtime_router(&store, Some(token.to_string()))
        .merge(agentd_bin::daemon::recovery_router(service, token.to_string()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}
```

Then the test:

```rust
#[tokio::test]
async fn http_adapter_uploads_and_acknowledges_artifact() {
    let fixture = fixture().await;
    // Seed a dispatched lease exactly like Task 2's acknowledge fixture:
    // SqliteTaskLeaseControlPlane::dispatch for fixture.task_id to
    // fixture.incarnation_id, binding `grant`.
    let base_url = serve_daemon(fixture.store.clone(), "worker-secret").await;
    let client = NativeRuntimeHttpClient::new(base_url, "worker-secret").expect("client");

    let stored = client
        .upload_artifact(b"worker output".to_vec())
        .await
        .expect("upload over HTTP");
    assert_eq!(stored.size_bytes, 13);
    assert!(!stored.storage_ref.is_empty());
    assert_eq!(stored.content_sha256.len(), 64);

    let report = agentd_core::ports::WorkerArtifactReport {
        claim: grant.claim(),
        observed_at: grant.acquired_at + 1,
        artifact: agentd_core::ports::ExecutionArtifactPublish {
            id: agentd_core::types::ExecutionArtifactId::new(),
            kind: agentd_core::ports::ExecutionArtifactKind::Transcript,
            content_sha256: stored.content_sha256.clone(),
            size_bytes: stored.size_bytes,
            media_type: "text/plain".to_string(),
            storage_ref: stored.storage_ref.clone(),
            provenance: serde_json::json!({"source": "m1-test"}),
            links: agentd_core::ports::ExecutionEvidenceLinks {
                execution_run_id: fixture.run_id.clone(),
                execution_task_id: Some(fixture.task_id.clone()),
                runtime_session_id: None,
                runtime_attempt_id: None,
            },
        },
    };
    let acknowledgement = client
        .acknowledge_artifact(&report)
        .await
        .expect("acknowledge over HTTP");
    assert_eq!(
        acknowledgement.artifact.content_sha256,
        stored.content_sha256
    );
}
```

(The fixture struct must expose `run_id` — add it to the `Fixture` struct and
its construction if it is not already a field. Same `ExecutionArtifactKind`
caveat as Task 2.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p agentd-bin --test native_runtime_client http_adapter_uploads_and_acknowledges_artifact`
Expected: FAIL to compile — `upload_artifact` not found.

- [ ] **Step 3: Implement the client methods**

In `crates/agentd-bin/src/native_runtime_client.rs`, next to `post_blocking`, add a raw-bytes variant and the two public methods:

```rust
fn post_bytes_blocking<Response: DeserializeOwned>(
    &self,
    path: &str,
    body: &[u8],
) -> Result<Response, NativeRuntimeControlError> {
    let mut stream = TcpStream::connect(&self.authority)
        .map_err(|error| NativeRuntimeControlError::Unavailable(error.to_string()))?;
    stream.set_read_timeout(Some(self.timeout)).ok();
    stream.set_write_timeout(Some(self.timeout)).ok();
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        self.authority,
        body.len(),
        self.auth_proof
    )
    .and_then(|()| stream.write_all(body))
    .map_err(|error| NativeRuntimeControlError::Unavailable(error.to_string()))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| NativeRuntimeControlError::Unavailable(error.to_string()))?;
    let response = String::from_utf8(response)
        .map_err(|_| NativeRuntimeControlError::Unavailable("non-UTF8 response".into()))?;
    let (head, body) = response.split_once("\r\n\r\n").ok_or_else(|| {
        NativeRuntimeControlError::Unavailable("malformed HTTP response".into())
    })?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    if !(200..300).contains(&status) {
        return Err(classify_http_error(status, body));
    }
    serde_json::from_str(body)
        .map_err(|error| NativeRuntimeControlError::Unavailable(error.to_string()))
}

/// Store artifact bytes content-addressed on the daemon.
pub async fn upload_artifact(
    &self,
    bytes: Vec<u8>,
) -> Result<agentd_tmux::native::NativeSpoolRecord, NativeRuntimeControlError> {
    let client = self.clone();
    tokio::task::spawn_blocking(move || {
        client.post_bytes_blocking("/api/runtime/artifacts/upload", &bytes)
    })
    .await
    .map_err(|error| NativeRuntimeControlError::Unavailable(error.to_string()))?
}

/// Acknowledge an uploaded artifact under the fenced lease claim.
pub async fn acknowledge_artifact(
    &self,
    report: &agentd_core::ports::WorkerArtifactReport,
) -> Result<agentd_core::ports::WorkerArtifactAcknowledgement, NativeRuntimeControlError> {
    self.post("/api/runtime/artifacts/acknowledge", report).await
}
```

`NativeSpoolRecord` (`storage_ref`, `content_sha256`, `size_bytes`) matches the
upload response JSON keys, and it derives serde — if it does not, add
`Serialize, Deserialize` to its derive in `crates/agentd-tmux/src/native.rs`.
The `post` generic requires `&'static str` paths — the acknowledge path literal
satisfies it; `post_bytes_blocking` takes `&str` so no constraint issue.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p agentd-bin --test native_runtime_client`
Expected: all pass, including the pre-existing lifecycle/recovery tests against the extended `serve_daemon`.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-bin -p agentd-tmux --all-targets -- -D warnings
cargo test -p agentd-bin
git add crates/agentd-bin/src/native_runtime_client.rs crates/agentd-bin/tests/native_runtime_client.rs crates/agentd-tmux/src/native.rs
git commit -m "feat(runtime): client-side artifact upload and acknowledge

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: `session_for_task` on the runtime control port

A pulled grant names an `execution_task_id`, but the worker needs the runtime session the daemon bound to that task. Add a task-keyed session lookup across port → SQLite adapter → HTTP route → client.

**Files:**
- Modify: `crates/agentd-core/src/ports/native_runtime.rs`, `crates/agentd-core/src/ports/mod.rs` (re-export unchanged — `NativeRuntimeSessionView` already exported)
- Modify: `crates/agentd-store/src/native_runtime_control_plane.rs`
- Modify: `crates/agentd-surface/src/native_runtime_http.rs`
- Modify: `crates/agentd-bin/src/native_runtime_client.rs`
- Modify: `crates/agentd-surface/tests/native_runtime_http.rs` (fake port impl)
- Test: `crates/agentd-store/tests/native_runtime_control_plane.rs`, `crates/agentd-bin/tests/native_runtime_client.rs`

**Interfaces:**
- Produces: trait method `async fn session_for_task(&self, task_id: &TaskRunId) -> Result<Option<NativeRuntimeSessionView>, NativeRuntimeControlError>`; route `POST /api/runtime/native/session/for-task` with body `{"task_id": ...}`; client method of the same name. Task 5's loop calls the client method.

- [ ] **Step 1: Write the failing store test**

Append to `crates/agentd-store/tests/native_runtime_control_plane.rs`:

```rust
#[tokio::test]
async fn sqlite_adapter_resolves_session_by_task() {
    let fixture = fixture().await;
    let plane = SqliteNativeRuntimeControlPlane::new(fixture.store.pool().clone());

    let view = plane
        .session_for_task(&fixture.task_id)
        .await
        .expect("lookup")
        .expect("session bound to task");
    assert_eq!(view.session_id, fixture.session_id);
    assert_eq!(view.task_id, fixture.task_id);

    let missing = plane
        .session_for_task(&TaskRunId::new())
        .await
        .expect("lookup");
    assert!(missing.is_none());
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p agentd-store --test native_runtime_control_plane sqlite_adapter_resolves_session_by_task`
Expected: FAIL to compile — `session_for_task` not found.

- [ ] **Step 3: Implement across the four layers**

3a. Trait (`crates/agentd-core/src/ports/native_runtime.rs`), after `session_view`:

```rust
/// Resolve the runtime session the control plane bound to a task, if any.
async fn session_for_task(
    &self,
    task_id: &TaskRunId,
) -> Result<Option<NativeRuntimeSessionView>, NativeRuntimeControlError>;
```

3b. SQLite adapter (`crates/agentd-store/src/native_runtime_control_plane.rs`):

```rust
async fn session_for_task(
    &self,
    task_id: &agentd_core::types::TaskRunId,
) -> Result<Option<agentd_core::ports::NativeRuntimeSessionView>, NativeRuntimeControlError> {
    let session_id: Option<String> = sqlx::query_scalar(
        "SELECT id FROM runtime_sessions WHERE execution_task_id = ? \
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(task_id.as_str())
    .fetch_optional(&self.pool)
    .await
    .map_err(|error| NativeRuntimeControlError::Unavailable(error.to_string()))?;
    let Some(session_id) = session_id else {
        return Ok(None);
    };
    self.session_view(&agentd_core::types::RuntimeSessionId::from_string(session_id))
        .await
}
```

3c. Surface route (`crates/agentd-surface/src/native_runtime_http.rs`): add
`.route("/api/runtime/native/session/for-task", post(session_for_task))` and:

```rust
#[derive(Debug, serde::Deserialize)]
struct SessionForTaskRequest {
    task_id: agentd_core::types::TaskRunId,
}

async fn session_for_task(
    State(state): State<NativeRuntimeHttpState>,
    headers: HeaderMap,
    Json(request): Json<SessionForTaskRequest>,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    respond(state.control.session_for_task(&request.task_id).await)
}
```

3d. Client (`crates/agentd-bin/src/native_runtime_client.rs`):

```rust
pub async fn session_for_task(
    &self,
    task_id: &TaskRunId,
) -> Result<Option<agentd_core::ports::NativeRuntimeSessionView>, NativeRuntimeControlError> {
    self.post(
        "/api/runtime/native/session/for-task",
        &serde_json::json!({ "task_id": task_id }),
    )
    .await
}
```

Also implement `session_for_task` on the trait impl for `NativeRuntimeHttpClient` (same body, since the trait now requires it) and add a passthrough on the test fake in `crates/agentd-surface/tests/native_runtime_http.rs`:

```rust
async fn session_for_task(
    &self,
    _task_id: &TaskRunId,
) -> Result<Option<agentd_core::ports::NativeRuntimeSessionView>, NativeRuntimeControlError> {
    Ok(None)
}
```

`use agentd_core::types::TaskRunId;` where missing.

- [ ] **Step 4: Add the client round-trip assertion and run everything**

In `crates/agentd-bin/tests/native_runtime_client.rs`, extend the existing
`http_adapter_round_trips_attempt_lifecycle_against_daemon` test — after the
`session_view` assertions, add:

```rust
    let by_task = client
        .session_for_task(&fixture.task_id)
        .await
        .expect("session_for_task over HTTP")
        .expect("bound session");
    assert_eq!(by_task.session_id, fixture.session_id);
```

Run: `cargo test -p agentd-store --test native_runtime_control_plane && cargo test -p agentd-bin --test native_runtime_client && cargo test -p agentd-surface --test native_runtime_http`
Expected: PASS.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run -p agentd-core -p agentd-store -p agentd-surface -p agentd-bin
git add crates/agentd-core/src/ports/native_runtime.rs crates/agentd-store/src/native_runtime_control_plane.rs crates/agentd-surface/src/native_runtime_http.rs crates/agentd-surface/tests/native_runtime_http.rs crates/agentd-bin/src/native_runtime_client.rs crates/agentd-bin/tests/native_runtime_client.rs crates/agentd-store/tests/native_runtime_control_plane.rs
git commit -m "feat(runtime): resolve runtime sessions by task through the control plane

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: `agentd worker` — the remote worker loop

Compose Tasks 1–4 into a worker process: register an incarnation, pull a fenced lease, resolve the session, execute natively with background lease renewal, upload + acknowledge the transcript, release the lease. `--once` executes at most one task (test and smoke hook); without it the loop polls until Ctrl-C.

**Files:**
- Create: `crates/agentd-bin/src/worker_main.rs`
- Modify: `crates/agentd-bin/src/lib.rs` (add `pub mod worker_main;`)
- Modify: `crates/agentd-bin/src/cli.rs` (subcommand + parse test)
- Modify: `crates/agentd-bin/src/main.rs` (dispatch)
- Test: `crates/agentd-bin/tests/worker_main.rs`

**Interfaces:**
- Consumes: `WorkerFleetHttpClient` (register/pull + `TaskLeasePort`), `NativeRuntimeHttpClient` (runtime + artifacts + `session_for_task`), `AgentdWorker::with_control_planes` (Task 1), `native_process_config_from_spec` (exists).
- Produces: `pub struct WorkerRunReport { pub executed: u32, pub released: u32 }` and `pub async fn run_worker_once(daemon_url: &str, auth_proof: &str, state_dir: &Path, poll: Duration, deadline: Duration) -> Result<WorkerRunReport, NativeWorkerError>`; CLI `agentd worker --daemon-url <url> --auth-proof <token> --state-dir <dir> [--once]`.

- [ ] **Step 1: Write the failing end-to-end test**

Create `crates/agentd-bin/tests/worker_main.rs`. Reuse the fixture + `serve_daemon` from `tests/native_runtime_client.rs` (copy both helpers into this file; they cannot be imported across integration-test binaries). Additionally mount the worker-fleet router in this file's `serve_daemon`:

```rust
    let auth = agentd_surface::http::AuthConfig {
        api_token: Some(token.to_string()),
        ..agentd_surface::http::AuthConfig::default()
    };
    let fleet_router = agentd_surface::worker_fleet_http::worker_fleet_router(
        std::sync::Arc::new(
            agentd_store::worker_fleet::SqliteWorkerFleet::new(store.pool().clone())
                .with_auth_proof(token.to_string()),
        ),
        auth,
    );
    let app = daemon_native_runtime_router(&store, Some(token.to_string()))
        .merge(agentd_bin::daemon::recovery_router(service, token.to_string()))
        .merge(fleet_router);
```

The test:

```rust
#[tokio::test]
async fn worker_once_executes_a_dispatched_task_end_to_end() {
    let fixture = fixture().await;
    // Attach a runnable execution spec to the fixture task so the pulled
    // grant carries it (provider "codex" with a fake `codex` on PATH is NOT
    // used here — the spec's program must be an allowed provider, so use the
    // sh-based NativeExecutionSpec only if spec validation allows it;
    // otherwise point program at a temp `codex` shim script that exits 0):
    let shim_dir = tempfile::tempdir().expect("shim dir");
    let shim = shim_dir.path().join("codex");
    std::fs::write(&shim, "#!/bin/sh\nexit 0\n").expect("write shim");
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let spec = agentd_core::types::NativeExecutionSpec {
        version: 1,
        provider: "codex".into(),
        program: shim.to_string_lossy().into_owned(),
        args: vec![],
        cwd: Some(shim_dir.path().to_string_lossy().into_owned()),
        env: vec![],
    };
    use agentd_core::ports::Store as _;
    fixture
        .store
        .set_task_execution_spec(&fixture.task_id, &spec)
        .await
        .expect("attach spec");

    let base_url = serve_daemon(fixture.store.clone(), "worker-secret").await;
    let worker_state = tempfile::tempdir().expect("worker state");

    let report = agentd_bin::worker_main::run_worker_once(
        &base_url,
        "worker-secret",
        worker_state.path(),
        std::time::Duration::from_millis(100),
        std::time::Duration::from_secs(30),
    )
    .await
    .expect("worker run");

    assert_eq!(report.executed, 1);
    assert_eq!(report.released, 1);

    // The daemon-side session completed and an artifact was acknowledged.
    let session = runtime_session_repo::get_session(fixture.store.pool(), &fixture.session_id)
        .await
        .expect("session lookup")
        .expect("session");
    assert_eq!(
        session.status,
        agentd_core::types::RuntimeSessionStatus::Completed
    );
    let worker = agentd_bin::native_worker::AgentdWorker::new(fixture.store.clone());
    let artifacts = worker
        .list_artifacts_for_run(fixture.run_id.as_str())
        .await
        .expect("artifact listing");
    assert!(
        !artifacts.items.is_empty(),
        "worker must acknowledge at least the transcript artifact"
    );
}
```

Implementer notes: (1) check the exact accessor for setting a task execution
spec — `Store::set_task_execution_spec` per
`crates/agentd-store/tests/enterprise_task_leases.rs:203`; import the trait it
belongs to. (2) `NativeExecutionSpec.provider_matches_program()` compares
provider to the program's basename — the shim named `codex` satisfies it.
(3) Check `ArtifactPage`'s field name (`items` or similar) in
`agentd_core::ports` and adjust the assertion. (4) The fixture's session must
start in a state `start_attempt` accepts (`requested` — it does, per existing
tests). (5) The fleet pull path dispatches the open task to the pulling
incarnation — the fixture's `task_id` must be the open task (`insert_task_run`
leaves it open).

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p agentd-bin --test worker_main`
Expected: FAIL to compile — `worker_main` module missing.

- [ ] **Step 3: Implement `worker_main.rs`**

Create `crates/agentd-bin/src/worker_main.rs`:

```rust
//! The remote worker loop (M1): pull a fenced lease over HTTP, execute the
//! task natively, upload + acknowledge the transcript, release the lease.
//! The worker never opens the daemon database; its local store only backs
//! disposable scratch state.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use agentd_core::ports::{
    Clock, ExecutionArtifactKind, ExecutionArtifactPublish, ExecutionEvidenceLinks,
    WorkerArtifactReport, WorkerFleetPullRequest, WorkerFleetRegisterRequest,
};
use agentd_core::types::{ExecutionArtifactId, TaskLeaseGrant, WorkerId, WorkerIncarnationId};
use agentd_store::SqliteStore;
use serde_json::json;

use crate::native_runtime_client::NativeRuntimeHttpClient;
use crate::native_worker::{
    native_process_config_from_spec, AgentdWorker, NativeWorkerError,
};
use crate::worker_fleet_client::{WorkerFleetHttpClient, WorkerFleetRetryPolicy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerRunReport {
    pub executed: u32,
    pub released: u32,
}

/// Register a fresh incarnation, pull until one grant arrives (or the
/// deadline passes), execute it, and return. `--once` semantics.
pub async fn run_worker_once(
    daemon_url: &str,
    auth_proof: &str,
    state_dir: &Path,
    poll: Duration,
    deadline: Duration,
) -> Result<WorkerRunReport, NativeWorkerError> {
    std::fs::create_dir_all(state_dir)
        .map_err(|error| NativeWorkerError::InvalidRecovery(error.to_string()))?;
    let scratch = SqliteStore::connect(&state_dir.join("worker-scratch.db"))
        .await
        .map_err(NativeWorkerError::Store)?;

    let fleet = WorkerFleetHttpClient::new(daemon_url, auth_proof)
        .map_err(|error| NativeWorkerError::Fleet(error.to_string()))?;
    let runtime = NativeRuntimeHttpClient::new(daemon_url, auth_proof)?;
    let policy = WorkerFleetRetryPolicy::default();

    let incarnation_id = WorkerIncarnationId::new();
    let registration = WorkerFleetRegisterRequest {
        auth_proof: auth_proof.to_string(),
        worker_id: WorkerId::new(),
        trust_domain: "corp-coding".to_string(),
        labels: json!({}),
        incarnation_id: incarnation_id.clone(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        host_name: hostname_or_default(),
        network_zone: None,
        capabilities: json!({"runtime": ["codex", "claude-code"]}),
    };
    fleet
        .register_with_retry(&registration, policy)
        .await
        .map_err(|error| NativeWorkerError::Fleet(error.to_string()))?;

    let started = std::time::Instant::now();
    loop {
        if started.elapsed() > deadline {
            return Ok(WorkerRunReport {
                executed: 0,
                released: 0,
            });
        }
        let observed_at = crate::clock::SystemClock.now_unix();
        let request = WorkerFleetPullRequest {
            auth_proof: auth_proof.to_string(),
            worker_incarnation_id: incarnation_id.clone(),
            observed_at,
            expires_at: observed_at.saturating_add(60),
        };
        match fleet
            .pull_native_with_scope(&request, policy)
            .await
            .map_err(|error| NativeWorkerError::Fleet(error.to_string()))?
        {
            None => tokio::time::sleep(poll).await,
            Some(grant) => {
                return execute_grant(&scratch, &fleet, &runtime, incarnation_id, grant).await;
            }
        }
    }
}

async fn execute_grant(
    scratch: &SqliteStore,
    fleet: &WorkerFleetHttpClient,
    runtime: &NativeRuntimeHttpClient,
    incarnation_id: WorkerIncarnationId,
    grant: TaskLeaseGrant,
) -> Result<WorkerRunReport, NativeWorkerError> {
    let claim = grant.claim();
    let spec = grant.execution_spec.as_ref().ok_or_else(|| {
        NativeWorkerError::InvalidRecovery("pulled grant has no execution spec".into())
    })?;
    let config = native_process_config_from_spec(spec)?;

    // Resolve the daemon-bound runtime session for this task; a grant
    // without one cannot execute (release so the daemon can requeue).
    let Some(view) = runtime.session_for_task(&grant.execution_task_id).await? else {
        let observed_at = crate::clock::SystemClock.now_unix();
        let _ = fleet
            .release(&agentd_core::ports::TaskLeaseCloseRequest {
                claim: claim.clone(),
                observed_at,
                reason: "no runtime session bound to task".to_string(),
            })
            .await;
        return Err(NativeWorkerError::InvalidRecovery(
            "no runtime session bound to pulled task".into(),
        ));
    };

    let worker = AgentdWorker::with_control_planes(
        scratch.clone(),
        Arc::new(runtime.clone()),
        Arc::new(fleet.clone()),
    );
    let handle = worker
        .start_for_task(
            view.session_id.clone(),
            grant.execution_task_id.clone(),
            incarnation_id,
            config,
        )
        .await?;
    let renewal = handle.spawn_lease_renewal(
        claim.clone(),
        Duration::from_secs(10),
        Duration::from_secs(60),
    );

    let event = handle.wait(Duration::from_secs(3600)).await?;
    renewal.abort();

    // Upload the bounded transcript and acknowledge it under the claim.
    let output = handle.output_snapshot();
    let stored = runtime.upload_artifact(output).await?;
    let observed_at = crate::clock::SystemClock.now_unix();
    let report = WorkerArtifactReport {
        claim: claim.clone(),
        observed_at,
        artifact: ExecutionArtifactPublish {
            id: ExecutionArtifactId::new(),
            kind: ExecutionArtifactKind::Transcript,
            content_sha256: stored.content_sha256,
            size_bytes: stored.size_bytes,
            media_type: "text/plain".to_string(),
            storage_ref: stored.storage_ref,
            provenance: json!({
                "source": "agentd-worker",
                "event": format!("{event:?}"),
            }),
            links: ExecutionEvidenceLinks {
                execution_run_id: view.run_id_placeholder_see_note(),
                execution_task_id: Some(grant.execution_task_id.clone()),
                runtime_session_id: Some(view.session_id.clone()),
                runtime_attempt_id: Some(handle.attempt_id().clone()),
            },
        },
    };
    runtime.acknowledge_artifact(&report).await?;

    let observed_at = crate::clock::SystemClock.now_unix();
    handle
        .release_lease(&claim, observed_at, "worker execution complete")
        .await?;
    Ok(WorkerRunReport {
        executed: 1,
        released: 1,
    })
}

fn hostname_or_default() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "agentd-worker".to_string())
}
```

**Resolve before compiling** (the one open type question in this plan):
`ExecutionEvidenceLinks.execution_run_id` is a `RunId` the view does not carry.
Two options — pick whichever compiles cleanly against
`crates/agentd-core/src/ports/execution_evidence.rs`:
(a) if `execution_run_id` is optional or derivable, thread it through; otherwise
(b) extend `NativeRuntimeSessionView` with `pub run_id: RunId` in Task 4
(SQLite adapter joins `task_runs.run_id`; one extra field, serde-defaulted for
wire compat) and use `view.run_id` here. Option (b) is the expected route —
implement it in Task 4 Step 3b by selecting
`(SELECT run_id FROM task_runs WHERE id = execution_task_id)` into the view,
and replace `view.run_id_placeholder_see_note()` with `view.run_id.clone()`.

Register the module in `crates/agentd-bin/src/lib.rs`:

```rust
pub mod worker_main;
```

- [ ] **Step 4: Add the CLI subcommand**

In `crates/agentd-bin/src/cli.rs`, add to `AgentdCommand`:

```rust
/// Run a remote native worker against a daemon's HTTP control plane.
Worker(WorkerArgs),
```

and the args struct next to the other arg structs:

```rust
#[derive(Debug, Args)]
pub struct WorkerArgs {
    /// Daemon base URL, e.g. http://127.0.0.1:8787
    #[arg(long)]
    pub daemon_url: String,
    /// Worker auth proof (bearer token registered with the daemon fleet).
    #[arg(long)]
    pub auth_proof: String,
    /// Directory for worker-local scratch state.
    #[arg(long, default_value = ".agentd/worker")]
    pub state_dir: std::path::PathBuf,
    /// Execute at most one task, then exit.
    #[arg(long, default_value_t = false)]
    pub once: bool,
    /// Poll interval in milliseconds while waiting for work.
    #[arg(long, default_value_t = 1000)]
    pub poll_ms: u64,
    /// Give up waiting for work after this many seconds (0 = wait forever).
    #[arg(long, default_value_t = 0)]
    pub deadline_secs: u64,
}
```

Parse test in the existing `mod tests`:

```rust
#[test]
fn agentd_cli_worker_accepts_daemon_url_and_once() {
    let cli = AgentdCli::try_parse_from([
        "agentd",
        "worker",
        "--daemon-url",
        "http://127.0.0.1:8787",
        "--auth-proof",
        "worker-secret",
        "--once",
    ])
    .expect("worker parses");
    let Some(AgentdCommand::Worker(args)) = &cli.command else {
        panic!("expected worker subcommand");
    };
    assert_eq!(args.daemon_url, "http://127.0.0.1:8787");
    assert!(args.once);
    assert_eq!(args.poll_ms, 1000);
}
```

In `crates/agentd-bin/src/main.rs`, add the dispatch arm following the pattern of the other subcommands:

```rust
Some(AgentdCommand::Worker(args)) => {
    let poll = std::time::Duration::from_millis(args.poll_ms.max(10));
    let deadline = if args.deadline_secs == 0 {
        std::time::Duration::from_secs(u64::MAX / 4)
    } else {
        std::time::Duration::from_secs(args.deadline_secs)
    };
    loop {
        let report = agentd_bin::worker_main::run_worker_once(
            &args.daemon_url,
            &args.auth_proof,
            &args.state_dir,
            poll,
            deadline,
        )
        .await?;
        println!(
            "worker: executed={} released={}",
            report.executed, report.released
        );
        if args.once {
            break;
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Run tests until green, then gate and commit**

Run: `cargo test -p agentd-bin --test worker_main --test agent_cli 2>/dev/null || cargo test -p agentd-bin`
Expected: `worker_once_executes_a_dispatched_task_end_to_end` and the CLI parse test pass. Debug failures against the daemon-side state (the test owns both sides).

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo nextest run --workspace
git add crates/agentd-bin/src/worker_main.rs crates/agentd-bin/src/lib.rs crates/agentd-bin/src/cli.rs crates/agentd-bin/src/main.rs crates/agentd-bin/tests/worker_main.rs
git commit -m "feat(worker): add the agentd worker remote execution loop

agentd worker registers an incarnation, pulls a fenced lease, resolves the
task's runtime session, executes natively with background lease renewal,
uploads and acknowledges the transcript, and releases the lease — entirely
over the authenticated HTTP control plane.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Env-gated real smoke and parity update

**Files:**
- Modify: `crates/agentd-bin/tests/worker_main.rs` (add the gated smoke)
- Modify: `docs/parity/agent-chat-capability-map.md`

**Interfaces:** none new.

- [ ] **Step 1: Add the gated smoke test**

Append to `crates/agentd-bin/tests/worker_main.rs`:

```rust
/// Opt-in real smoke (M1 exit gate): requires a real provider CLI.
/// Run with: AGENTD_REAL_WORKER_SMOKE=1 cargo test -p agentd-bin \
///   --test worker_main real_remote_worker_smoke -- --ignored --nocapture
#[tokio::test]
#[ignore = "requires AGENTD_REAL_WORKER_SMOKE=1 and a real codex CLI"]
async fn real_remote_worker_smoke() {
    if std::env::var("AGENTD_REAL_WORKER_SMOKE").as_deref() != Ok("1") {
        eprintln!("skipping: AGENTD_REAL_WORKER_SMOKE!=1");
        return;
    }
    let codex = which_codex().expect("codex CLI on PATH");
    let fixture = fixture().await;
    let workdir = tempfile::tempdir().expect("workdir");
    let spec = agentd_core::types::NativeExecutionSpec {
        version: 1,
        provider: "codex".into(),
        program: codex,
        args: vec!["exec".into(), "--json".into(), "reply with the word done".into()],
        cwd: Some(workdir.path().to_string_lossy().into_owned()),
        env: vec![],
    };
    use agentd_core::ports::Store as _;
    fixture
        .store
        .set_task_execution_spec(&fixture.task_id, &spec)
        .await
        .expect("attach spec");
    let base_url = serve_daemon(fixture.store.clone(), "worker-secret").await;
    let state = tempfile::tempdir().expect("state");

    let report = agentd_bin::worker_main::run_worker_once(
        &base_url,
        "worker-secret",
        state.path(),
        std::time::Duration::from_millis(200),
        std::time::Duration::from_secs(600),
    )
    .await
    .expect("real worker run");
    assert_eq!(report.executed, 1);
}

fn which_codex() -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    path.split(':')
        .map(|dir| std::path::Path::new(dir).join("codex"))
        .find(|candidate| candidate.is_file())
        .map(|p| p.to_string_lossy().into_owned())
}
```

- [ ] **Step 2: Verify the smoke is skipped by default and compiles**

Run: `cargo test -p agentd-bin --test worker_main`
Expected: normal tests pass; `real_remote_worker_smoke` reported as ignored.

Optionally (requires local codex login): `AGENTD_REAL_WORKER_SMOKE=1 cargo test -p agentd-bin --test worker_main real_remote_worker_smoke -- --ignored --nocapture` — record the result in the commit message body, and do not claim it passed if it was not run.

- [ ] **Step 3: Update the parity map**

In `docs/parity/agent-chat-capability-map.md`, update exactly these rows (keep table formatting):
- `native_runtime_process`: status `missing` → `partial`, append to its note: "M1 adds the control-plane-ported native worker and the `agentd worker` remote loop (worker path is tmux-free); daemon workflow dispatch still composes tmux, so full coverage lands with M2 native dispatch."
- `native_runtime_session_restore`: status `missing` → `covered`, note: "session_view/session_for_task + provider-native resume over the authenticated control plane; a worker with no local DB recovers a resume_pending session (worker_main + native_runtime_client tests)."
- `worker_fleet_protocol`: keep `partial`, append: "M1 exercises register/pull/renew/release end-to-end from the remote worker loop."
- `artifact_audit_provenance`: keep `partial`, append: "M1 adds HTTP content-addressed upload and fenced worker acknowledgement."

- [ ] **Step 4: Gate and commit**

```bash
cargo fmt --all
cargo clippy -p agentd-bin --all-targets -- -D warnings
cargo test -p agentd-bin --test worker_main
git add crates/agentd-bin/tests/worker_main.rs docs/parity/agent-chat-capability-map.md
git commit -m "test(worker): add env-gated real remote-worker smoke; update parity

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Self-Review Notes

- **Spec coverage vs design doc §M1:** item 1 (lease via injected port) → Task 1; item 2 (artifact upload/ack over HTTP) → Tasks 2–3; item 3 (`agentd worker` CLI) → Task 5; item 4 (opt-in real smoke + restart-recovery evidence) → Task 6 smoke + the existing `remote_worker_recovers_without_a_local_runtime_session` test (already on main). The task-session lookup (Task 4) is the missing plumbing between grant and session that M1's outcome sentence implies.
- **Known open type question:** `ExecutionEvidenceLinks.execution_run_id` — resolved by extending `NativeRuntimeSessionView` with `run_id` in Task 4 (explicit instruction in Task 5 Step 3).
- **M1 exit gate honesty (per design doc):** M1 claims the *worker path* is tmux-free; daemon workflow dispatch keeps tmux until M2. The parity note in Task 6 states this explicitly.
