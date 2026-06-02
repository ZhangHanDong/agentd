//! `agentd` daemon entrypoint (P0.9). Thin wrapper over `agentd_bin::daemon`:
//! parse the config, then boot + serve. The assembly lives in the lib so it is
//! integration-testable.

#![warn(clippy::unwrap_used, clippy::panic)]

use std::process::ExitCode;

use agentd_bin::DaemonConfig;
use agentd_bin::daemon;
use clap::Parser;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt::init();
    let config = DaemonConfig::parse();
    match daemon::serve(config).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("agentd daemon error: {err}");
            ExitCode::FAILURE
        }
    }
}
