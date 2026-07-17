//! The daemon assembly (P0.9 9b): construct the production host, build the
//! HTTP/SSE router, discover in-flight parked runs on boot, and serve.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agentd_core::CoreError;
use agentd_core::ports::{
    AgentBackend, PolicyRevocationPort, SecurityError, WorktreeAllocator,
};
use agentd_core::types::{AgentHandle, SecurityEpochRequest, SecurityEpochStatus, SpawnRequest};
use agentd_store::fleet_scheduler::SqliteFleetScheduler;
use agentd_store::{
    FailedWorktreeCleanupCandidate, SqliteEnterpriseScaleControlPlane, SqliteStore,
};
use agentd_surface::host::RunHost;
use agentd_surface::http::{AppState, AuthConfig, MediaConfig, SchedulerConfig, router};
use agentd_security::{ContentRedactor, RedactionLimits};
use agentd_worktree::{GitWorktreeProvider, WorktreePool};
use axum::Router;

use crate::agent_mcp_context::{McpStdioContextBackend, mcp_stdio_command_from_current_process};
use crate::cli::DaemonConfig;
use crate::clock::SystemClock;
use crate::command_runner::TokioCommandRunner;
use crate::host::ProductionRunHost;
use crate::enterprise::{
    EnterpriseControlPlaneConfig, EnterpriseLeadershipGate, start_enterprise_coordination,
};
use crate::native_backend::{
    LocalInteractiveSandbox, NativeAgentBackend, NativeAgentLifecycle,
    StandalonePolicyRevocation,
};
use crate::mempal::OfflineMempal;
use crate::runtime::{NativeRuntimeCompositionConfig, compose_native_runtime};
use crate::security::{
    SecurityRuntimeMode, build_security_runtime, validate_enterprise_control_plane_auth,
};

#[derive(Debug)]
struct ControlPlaneOnlyBackend;

#[async_trait::async_trait]
impl AgentBackend for ControlPlaneOnlyBackend {
    async fn spawn(&self, _request: SpawnRequest) -> Result<AgentHandle, CoreError> {
        Err(CoreError::Backend(
            "enterprise control-plane-only process cannot execute local agents".to_string(),
        ))
    }
}

#[derive(Debug)]
struct FailClosedPolicyRevocation;

#[async_trait::async_trait]
impl PolicyRevocationPort for FailClosedPolicyRevocation {
    async fn check_security_epoch(
        &self,
        _request: &SecurityEpochRequest,
    ) -> Result<SecurityEpochStatus, SecurityError> {
        Err(SecurityError::Unavailable(
            "enterprise policy revocation provider is not configured".to_string(),
        ))
    }
}

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

/// Production media root colocated with the daemon database.
#[must_use]
pub fn media_dir_for_db(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join("media")
}

/// Construct the production host from a real `SqliteStore`, the real
/// native PTY runtime, the offline mempal, and the system clock.
///
/// # Errors
/// [`CoreError`] if the store cannot be opened/migrated.
pub async fn build_production_host(config: &DaemonConfig) -> Result<ProductionRunHost, CoreError> {
    if config.security_mode == SecurityRuntimeMode::Enterprise {
        return Err(CoreError::Invariant(
            "enterprise mode cannot construct the standalone compatibility host".to_string(),
        ));
    }
    let store = SqliteStore::connect(&config.db_path).await?;
    std::fs::create_dir_all(&config.worktree_base)?;
    let worktree_pool = WorktreePool::new(Arc::new(GitWorktreeProvider::new(
        config.repo_dir.clone(),
        config.worktree_base.clone(),
    )));
    gc_worktrees_on_boot(&store, &worktree_pool).await?;
    let transcript_root = config
        .db_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("runtime-transcripts");
    let sandbox = Arc::new(LocalInteractiveSandbox::new([
        config.repo_dir.clone(),
        config.worktree_base.clone(),
    ])?);
    let redactor = Arc::new(
        ContentRedactor::compile(
            Vec::new(),
            vec![
                r"(?i)(api[_-]?key|token|secret|password)[[:space:]]*[:=][[:space:]]*[^[:space:]]+"
                    .to_string(),
            ],
            RedactionLimits {
                max_input_bytes: 1024 * 1024,
                max_output_bytes: 1024 * 1024,
            },
        )
        .map_err(|error| CoreError::Backend(format!("runtime redaction policy: {error}")))?,
    );
    let runtime_service = compose_native_runtime(
        &store,
        sandbox,
        redactor,
        Arc::new(StandalonePolicyRevocation),
        Arc::new(SystemClock),
        &NativeRuntimeCompositionConfig {
            transcript_root,
            max_transcript_object_bytes: 64 * 1024 * 1024,
        },
    )
    .map_err(|error| CoreError::Backend(error.to_string()))?;
    let recovered = runtime_service
        .recover_startup(1_000)
        .await
        .map_err(|error| CoreError::Backend(error.to_string()))?;
    tracing::info!(sessions = recovered.len(), "native runtime startup recovery complete");
    let native_backend = Arc::new(NativeAgentBackend::new(
        store.clone(),
        Arc::clone(&runtime_service),
        format!("agentd-{}", std::process::id()),
    ));
    let mcp_stdio_command = mcp_stdio_command_from_current_process(config)?;
    let backend = McpStdioContextBackend::new(
        Box::new((*native_backend).clone()),
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
    .with_agent_lifecycle(Box::new(NativeAgentLifecycle::new(native_backend)))
    .with_native_runtime(runtime_service)
    .with_tool_cwd(config.repo_dir.clone())
    .with_worktree_allocator(Some(Box::new(worktree_pool))))
}

async fn build_enterprise_control_plane_host(
    config: &DaemonConfig,
    store: SqliteStore,
    leadership: EnterpriseLeadershipGate,
) -> Result<ProductionRunHost, CoreError> {
    let scale = Arc::new(SqliteEnterpriseScaleControlPlane::new(store.pool().clone()));
    let fleet = Arc::new(SqliteFleetScheduler::new(
        store.pool().clone(),
        Arc::new(FailClosedPolicyRevocation),
    ));
    Ok(ProductionRunHost::new(
        store,
        Box::new(ControlPlaneOnlyBackend),
        Box::new(TokioCommandRunner::new()),
        Box::new(OfflineMempal),
        Box::new(SystemClock),
        config.workflows_dir.clone(),
    )
    .with_enterprise_control_plane(scale, fleet, leadership)
    .with_tool_cwd(config.repo_dir.clone()))
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

/// Boot the daemon: validate the security composition, then bind the listener
/// before opening the store. Enterprise mode has no compatibility listener and
/// fails before bind until all closed providers are explicitly selected.
///
/// # Errors
/// Returns any bind/store/serve error as a boxed error.
pub async fn serve(config: DaemonConfig) -> Result<(), Box<dyn std::error::Error>> {
    let auth = config.auth_config();
    if config.security_mode == SecurityRuntimeMode::Enterprise
        && config.enterprise.enterprise_control_plane_only
    {
        validate_enterprise_control_plane_auth(&auth)?;
        EnterpriseControlPlaneConfig::from_daemon(&config)?;
        return serve_enterprise_control_plane(config, auth).await;
    }
    build_security_runtime(config.security_mode, &auth, None)?;
    // After the pure security gate, bind before store/recovery work so the P1
    // already-running guard remains fail-fast in standalone mode.
    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = bind_listener(addr).await?;
    tracing::info!(%addr, "agentd daemon listening");

    let host = build_production_host(&config).await?;
    if let Some(runtime) = host.native_runtime_service() {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                match runtime.reap_idle().await {
                    Ok(reports) if !reports.is_empty() => {
                        tracing::info!(count = reports.len(), "reaped idle native runtimes");
                    }
                    Ok(_) => {}
                    Err(error) => {
                        tracing::error!(%error, "native runtime idle reaper failed");
                    }
                }
            }
        });
    }
    if let Ok(parked) = agentd_store::run_repo::count_in_flight(host.store().pool()).await {
        tracing::info!(
            parked,
            "in-flight runs awaiting events (resumable from checkpoint)"
        );
    }
    let media_dir = media_dir_for_db(&config.db_path);
    let app = build_router_with_auth_and_media(Arc::new(host), auth, MediaConfig::new(media_dir));
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn serve_enterprise_control_plane(
    config: DaemonConfig,
    auth: AuthConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let bind_ip = config
        .enterprise
        .enterprise_bind_address
        .as_deref()
        .unwrap_or("127.0.0.1")
        .parse::<std::net::IpAddr>()?;
    let addr = SocketAddr::from((bind_ip, config.port));
    let listener = bind_listener(addr).await?;
    let store = SqliteStore::connect(&config.db_path).await?;
    let coordinator = start_enterprise_coordination(&config, &store)
        .await?
        .ok_or_else(|| std::io::Error::other("enterprise coordinator was not constructed"))?;
    let host = build_enterprise_control_plane_host(
        &config,
        store,
        coordinator.leadership_gate(),
    )
    .await?;
    let media_dir = media_dir_for_db(&config.db_path);
    let app = build_router_with_auth_and_media(Arc::new(host), auth, MediaConfig::new(media_dir));
    tracing::info!(%addr, "enterprise agentd control plane listening");
    let result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await;
    coordinator.shutdown().await;
    result?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let terminate = tokio::signal::unix::SignalKind::terminate();
        match tokio::signal::unix::signal(terminate) {
            Ok(mut signal) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = signal.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
