//! The daemon assembly (P0.9 9b): construct the production host, build the
//! HTTP/SSE router, discover in-flight parked runs on boot, and serve.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentd_core::CoreError;
use agentd_core::ports::{AgentAllocation, AgentBackend, WorkerFleetPort, WorktreeAllocator};
use agentd_core::types::{AgentHandle, SpawnRequest};
use agentd_store::worker_fleet::SqliteWorkerFleet;
use agentd_store::{FailedWorktreeCleanupCandidate, SqliteStore};
use agentd_surface::host::RunHost;
use agentd_surface::http::{AppState, AuthConfig, MediaConfig, SchedulerConfig, router};
use agentd_surface::worker_fleet_http::worker_fleet_router;
use agentd_tmux::{
    Config, GitWorktreeProvider, ShutdownMethod, ShutdownOpts, TmuxBackend, TokioCommandRunner,
    WorktreePool,
};
use axum::Router;

use crate::agent_mcp_context::{McpStdioContextBackend, mcp_stdio_command_from_current_process};
use crate::cli::DaemonConfig;
use crate::clock::SystemClock;
use crate::host::{
    AgentLifecycle, AgentLifecycleShutdown, AgentLifecycleShutdownReport, ProductionRunHost,
};
use crate::mempal::OfflineMempal;
use crate::native_worker::{AgentdWorker, NativeRecoveryRegistry};

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
    build_router_with_auth_and_media(host, auth, media).merge(worker_fleet_router(fleet))
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
}

impl WorkerFleetService {
    #[must_use]
    pub fn new(fleet: Arc<dyn WorkerFleetPort>, native_worker: AgentdWorker) -> Self {
        Self {
            fleet,
            recovery_registry: Arc::new(NativeRecoveryRegistry::new()),
            native_worker,
        }
    }

    pub fn register_recovery(&self, request: crate::native_worker::NativeRecoveryRequest) {
        self.recovery_registry.register(request);
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
    let auth = config.auth_config();
    let supervisor_fleet = Arc::new(SqliteWorkerFleet::new(host.store().pool().clone()));
    let native_worker = AgentdWorker::new(host.store().clone());
    let _supervisor = WorkerFleetService::new(supervisor_fleet, native_worker).start();
    let fleet = SqliteWorkerFleet::new(host.store().pool().clone());
    let fleet = match auth.api_token.clone() {
        Some(token) => Arc::new(fleet.with_auth_proof(token)),
        None => Arc::new(fleet),
    };
    let app =
        build_router_with_worker_fleet(Arc::new(host), auth, MediaConfig::new(media_dir), fleet);
    axum::serve(listener, app).await?;
    Ok(())
}
