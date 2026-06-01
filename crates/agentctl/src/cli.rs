//! Command-line surface for `agentctl`. P0.1 ships `flow validate`; P0.8 adds
//! `run start` (the standalone Path-B trigger); more subcommands arrive later.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

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
    /// Run operations (start a standalone Path-B workflow run).
    #[command(subcommand)]
    Run(RunCmd),
}

#[derive(Debug, Subcommand)]
pub enum RunCmd {
    /// Start a workflow run from a local issue/spec (standalone, Path B).
    Start(RunStartArgs),
}

/// Which standalone Path-B workflow to run.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Flow {
    /// `draft.dot` — issue → spec draft.
    Draft,
    /// `execute.dot` — frozen spec → PR.
    Execute,
}

impl Flow {
    /// The workflow file name for this flow.
    #[must_use]
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Draft => "draft.dot",
            Self::Execute => "execute.dot",
        }
    }
}

#[derive(Debug, Args)]
pub struct RunStartArgs {
    /// Which standalone workflow to run.
    #[arg(long, value_enum)]
    pub flow: Flow,
    /// The issue id (draft) or frozen-spec id (execute) for this run.
    pub id: String,
    /// Optional run-context file for the run.
    #[arg(long)]
    pub context_file: Option<PathBuf>,
    /// Directory holding the workflow `.dot` files.
    #[arg(long, default_value = "workflows")]
    pub workflows_dir: PathBuf,
    /// Validate + print the resolved plan without launching a live run.
    #[arg(long)]
    pub dry_run: bool,
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
