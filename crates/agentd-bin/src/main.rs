//! `agentd` daemon entrypoint. P0.0 stub — boots the tokio runtime and exits.
//! Real wiring (engine, adapters, surfaces) arrives starting in P0.1.

#![warn(clippy::unwrap_used, clippy::panic)]

fn main() -> std::process::ExitCode {
    tracing_subscriber::fmt::init();
    tracing::info!("agentd P0.0 stub: nothing to do, exiting.");
    std::process::ExitCode::SUCCESS
}
