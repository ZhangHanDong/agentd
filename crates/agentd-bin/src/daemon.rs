//! The daemon assembly (P0.9 9b): construct the production host, build the
//! HTTP/SSE router, discover in-flight parked runs on boot, and serve.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use agentd_core::CoreError;
use agentd_store::SqliteStore;
use agentd_surface::host::RunHost;
use agentd_surface::http::{AppState, router};
use agentd_tmux::{Config, TmuxBackend, TokioCommandRunner};
use axum::Router;

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
    let backend = TmuxBackend::new(
        Arc::new(TokioCommandRunner::new()),
        PathBuf::from("tmux"),
        Config::default(),
    );
    Ok(ProductionRunHost::new(
        store,
        Box::new(backend),
        Box::new(TokioCommandRunner::new()),
        Box::new(OfflineMempal),
        Box::new(SystemClock),
        config.workflows_dir.clone(),
    ))
}

/// Boot the daemon: build the production host, bind the listener, and serve the
/// HTTP/SSE surface. Logs the bound address and any in-flight parked runs.
///
/// # Errors
/// Returns any store/bind/serve error as a boxed error.
pub async fn serve(config: DaemonConfig) -> Result<(), Box<dyn std::error::Error>> {
    let host = build_production_host(&config).await?;
    if let Ok(parked) = agentd_store::run_repo::count_in_flight(host.store().pool()).await {
        tracing::info!(
            parked,
            "in-flight runs awaiting events (resumable from checkpoint)"
        );
    }
    let app = build_router(Arc::new(host));
    let addr = SocketAddr::from(([127, 0, 0, 1], config.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "agentd daemon listening");
    axum::serve(listener, app).await?;
    Ok(())
}
