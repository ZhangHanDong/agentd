//! The daemon assembly (P0.9 9b): construct the production host, build the
//! HTTP/SSE router, discover in-flight parked runs on boot, and serve.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use agentd_core::CoreError;
use agentd_core::ports::WorktreeAllocator;
use agentd_store::{FailedWorktreeCleanupCandidate, SqliteStore};
use agentd_surface::host::RunHost;
use agentd_surface::http::{AppState, router};
use agentd_tmux::{Config, GitWorktreeProvider, TmuxBackend, TokioCommandRunner, WorktreePool};
use axum::Router;

use crate::agent_mcp_context::{McpStdioContextBackend, mcp_stdio_command_from_current_process};
use crate::cli::DaemonConfig;
use crate::clock::SystemClock;
use crate::host::ProductionRunHost;
use crate::mempal::OfflineMempal;

/// Build the HTTP/SSE router over a host (testable via `tower::oneshot`).
pub fn build_router(host: Arc<dyn RunHost>) -> Router {
    router(AppState { host })
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
    let backend = TmuxBackend::new(
        Arc::new(TokioCommandRunner::new()),
        PathBuf::from("tmux"),
        Config::default(),
    );
    let mcp_stdio_command = mcp_stdio_command_from_current_process(config)?;
    let backend = McpStdioContextBackend::new(Box::new(backend), mcp_stdio_command);
    Ok(ProductionRunHost::new(
        store,
        Box::new(backend),
        Box::new(TokioCommandRunner::new()),
        Box::new(OfflineMempal),
        Box::new(SystemClock),
        config.workflows_dir.clone(),
    )
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
    let app = build_router(Arc::new(host));
    axum::serve(listener, app).await?;
    Ok(())
}
