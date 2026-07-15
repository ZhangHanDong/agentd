//! `agentctl` — CLI client for the agentd daemon. P0.1 ships `flow validate`;
//! `run`/`status`/etc. come in later phases.

#![warn(clippy::unwrap_used, clippy::panic)]

mod agent;
mod cli;
mod flow;
mod parity;
mod run;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match cli.cmd {
        cli::Cmd::Agent(agent_cmd) => agent::run(&agent_cmd),
        cli::Cmd::Flow(flow_cmd) => flow::run(&flow_cmd),
        cli::Cmd::Run(run_cmd) => run::run(&run_cmd),
        cli::Cmd::Parity(parity_cmd) => parity::run(&parity_cmd),
    }
}
