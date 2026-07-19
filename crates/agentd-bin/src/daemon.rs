//! The daemon assembly (P0.9 9b): construct the production host, build the
//! HTTP/SSE router, discover in-flight parked runs on boot, and serve.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentd_core::CoreError;
use agentd_core::ports::{
    AgentAllocation, AgentBackend, ArtifactIndexPort, ExecutionArtifactKind,
    ExecutionEvidenceLinks, ExecutionSnapshotLink, MtlsWorkloadVerifier, WorkerFleetPort,
    WorkerFleetPullRequest, WorktreeAllocator,
};
use agentd_core::types::{AgentHandle, ExecutionArtifactId, SpawnRequest};
use agentd_security::{
    EnterpriseSecurityConfig, PeerCertificateVerifier, WorkloadIdentityVerifier,
};
use agentd_store::content_store::{
    ArtifactObjectStore, LocalContentStore, S3CompatibleObjectStore,
};
use agentd_store::execution_evidence_control_plane::SqliteExecutionEvidenceControlPlane;
use agentd_store::native_runtime_control_plane::SqliteNativeRuntimeControlPlane;
use agentd_store::task_lease_control_plane::SqliteTaskLeaseControlPlane;
use agentd_store::worker_fleet::SqliteWorkerFleet;
use agentd_store::{FailedWorktreeCleanupCandidate, SqliteStore};
use agentd_surface::host::RunHost;
use agentd_surface::http::{AppState, AuthConfig, MediaConfig, SchedulerConfig, router};
use agentd_surface::native_runtime_http::native_runtime_router;
use agentd_surface::worker_fleet_http::worker_fleet_router;
use agentd_surface::worker_fleet_mtls_http::worker_fleet_mtls_router;
use agentd_tmux::{
    Config, GitWorktreeProvider, ShutdownMethod, ShutdownOpts, TmuxBackend, TokioCommandRunner,
    WorktreePool,
};
use axum::Router;
use axum::extract::{Path as AxumPath, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{
    Json,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;

use crate::agent_mcp_context::{McpStdioContextBackend, mcp_stdio_command_from_current_process};
use crate::cli::DaemonConfig;
use crate::clock::SystemClock;
use crate::host::{
    AgentLifecycle, AgentLifecycleShutdown, AgentLifecycleShutdownReport, ProductionRunHost,
};
use crate::mempal::OfflineMempal;
use crate::native_worker::{
    AgentdWorker, NativeRecoveryRegistry, NativeWorkerSecurityBinding,
    native_grant_execution_context, native_process_config_from_spec,
};

/// Build the HTTP/SSE router over a host (testable via `tower::oneshot`).
pub fn build_router(host: Arc<dyn RunHost>) -> Router {
    build_router_with_auth(host, AuthConfig::open())
}

/// Build the HTTP/SSE router with explicit API auth configuration.
pub fn build_router_with_auth(host: Arc<dyn RunHost>, auth: AuthConfig) -> Router {
    build_router_with_auth_and_media(host, auth, MediaConfig::default_local())
}

/// Build the HTTP/SSE router with an explicit local media staging root.
pub fn build_router_with_media_dir(
    host: Arc<dyn RunHost>,
    media_dir: impl Into<PathBuf>,
) -> Router {
    build_router_with_auth_and_media(host, AuthConfig::open(), MediaConfig::new(media_dir))
}

/// Build the HTTP/SSE router with explicit API auth and media staging settings.
pub fn build_router_with_auth_and_media(
    host: Arc<dyn RunHost>,
    auth: AuthConfig,
    media: MediaConfig,
) -> Router {
    router(AppState {
        host,
        auth,
        media,
        scheduler: SchedulerConfig::default(),
    })
}

/// Build the daemon surface with the worker-fleet transport mounted.
pub fn build_router_with_worker_fleet(
    host: Arc<dyn RunHost>,
    auth: AuthConfig,
    media: MediaConfig,
    fleet: Arc<dyn agentd_core::ports::WorkerFleetPort>,
) -> Router {
    build_router_with_auth_and_media(host, auth.clone(), media)
        .merge(worker_fleet_router(fleet, auth))
}

pub fn build_router_with_worker_fleet_mtls(
    host: Arc<dyn RunHost>,
    auth: AuthConfig,
    media: MediaConfig,
    fleet: Arc<dyn WorkerFleetPort>,
    verifier: Arc<dyn MtlsWorkloadVerifier>,
) -> Router {
    build_router_with_worker_fleet(host, auth, media, fleet.clone())
        .merge(worker_fleet_mtls_router(fleet, verifier))
}

/// Run one durable worker-fleet maintenance tick.
pub async fn worker_fleet_tick(
    fleet: &dyn WorkerFleetPort,
    recovery_registry: &NativeRecoveryRegistry,
    native_worker: &AgentdWorker,
    observed_at: i64,
) {
    let _ = fleet.recover_offline(observed_at - 30).await;
    let _ = fleet.expire_due(observed_at).await;
    let _ = recovery_registry.recover_one(native_worker).await;
}

#[derive(Clone)]
pub struct WorkerFleetService {
    fleet: Arc<dyn WorkerFleetPort>,
    recovery_registry: Arc<NativeRecoveryRegistry>,
    native_worker: AgentdWorker,
    content_store: Arc<dyn ArtifactObjectStore>,
}

#[derive(Clone)]
struct RecoveryApiState {
    service: Arc<WorkerFleetService>,
    token: String,
}

#[derive(Debug, Deserialize)]
struct CodexRecoveryRequest {
    session_id: String,
    worker_incarnation_id: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    env: Vec<(String, String)>,
}

#[derive(Debug, Deserialize)]
struct CutoverTransitionRequest {
    phase: agentd_store::cutover_repo::CutoverPhase,
    authority_revision: String,
    matrix_cursor: i64,
    lease_epoch: i64,
}

#[derive(Debug, Deserialize)]
struct CutoverRollbackRequest {
    lease_epoch: i64,
}

#[derive(Debug, Deserialize)]
struct CutoverDrainRequest {
    #[serde(default = "default_drain_timeout_ms")]
    timeout_ms: u64,
    authority_revision: String,
    matrix_cursor: i64,
    lease_epoch: i64,
}

const fn default_drain_timeout_ms() -> u64 {
    30_000
}

/// Native runtime control-plane transport backed by the daemon SQLite store.
///
/// Remote workers call this instead of opening the daemon database; the
/// SQLite adapter stays daemon-side per the AD-E5 boundary.
pub fn daemon_native_runtime_router(store: &SqliteStore, token: Option<String>) -> Router {
    let control = Arc::new(SqliteNativeRuntimeControlPlane::new(store.pool().clone()));
    let auth = AuthConfig {
        api_token: token,
        ..AuthConfig::default()
    };
    native_runtime_router(control, auth)
}

pub fn recovery_router(service: Arc<WorkerFleetService>, token: String) -> Router {
    Router::new()
        .route("/api/runtime/recover", post(register_codex_recovery))
        .route("/api/runtime/capabilities", get(runtime_capabilities))
        .route("/api/cutover/inventory", get(cutover_inventory))
        .route(
            "/api/cutover/projects/:project_id",
            get(cutover_project_state),
        )
        .route(
            "/api/cutover/projects/:project_id/transition",
            post(transition_cutover_project),
        )
        .route(
            "/api/cutover/projects/:project_id/promote",
            post(promote_cutover_project),
        )
        .route(
            "/api/cutover/projects/:project_id/rollback",
            post(rollback_cutover_project),
        )
        .route(
            "/api/cutover/projects/:project_id/drain",
            post(drain_cutover_project),
        )
        .route(
            "/api/runtime/runs/:run_id/artifacts",
            get(runtime_run_artifacts),
        )
        .with_state(RecoveryApiState { service, token })
}

async fn cutover_inventory(State(state): State<RecoveryApiState>, headers: HeaderMap) -> Response {
    let expected = format!("Bearer {}", state.token);
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    match state.service.native_worker.cutover_inventory().await {
        Ok(inventory) => Json(inventory).into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

async fn cutover_project_state(
    State(state): State<RecoveryApiState>,
    AxumPath(project_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let expected = format!("Bearer {}", state.token);
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    match state
        .service
        .native_worker
        .cutover_project_state(&project_id)
        .await
    {
        Ok(Some(state)) => Json(state).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

async fn transition_cutover_project(
    State(state): State<RecoveryApiState>,
    AxumPath(project_id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<CutoverTransitionRequest>,
) -> Response {
    let expected = format!("Bearer {}", state.token);
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    match state
        .service
        .native_worker
        .transition_cutover_project(
            &project_id,
            request.phase,
            &request.authority_revision,
            request.matrix_cursor,
            request.lease_epoch,
        )
        .await
    {
        Ok(state) => Json(state).into_response(),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

async fn promote_cutover_project(
    State(state): State<RecoveryApiState>,
    AxumPath(project_id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<CutoverTransitionRequest>,
) -> Response {
    let expected = format!("Bearer {}", state.token);
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    let Ok(Some(current)) = state
        .service
        .native_worker
        .cutover_project_state(&project_id)
        .await
    else {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "not_found" }))).into_response();
    };
    if current.phase != agentd_store::cutover_repo::CutoverPhase::Canary {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "error": "only canary projects can be promoted" })),
        )
            .into_response();
    }
    match state
        .service
        .native_worker
        .transition_cutover_project(
            &project_id,
            agentd_store::cutover_repo::CutoverPhase::Cutover,
            &request.authority_revision,
            request.matrix_cursor,
            request.lease_epoch,
        )
        .await
    {
        Ok(state) => Json(state).into_response(),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

async fn rollback_cutover_project(
    State(state): State<RecoveryApiState>,
    AxumPath(project_id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<CutoverRollbackRequest>,
) -> Response {
    let expected = format!("Bearer {}", state.token);
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    match state
        .service
        .native_worker
        .rollback_cutover_project(&project_id, request.lease_epoch)
        .await
    {
        Ok(state) => Json(state).into_response(),
        Err(error) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

async fn drain_cutover_project(
    State(state): State<RecoveryApiState>,
    AxumPath(project_id): AxumPath<String>,
    headers: HeaderMap,
    Json(request): Json<CutoverDrainRequest>,
) -> Response {
    let expected = format!("Bearer {}", state.token);
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    match state
        .service
        .native_worker
        .drain_project_until_ready(&project_id, request.timeout_ms)
        .await
    {
        Ok(_) => match state
            .service
            .native_worker
            .transition_cutover_project(
                &project_id,
                agentd_store::cutover_repo::CutoverPhase::Drain,
                &request.authority_revision,
                request.matrix_cursor,
                request.lease_epoch,
            )
            .await
        {
            Ok(cutover) => Json(cutover).into_response(),
            Err(error) => (
                StatusCode::CONFLICT,
                Json(json!({ "error": error.to_string() })),
            )
                .into_response(),
        },
        Err(error) => (
            StatusCode::REQUEST_TIMEOUT,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

async fn runtime_run_artifacts(
    State(state): State<RecoveryApiState>,
    AxumPath(run_id): AxumPath<String>,
    headers: HeaderMap,
) -> Response {
    let expected = format!("Bearer {}", state.token);
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    match state
        .service
        .native_worker
        .list_artifacts_for_run(&run_id)
        .await
    {
        Ok(page) => Json(page).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

async fn runtime_capabilities(
    State(state): State<RecoveryApiState>,
    headers: HeaderMap,
) -> Response {
    let expected = format!("Bearer {}", state.token);
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    Json(json!({
        "runtime": "native",
        "runtimeApiVersion": 1,
        "providers": ["codex", "claude-code"],
        "sessionResume": true,
        "leaseFencing": true,
        "artifactAcknowledgement": true,
        "workerProtocol": "http-or-mtls",
        "artifactStorage": "content-addressed",
        "legacyTmux": "compatibility-only"
    }))
    .into_response()
}

async fn register_codex_recovery(
    State(state): State<RecoveryApiState>,
    headers: HeaderMap,
    Json(request): Json<CodexRecoveryRequest>,
) -> Response {
    let expected = format!("Bearer {}", state.token);
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }
    let result = state.service.register_codex_recovery(
        agentd_core::types::RuntimeSessionId::from_string(request.session_id),
        agentd_core::types::WorkerIncarnationId::from_string(request.worker_incarnation_id),
        request.cwd,
        request.env,
    );
    match result {
        Ok(()) => (StatusCode::ACCEPTED, Json(json!({ "accepted": true }))).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": error.to_string() })),
        )
            .into_response(),
    }
}

impl std::fmt::Debug for WorkerFleetService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WorkerFleetService")
            .finish_non_exhaustive()
    }
}

impl WorkerFleetService {
    #[must_use]
    pub fn new(
        fleet: Arc<dyn WorkerFleetPort>,
        native_worker: AgentdWorker,
        content_store: Arc<dyn ArtifactObjectStore>,
    ) -> Self {
        Self {
            fleet,
            recovery_registry: Arc::new(NativeRecoveryRegistry::new()),
            native_worker,
            content_store,
        }
    }

    pub fn register_recovery(
        &self,
        request: crate::native_worker::NativeRecoveryRequest,
    ) -> Result<(), crate::native_worker::NativeWorkerError> {
        self.recovery_registry.register(request)
    }

    pub fn register_codex_recovery(
        &self,
        session_id: agentd_core::types::RuntimeSessionId,
        worker_incarnation_id: agentd_core::types::WorkerIncarnationId,
        cwd: Option<String>,
        env: Vec<(String, String)>,
    ) -> Result<(), crate::native_worker::NativeWorkerError> {
        self.register_recovery(crate::native_worker::NativeRecoveryRequest {
            session_id,
            worker_incarnation_id,
            config: agentd_tmux::native::NativeProcessConfig {
                program: "codex".into(),
                args: Vec::new(),
                cwd,
                env,
                ..agentd_tmux::native::NativeProcessConfig::default()
            },
        })
    }

    /// Execute one already-authorized native claim and reconcile its lease.
    /// This is the worker-side lifecycle boundary used by transports and
    /// higher-level dispatchers; it intentionally accepts a pinned security
    /// binding rather than deriving authority from task metadata.
    pub async fn execute_native_claim(
        &self,
        session_id: agentd_core::types::RuntimeSessionId,
        worker_incarnation_id: agentd_core::types::WorkerIncarnationId,
        config: agentd_tmux::native::NativeProcessConfig,
        binding: NativeWorkerSecurityBinding,
        observed_at: i64,
        execution_timeout: Duration,
        renewal_interval: Duration,
        lease_duration: Duration,
    ) -> Result<agentd_tmux::native::NativeProcessEvent, crate::native_worker::NativeWorkerError>
    {
        let claim = binding.scope.lease_claim.clone();
        let handle = self
            .native_worker
            .start_secured(
                session_id,
                worker_incarnation_id,
                config,
                &binding,
                observed_at,
            )
            .await?;
        let renewal = handle.spawn_lease_renewal(claim.clone(), renewal_interval, lease_duration);
        let event = match handle.wait(execution_timeout).await {
            Ok(event) => event,
            Err(error) => {
                renewal.abort();
                let _ = handle
                    .cancel_secured_lease(unix_now(), "native_wait_failed")
                    .await;
                return Err(error);
            }
        };
        renewal.abort();
        match &event {
            agentd_tmux::native::NativeProcessEvent::Exited { code, .. } => {
                let reason = if *code == Some(0) {
                    "native_completed"
                } else {
                    "native_failed"
                };
                handle.release_secured_lease(unix_now(), reason).await?;
            }
            agentd_tmux::native::NativeProcessEvent::Gone { .. } => {
                handle
                    .cancel_secured_lease(unix_now(), "native_runtime_gone")
                    .await?;
            }
        }
        Ok(event)
    }

    /// Execute a native claim and publish its bounded output before releasing
    /// the lease. Failed acknowledgement cancels the lease while leaving the
    /// content-addressed object available for an explicit retry.
    pub async fn execute_native_claim_with_artifact(
        &self,
        session_id: agentd_core::types::RuntimeSessionId,
        worker_incarnation_id: agentd_core::types::WorkerIncarnationId,
        config: agentd_tmux::native::NativeProcessConfig,
        binding: NativeWorkerSecurityBinding,
        observed_at: i64,
        execution_timeout: Duration,
        renewal_interval: Duration,
        lease_duration: Duration,
        content_store: &dyn agentd_store::content_store::ArtifactObjectStore,
        artifact_port: &(dyn ArtifactIndexPort + Send + Sync),
        artifact_id: ExecutionArtifactId,
        links: ExecutionEvidenceLinks,
    ) -> Result<agentd_tmux::native::NativeProcessEvent, crate::native_worker::NativeWorkerError>
    {
        let claim = binding.scope.lease_claim.clone();
        let handle = self
            .native_worker
            .start_secured(
                session_id,
                worker_incarnation_id,
                config,
                &binding,
                observed_at,
            )
            .await?;
        let renewal = handle.spawn_lease_renewal(claim.clone(), renewal_interval, lease_duration);
        let event = match handle.wait(execution_timeout).await {
            Ok(event) => event,
            Err(error) => {
                renewal.abort();
                let _ = handle
                    .cancel_secured_lease(unix_now(), "native_wait_failed")
                    .await;
                return Err(error);
            }
        };
        renewal.abort();
        if matches!(
            &event,
            agentd_tmux::native::NativeProcessEvent::Exited { .. }
        ) {
            let provenance = json!({
                "source": "agentd-native-runtime",
                "status": match &event {
                    agentd_tmux::native::NativeProcessEvent::Exited { code: Some(0), .. } => "success",
                    _ => "failure",
                },
            });
            if let Err(error) = handle
                .spool_and_acknowledge_to_store(
                    artifact_port,
                    content_store,
                    claim,
                    unix_now(),
                    artifact_id,
                    ExecutionArtifactKind::Log,
                    "text/plain".to_string(),
                    provenance,
                    links,
                )
                .await
            {
                let _ = handle
                    .cancel_secured_lease(unix_now(), "artifact_ack_failed")
                    .await;
                return Err(error);
            }
            let reason = if matches!(
                &event,
                agentd_tmux::native::NativeProcessEvent::Exited { code: Some(0), .. }
            ) {
                "native_completed"
            } else {
                "native_failed"
            };
            handle.release_secured_lease(unix_now(), reason).await?;
        } else {
            handle
                .cancel_secured_lease(unix_now(), "native_runtime_gone")
                .await?;
        }
        Ok(event)
    }

    /// Execute a lease grant using its durable, validated execution spec.
    pub async fn execute_native_grant(
        &self,
        session_id: agentd_core::types::RuntimeSessionId,
        grant: agentd_core::types::TaskLeaseGrant,
        binding: NativeWorkerSecurityBinding,
        observed_at: i64,
        execution_timeout: Duration,
        renewal_interval: Duration,
        lease_duration: Duration,
    ) -> Result<agentd_tmux::native::NativeProcessEvent, crate::native_worker::NativeWorkerError>
    {
        self.native_worker
            .validate_session_task(&session_id, &grant.execution_task_id)
            .await?;
        let spec = grant.execution_spec.ok_or_else(|| {
            crate::native_worker::NativeWorkerError::InvalidRecovery(
                "worker lease grant has no execution spec".into(),
            )
        })?;
        let config = native_process_config_from_spec(&spec)?;
        self.execute_native_claim(
            session_id,
            grant.worker_incarnation_id,
            config,
            binding,
            observed_at,
            execution_timeout,
            renewal_interval,
            lease_duration,
        )
        .await
    }

    /// Pull one lease and execute it through the native lifecycle boundary.
    pub async fn pull_and_execute_native(
        &self,
        request: WorkerFleetPullRequest,
        session_id: agentd_core::types::RuntimeSessionId,
        binding: NativeWorkerSecurityBinding,
        execution_timeout: Duration,
        renewal_interval: Duration,
        lease_duration: Duration,
    ) -> Result<
        Option<agentd_tmux::native::NativeProcessEvent>,
        crate::native_worker::NativeWorkerError,
    > {
        let grant =
            self.fleet.pull(&request).await.map_err(|error| {
                crate::native_worker::NativeWorkerError::Fleet(error.to_string())
            })?;
        let Some(grant) = grant else {
            return Ok(None);
        };
        if grant.worker_incarnation_id != request.worker_incarnation_id
            || grant.claim() != binding.scope.lease_claim
        {
            return Err(crate::native_worker::NativeWorkerError::InvalidRecovery(
                "pulled lease does not match worker security binding".into(),
            ));
        }
        self.execute_native_grant_with_artifact(
            session_id,
            grant,
            binding,
            request.observed_at,
            execution_timeout,
            renewal_interval,
            lease_duration,
        )
        .await
        .map(Some)
    }

    pub async fn execute_native_grant_with_artifact(
        &self,
        session_id: agentd_core::types::RuntimeSessionId,
        grant: agentd_core::types::TaskLeaseGrant,
        binding: NativeWorkerSecurityBinding,
        observed_at: i64,
        execution_timeout: Duration,
        renewal_interval: Duration,
        lease_duration: Duration,
    ) -> Result<agentd_tmux::native::NativeProcessEvent, crate::native_worker::NativeWorkerError>
    {
        self.native_worker
            .validate_session_task(&session_id, &grant.execution_task_id)
            .await?;
        let spec = grant.execution_spec.clone().ok_or_else(|| {
            crate::native_worker::NativeWorkerError::InvalidRecovery(
                "worker lease grant has no execution spec".into(),
            )
        })?;
        let config = native_process_config_from_spec(&spec)?;
        let session = agentd_store::runtime_session_repo::get_session(
            self.native_worker.store().pool(),
            &session_id,
        )
        .await?
        .ok_or_else(|| {
            crate::native_worker::NativeWorkerError::InvalidRecovery(
                "runtime session not found".into(),
            )
        })?;
        let run_id = agentd_store::task_repo::get_task_run_run_id(
            self.native_worker.store().pool(),
            &grant.execution_task_id,
        )
        .await?;
        let runtime_attempt_id = agentd_store::runtime_session_repo::latest_attempt(
            self.native_worker.store().pool(),
            &session_id,
        )
        .await?
        .map(|attempt| attempt.id);
        let links = ExecutionEvidenceLinks {
            execution_run_id: run_id,
            execution_task_id: Some(grant.execution_task_id.clone()),
            runtime_session_id: Some(session_id.clone()),
            runtime_attempt_id: runtime_attempt_id.clone(),
            worker_incarnation_id: Some(grant.worker_incarnation_id.clone()),
            snapshot: ExecutionSnapshotLink {
                authority_key: session.snapshot.authority_key,
                resource_kind: session.snapshot.resource_kind,
                resource_id: session.snapshot.resource_id,
                resource_version: session.snapshot.resource_version,
                content_sha256: session.snapshot.content_sha256,
            },
            target_repository_id: "unspecified".into(),
            target_base_commit: "unspecified".into(),
        };
        // The attempt is the durable execution boundary. Deriving the artifact
        // id from it makes a reconnect/retry idempotent instead of publishing a
        // second logical artifact for the same native execution.
        let artifact_id = runtime_attempt_id
            .as_ref()
            .map(|attempt| {
                ExecutionArtifactId::from_string(
                    attempt
                        .as_str()
                        .strip_prefix("ra_")
                        .map(|suffix| format!("ar_{suffix}"))
                        .unwrap_or_else(|| format!("ar_{}", attempt.as_str())),
                )
            })
            .unwrap_or_default();
        let lease_port =
            SqliteTaskLeaseControlPlane::new(self.native_worker.store().pool().clone());
        let evidence = SqliteExecutionEvidenceControlPlane::new(
            self.native_worker.store().pool().clone(),
            lease_port,
        );
        self.execute_native_claim_with_artifact(
            session_id,
            grant.worker_incarnation_id,
            config,
            binding,
            observed_at,
            execution_timeout,
            renewal_interval,
            lease_duration,
            self.content_store.as_ref(),
            &evidence,
            artifact_id,
            links,
        )
        .await
    }

    /// Execute a control-plane grant through the native artifact-aware path.
    /// The grant is the sole source of session and security identity; callers
    /// cannot supply a weaker ad-hoc binding for remote execution.
    pub async fn execute_remote_native_grant(
        &self,
        grant: agentd_core::types::TaskLeaseGrant,
        observed_at: i64,
        execution_timeout: Duration,
        renewal_interval: Duration,
        lease_duration: Duration,
    ) -> Result<agentd_tmux::native::NativeProcessEvent, crate::native_worker::NativeWorkerError>
    {
        let (session_id, binding) = native_grant_execution_context(&grant)?;
        self.execute_native_grant_with_artifact(
            session_id,
            grant,
            binding,
            observed_at,
            execution_timeout,
            renewal_interval,
            lease_duration,
        )
        .await
    }

    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                interval.tick().await;
                worker_fleet_tick(
                    self.fleet.as_ref(),
                    self.recovery_registry.as_ref(),
                    &self.native_worker,
                    unix_now(),
                )
                .await;
            }
        })
    }
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Production media root colocated with the daemon database.
#[must_use]
pub fn media_dir_for_db(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("media")
}

fn production_content_store(media_dir: &Path) -> Result<Arc<dyn ArtifactObjectStore>, String> {
    match std::env::var("AGENTD_OBJECT_STORE_ENDPOINT") {
        Ok(endpoint) if !endpoint.trim().is_empty() => {
            let token = std::env::var("AGENTD_OBJECT_STORE_BEARER").ok();
            let store =
                S3CompatibleObjectStore::new(endpoint, token).map_err(|error| error.to_string())?;
            Ok(Arc::new(store))
        }
        _ => Ok(Arc::new(
            LocalContentStore::new(media_dir.join("artifacts"))
                .map_err(|error| error.to_string())?,
        )),
    }
}

#[derive(Clone)]
struct SharedTmuxBackend(Arc<TmuxBackend>);

#[async_trait::async_trait]
impl AgentBackend for SharedTmuxBackend {
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        self.0.spawn(req).await
    }

    async fn dispatch_allocated(
        &self,
        req: SpawnRequest,
        allocation: &AgentAllocation,
    ) -> Result<AgentHandle, CoreError> {
        self.0.dispatch_allocated(req, allocation).await
    }
}

#[derive(Clone)]
struct TmuxAgentLifecycle(Arc<TmuxBackend>);

#[async_trait::async_trait]
impl AgentLifecycle for TmuxAgentLifecycle {
    async fn shutdown(
        &self,
        handle: &AgentHandle,
        opts: AgentLifecycleShutdown,
    ) -> Result<AgentLifecycleShutdownReport, CoreError> {
        let report = self
            .0
            .shutdown(
                handle,
                ShutdownOpts {
                    archive_to: opts.archive_to,
                },
            )
            .await?;
        Ok(AgentLifecycleShutdownReport {
            method: match report.method {
                ShutdownMethod::Graceful => "graceful",
                ShutdownMethod::Interrupt => "interrupt",
                ShutdownMethod::Kill => "kill",
            }
            .to_string(),
            final_capture_sha: report.final_capture_sha,
        })
    }

    async fn rebind(&self, target: &str) -> Result<Option<AgentHandle>, CoreError> {
        self.0.rebind(target).await.map_err(Into::into)
    }
}

/// Construct the production host from a real `SqliteStore`, the real
/// `TmuxBackend`, the offline mempal, and the system clock. Spawning real agents
/// is a runtime (deployment) concern; constructing the host does not touch tmux.
///
/// # Errors
/// [`CoreError`] if the store cannot be opened/migrated.
pub async fn build_production_host(config: &DaemonConfig) -> Result<ProductionRunHost, CoreError> {
    let store = SqliteStore::connect(&config.db_path).await?;
    std::fs::create_dir_all(&config.worktree_base)?;
    let worktree_pool = WorktreePool::new(Arc::new(GitWorktreeProvider::new(
        config.repo_dir.clone(),
        config.worktree_base.clone(),
    )));
    gc_worktrees_on_boot(&store, &worktree_pool).await?;
    let tmux_backend = Arc::new(TmuxBackend::new(
        Arc::new(TokioCommandRunner::new()),
        PathBuf::from("tmux"),
        Config::default(),
    ));
    let mcp_stdio_command = mcp_stdio_command_from_current_process(config)?;
    let backend = McpStdioContextBackend::new(
        Box::new(SharedTmuxBackend(Arc::clone(&tmux_backend))),
        mcp_stdio_command,
    );
    Ok(ProductionRunHost::new(
        store,
        Box::new(backend),
        Box::new(TokioCommandRunner::new()),
        Box::new(OfflineMempal),
        Box::new(SystemClock),
        config.workflows_dir.clone(),
    )
    .with_agent_lifecycle(Box::new(TmuxAgentLifecycle(Arc::clone(&tmux_backend))))
    .with_tool_cwd(config.repo_dir.clone())
    .with_worktree_allocator(Some(Box::new(worktree_pool))))
}

/// Run daemon boot-GC over the worktree pool while preserving worktrees that
/// the durable store still references for non-finished runs.
///
/// # Errors
/// [`CoreError`] if the store query or worktree provider operation fails.
pub async fn gc_worktrees_on_boot(
    store: &SqliteStore,
    worktree_pool: &WorktreePool,
) -> Result<(), CoreError> {
    let preserve_paths = store.active_worktree_paths().await?;
    worktree_pool.gc_on_boot_preserving(preserve_paths).await
}

/// Result of a failed-run worktree cleanup pass.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeCleanupPlan {
    /// Candidates found before any release attempt.
    pub candidates: Vec<FailedWorktreeCleanupCandidate>,
    /// Number of candidates actually released.
    pub released: usize,
    /// Whether this pass executed releases or only reported candidates.
    pub execute: bool,
}

/// Open the configured store/pool and run failed-run worktree cleanup.
///
/// # Errors
/// [`CoreError`] if the store, provider, or release operation fails.
pub async fn cleanup_failed_worktrees_from_config(
    config: &DaemonConfig,
    execute: bool,
) -> Result<WorktreeCleanupPlan, CoreError> {
    let store = SqliteStore::connect(&config.db_path).await?;
    let worktree_pool = WorktreePool::new(Arc::new(GitWorktreeProvider::new(
        config.repo_dir.clone(),
        config.worktree_base.clone(),
    )));
    cleanup_failed_worktrees(&store, &worktree_pool, execute).await
}

/// List or release worktrees attached to failed runs only.
///
/// # Errors
/// [`CoreError`] if the store query or release operation fails.
pub async fn cleanup_failed_worktrees(
    store: &SqliteStore,
    worktree_pool: &WorktreePool,
    execute: bool,
) -> Result<WorktreeCleanupPlan, CoreError> {
    let candidates = store.failed_worktree_cleanup_candidates().await?;
    let mut released = 0;
    if execute {
        for candidate in &candidates {
            WorktreeAllocator::release(worktree_pool, &candidate.key, &candidate.path).await?;
            store.mark_failed_worktree_released(candidate).await?;
            released += 1;
        }
    }
    Ok(WorktreeCleanupPlan {
        candidates,
        released,
        execute,
    })
}

/// The clear "a daemon already owns this port" message (P1 startup guard). One
/// helper so the detection path can't drift from any other reference to it.
fn already_running_msg(addr: SocketAddr) -> String {
    format!("a daemon is already running on {addr} — refusing to start a second instance")
}

/// Bind the daemon's TCP listener, mapping the already-running case to a clear
/// message (P1 startup guard). For TCP `bind` ITSELF is the race-free
/// live-vs-free detector: `AddrInUse` means a live listener already owns the
/// port (no probe, no TOCTOU). Other bind errors pass through as their string.
///
/// # Errors
/// [`already_running_msg`] on `AddrInUse`; the underlying error's text otherwise.
pub async fn bind_listener(addr: SocketAddr) -> Result<tokio::net::TcpListener, String> {
    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => Ok(listener),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Err(already_running_msg(addr)),
        Err(e) => Err(format!("cannot bind {addr}: {e}")),
    }
}

/// Boot the daemon: bind the listener FIRST (so an already-running instance
/// fails fast on the clear guard message before opening the store), then build
/// the production host and serve the HTTP/SSE surface. Logs the bound address
/// and any in-flight parked runs.
///
/// # Errors
/// Returns any bind/store/serve error as a boxed error.
pub async fn serve(config: DaemonConfig) -> Result<(), Box<dyn std::error::Error>> {
    validate_enterprise_security_environment()?;
    // Bind before any startup work: a second instance returns the guard message
    // without opening the SQLite store or doing recovery (fail-fast, P1).
    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = bind_listener(addr).await?;
    tracing::info!(%addr, "agentd daemon listening");

    let host = build_production_host(&config).await?;
    if let Ok(parked) = agentd_store::run_repo::count_in_flight(host.store().pool()).await {
        tracing::info!(
            parked,
            "in-flight runs awaiting events (resumable from checkpoint)"
        );
    }
    let media_dir = media_dir_for_db(&config.db_path);
    let content_store = production_content_store(&media_dir)?;
    let auth = config.auth_config();
    let supervisor_fleet = Arc::new(SqliteWorkerFleet::new(host.store().pool().clone()));
    let native_worker = AgentdWorker::new(host.store().clone());
    let worker_service = Arc::new(WorkerFleetService::new(
        supervisor_fleet,
        native_worker,
        content_store,
    ));
    let _supervisor = (*worker_service).clone().start();
    let fleet = SqliteWorkerFleet::new(host.store().pool().clone());
    let fleet = Arc::new(configure_worker_fleet_auth(fleet, auth.api_token.clone()));
    let host_store = host.store().clone();
    let app = if enterprise_security_enabled() {
        let verifier = enterprise_mtls_verifier().await?;
        build_router_with_worker_fleet_mtls(
            Arc::new(host),
            auth.clone(),
            MediaConfig::new(media_dir),
            fleet,
            verifier,
        )
    } else {
        build_router_with_worker_fleet(
            Arc::new(host),
            auth.clone(),
            MediaConfig::new(media_dir),
            fleet,
        )
    };
    let app = app.merge(daemon_native_runtime_router(
        &host_store,
        auth.api_token.clone(),
    ));
    if let Some(token) = auth.api_token {
        let app = app.merge(recovery_router(worker_service, token));
        axum::serve(listener, app).await?;
        return Ok(());
    }
    axum::serve(listener, app).await?;
    Ok(())
}

fn configure_worker_fleet_auth(
    fleet: SqliteWorkerFleet,
    fallback_token: Option<String>,
) -> SqliteWorkerFleet {
    let configured = parse_worker_fleet_auth_proofs(
        std::env::var("AGENTD_WORKER_FLEET_AUTH_PROOFS")
            .ok()
            .as_deref(),
    );
    if let Some(configured) = configured {
        return fleet.with_auth_proofs(configured);
    }
    match fallback_token {
        Some(token) => fleet.with_auth_proof(token),
        None => fleet,
    }
}

fn parse_worker_fleet_auth_proofs(value: Option<&str>) -> Option<Vec<String>> {
    value.map(|value| {
        value
            .split(',')
            .map(str::trim)
            .filter(|proof| !proof.is_empty())
            .map(str::to_owned)
            .collect()
    })
}

#[cfg(test)]
mod worker_fleet_auth_tests {
    use super::parse_worker_fleet_auth_proofs;

    #[test]
    fn parses_rotation_proofs_without_empty_entries() {
        assert_eq!(
            parse_worker_fleet_auth_proofs(Some(" old , ,new,, ")),
            Some(vec!["old".into(), "new".into()])
        );
        assert_eq!(
            parse_worker_fleet_auth_proofs(Some(" , ")),
            Some(Vec::new())
        );
        assert_eq!(parse_worker_fleet_auth_proofs(None), None);
    }
}

fn validate_enterprise_security_environment() -> Result<(), Box<dyn std::error::Error>> {
    if !enterprise_security_enabled() {
        return Ok(());
    }
    let config = EnterpriseSecurityConfig {
        trust_domain: std::env::var("AGENTD_SECURITY_TRUST_DOMAIN").unwrap_or_default(),
        sandbox_runtime: std::env::var("AGENTD_SECURITY_SANDBOX_RUNTIME").unwrap_or_default(),
        secret_broker: std::env::var("AGENTD_SECURITY_SECRET_BROKER").unwrap_or_default(),
    };
    config.validate().map_err(|error| {
        std::io::Error::other(format!(
            "enterprise security configuration rejected: {error}"
        ))
    })?;
    for variable in ["AGENTD_SECURITY_ROOT_CERT", "AGENTD_SECURITY_FINGERPRINTS"] {
        if std::env::var(variable)
            .ok()
            .is_none_or(|value| value.trim().is_empty())
        {
            return Err(format!("enterprise security configuration missing {variable}").into());
        }
    }
    Ok(())
}

fn enterprise_security_enabled() -> bool {
    std::env::var("AGENTD_ENTERPRISE_SECURITY")
        .ok()
        .is_some_and(|value| value.trim() == "1" || value.trim().eq_ignore_ascii_case("true"))
}

async fn enterprise_mtls_verifier()
-> Result<Arc<dyn MtlsWorkloadVerifier>, Box<dyn std::error::Error>> {
    let identity = WorkloadIdentityVerifier::new(std::env::var("AGENTD_SECURITY_TRUST_DOMAIN")?);
    for fingerprint in std::env::var("AGENTD_SECURITY_FINGERPRINTS")?.split(',') {
        if !fingerprint.trim().is_empty() {
            identity
                .trust_fingerprint(fingerprint.trim().to_string())
                .await;
        }
    }
    let verifier = PeerCertificateVerifier::new(identity);
    verifier
        .trust_root(tokio::fs::read(std::env::var("AGENTD_SECURITY_ROOT_CERT")?).await?)
        .await;
    Ok(Arc::new(verifier))
}
