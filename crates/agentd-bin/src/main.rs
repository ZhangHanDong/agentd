//! `agentd` daemon entrypoint (P0.9). Thin wrapper over `agentd_bin::daemon`:
//! parse the config, then boot + serve. The assembly lives in the lib so it is
//! integration-testable.

#![warn(clippy::unwrap_used, clippy::panic)]

use std::process::ExitCode;

use agentd_bin::DaemonConfig;
use agentd_bin::daemon;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let config = DaemonConfig::parse();
    // RUST_LOG (if set) wins; otherwise `--log-level` is the effective default.
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.log_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    match daemon::serve(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("agentd daemon error: {err}");
            ExitCode::FAILURE
        }
    }
}
