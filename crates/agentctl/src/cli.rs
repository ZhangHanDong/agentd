//! Command-line surface for `agentctl`. P0.1 ships `flow validate`; more
//! subcommands (run, status, …) arrive in later phases.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// agentd control CLI.
#[derive(Debug, Parser)]
#[command(name = "agentctl", version)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Cmd,
}

#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Workflow (`.dot`) operations.
    #[command(subcommand)]
    Flow(FlowCmd),
}

#[derive(Debug, Subcommand)]
pub enum FlowCmd {
    /// Validate a workflow `.dot` file against the §2.7 rules.
    Validate(ValidateArgs),
}

#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Path to the `.dot` workflow file.
    pub path: PathBuf,
}
