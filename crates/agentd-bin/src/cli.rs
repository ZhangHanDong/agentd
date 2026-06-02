//! The `agentd` daemon's command-line configuration (P0.9 9b).

use std::path::PathBuf;

use clap::Parser;

/// The agentd workflow daemon.
#[derive(Debug, Parser)]
#[command(name = "agentd", version, about = "The agentd local workflow daemon")]
pub struct DaemonConfig {
    /// Path to the `SQLite` database (migrations apply on connect).
    #[arg(long, default_value = "agentd.db")]
    pub db_path: PathBuf,

    /// TCP port for the HTTP/SSE surface.
    #[arg(long, default_value_t = 8787)]
    pub port: u16,

    /// Directory holding the workflow `.dot` files.
    #[arg(long, default_value = "workflows")]
    pub workflows_dir: PathBuf,

    /// Tracing log level (`error`/`warn`/`info`/`debug`/`trace`).
    #[arg(long, default_value = "info")]
    pub log_level: String,
}
