//! `agentd` daemon entrypoint (P0.9). Thin wrapper over `agentd_bin::daemon`:
//! parse the config, then boot + serve. The assembly lives in the lib so it is
//! integration-testable.

#![warn(clippy::unwrap_used, clippy::panic)]

use std::process::ExitCode;

use agentd_bin::daemon;
use agentd_bin::{AgentdCli, AgentdCommand};
use clap::Parser;
use tokio::io::BufReader;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = AgentdCli::parse();
    // RUST_LOG (if set) wins; otherwise `--log-level` is the effective default.
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&cli.config.log_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    let result: Result<(), Box<dyn std::error::Error>> = match cli.command {
        Some(AgentdCommand::Doctor(args)) => run_doctor(&cli.config, args.repair).await,
        Some(AgentdCommand::CleanupWorktrees(args)) => {
            match daemon::cleanup_failed_worktrees_from_config(&cli.config, args.execute).await {
                Ok(plan) => {
                    if args.execute {
                        println!(
                            "released {} failed-run worktrees ({} candidates)",
                            plan.released,
                            plan.candidates.len()
                        );
                    } else {
                        println!(
                            "dry-run: {} failed-run worktrees would be released",
                            plan.candidates.len()
                        );
                        for candidate in &plan.candidates {
                            println!("{} {}", candidate.key, candidate.path.display());
                        }
                    }
                    Ok(())
                }
                Err(err) => Err(Box::new(err)),
            }
        }
        Some(AgentdCommand::MatrixBridgeOnce(args)) => {
            match agentd_bin::matrix_bridge::run_matrix_bridge_once(&cli.config, &args) {
                Ok(report) => {
                    println!(
                        "matrix-bridge-once: registered_rooms={} inbound_forwarded={} outbound_sent={} bot_command_replies_sent={} next_from_seq={}",
                        report.run.registered_rooms,
                        report.run.inbound_forwarded,
                        report.run.outbound_sent,
                        report.run.bot_command_replies_sent,
                        report.next_from_seq
                    );
                    Ok(())
                }
                Err(err) => Err(Box::new(err)),
            }
        }
        Some(AgentdCommand::MatrixClientBridgePreflight(args)) => {
            run_matrix_client_bridge_preflight_command(&cli.config, &args)
        }
        Some(AgentdCommand::MatrixClientBridgeService(args)) => {
            run_matrix_client_bridge_service_command(&cli.config, &args)
        }
        Some(AgentdCommand::McpStdio(args)) => {
            let stdin = BufReader::new(tokio::io::stdin());
            let stdout = tokio::io::stdout();
            let agent_id =
                agentd_bin::stdio_mcp::identity_from_cli_or_env(args.agent_id.as_deref());
            if let Some(proxy_url) = args.proxy_url {
                agentd_bin::stdio_mcp::serve_proxy_json_lines_with_identity(
                    &proxy_url,
                    stdin,
                    stdout,
                    agent_id.as_deref(),
                )
                .await
                .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            } else {
                match daemon::build_production_host(&cli.config).await {
                    Ok(host) => agentd_bin::stdio_mcp::serve_json_lines_with_identity(
                        &host,
                        stdin,
                        stdout,
                        agent_id.as_deref(),
                    )
                    .await
                    .map_err(|err| Box::new(err) as Box<dyn std::error::Error>),
                    Err(err) => Err(Box::new(err)),
                }
            }
        }
        None => daemon::serve(cli.config).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("agentd daemon error: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run_doctor(
    config: &agentd_bin::DaemonConfig,
    repair: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let store = agentd_store::SqliteStore::connect(&config.db_path).await?;
    let report = agentd_store::doctor::OperationalDoctor::new(store.pool().clone())
        .check()
        .await?;
    if repair {
        let remediation = agentd_store::doctor::OperationalDoctor::new(store.pool().clone())
            .remediate(report.checked_at, 30)
            .await?;
        println!("{}", serde_json::to_string_pretty(&remediation)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&report)?);
    }
    Ok(())
}

fn run_matrix_client_bridge_preflight_command(
    config: &agentd_bin::DaemonConfig,
    args: &agentd_bin::MatrixClientBridgePreflightArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    match agentd_bin::matrix_bridge::run_matrix_client_bridge_preflight(config, &args.service) {
        Ok(report) => {
            print_matrix_client_bridge_preflight_report(&report);
            Ok(())
        }
        Err(err) => Err(Box::new(err)),
    }
}

fn run_matrix_client_bridge_service_command(
    config: &agentd_bin::DaemonConfig,
    args: &agentd_bin::MatrixClientBridgeServiceArgs,
) -> Result<(), Box<dyn std::error::Error>> {
    match agentd_bin::matrix_bridge::run_matrix_sdk_bridge_service(config, args) {
        Ok(report) => {
            print_matrix_client_bridge_service_report(&report);
            Ok(())
        }
        Err(err) => Err(Box::new(err)),
    }
}

fn print_matrix_client_bridge_preflight_report(
    report: &agentd_bin::matrix_bridge::MatrixClientBridgePreflightReport,
) {
    let versions = if report.homeserver.versions.is_empty() {
        "none".to_owned()
    } else {
        report.homeserver.versions.join(",")
    };
    let whoami_user_id = report
        .homeserver
        .whoami_user_id
        .as_deref()
        .unwrap_or("not_checked");
    println!(
        "matrix-client-bridge-preflight: homeserver={} versions={} whoami_user_id={} iterations={} puppet_accounts_configured={}",
        report.homeserver.homeserver_url,
        versions,
        whoami_user_id,
        report.iterations,
        report.puppet_accounts_configured
    );
}

fn print_matrix_client_bridge_service_report(
    report: &agentd_bin::matrix_bridge::MatrixClientBridgeServiceReport,
) {
    let registered_rooms = report
        .iterations
        .iter()
        .map(|iteration| iteration.run.registered_rooms)
        .sum::<usize>();
    let inbound_forwarded = report
        .iterations
        .iter()
        .map(|iteration| iteration.run.inbound_forwarded)
        .sum::<usize>();
    let outbound_sent = report
        .iterations
        .iter()
        .map(|iteration| iteration.run.outbound_sent)
        .sum::<usize>();
    println!(
        "matrix-client-bridge-service: iterations={} registered_rooms={} inbound_forwarded={} outbound_sent={} bot_command_replies_sent={} next_from_seq={}",
        report.iterations.len(),
        registered_rooms,
        inbound_forwarded,
        outbound_sent,
        report.bot_command_replies_sent,
        report.next_from_seq
    );
}
