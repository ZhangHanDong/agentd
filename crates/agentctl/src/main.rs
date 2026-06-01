//! `agentctl` — CLI client for the agentd daemon. P0.1 ships `flow validate`;
//! `run`/`status`/etc. come in later phases.

#![warn(clippy::unwrap_used, clippy::panic)]

mod cli;
mod flow;
mod run;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match cli.cmd {
        cli::Cmd::Flow(flow_cmd) => flow::run(&flow_cmd),
        cli::Cmd::Run(run_cmd) => run::run(&run_cmd),
    }
}
