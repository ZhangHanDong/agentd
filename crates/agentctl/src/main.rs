//! `agentctl` — CLI client for the agentd daemon. P0.1 ships `flow validate`;
//! `run`/`status`/etc. come in later phases.

#![warn(clippy::unwrap_used, clippy::panic)]

mod agent;
mod cli;
mod cutover;
mod enterprise;
mod flow;
mod parity;
mod run;
mod runtime;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match cli.cmd {
        cli::Cmd::Cutover(cutover_cmd) => cutover::run(&cutover_cmd),
        cli::Cmd::Agent(agent_cmd) => agent::run(&agent_cmd),
        cli::Cmd::Flow(flow_cmd) => flow::run(&flow_cmd),
        cli::Cmd::Run(run_cmd) => run::run(&run_cmd),
        cli::Cmd::Parity(parity_cmd) => parity::run(&parity_cmd),
        cli::Cmd::Runtime(runtime_cmd) => runtime::run(&runtime_cmd),
        cli::Cmd::Enterprise(enterprise_cmd) => enterprise::run(&enterprise_cmd),
    }
}
