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
        Some(AgentdCommand::McpStdio) => match daemon::build_production_host(&cli.config).await {
            Ok(host) => {
                let stdin = BufReader::new(tokio::io::stdin());
                let stdout = tokio::io::stdout();
                agentd_bin::stdio_mcp::serve_json_lines(&host, stdin, stdout)
                    .await
                    .map_err(|err| Box::new(err) as Box<dyn std::error::Error>)
            }
            Err(err) => Err(Box::new(err)),
        },
        None => daemon::serve(cli.config).await.map(|()| ()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!("agentd daemon error: {err}");
            ExitCode::FAILURE
        }
    }
}
