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
    /// `spike.dot` — exploratory throwaway (no gate/review/PR).
    Spike,
    /// `docs-only.dot` — a docs change (linear, no review).
    DocsOnly,
    /// `bugfix-rapid.dot` — a fast fix (keeps the gate, skips review).
    BugfixRapid,
    /// `refactor-only.dot` — behavior-preserving (keeps gate + review).
    RefactorOnly,
    /// `bootstrap.dot` — derive a starter spec from an existing codebase.
    Bootstrap,
}

impl Flow {
    /// The workflow file name for this flow.
    #[must_use]
    pub fn file_name(self) -> &'static str {
        match self {
            Self::Draft => "draft.dot",
            Self::Execute => "execute.dot",
            Self::Spike => "spike.dot",
            Self::DocsOnly => "docs-only.dot",
            Self::BugfixRapid => "bugfix-rapid.dot",
            Self::RefactorOnly => "refactor-only.dot",
            Self::Bootstrap => "bootstrap.dot",
        }
    }

    /// The flow's wire name for the `POST /runs` body — the file stem, identical
    /// to the daemon's `flow_to_file` arm (the flow triple's shared string).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Execute => "execute",
            Self::Spike => "spike",
            Self::DocsOnly => "docs-only",
            Self::BugfixRapid => "bugfix-rapid",
            Self::RefactorOnly => "refactor-only",
            Self::Bootstrap => "bootstrap",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Flow;
    use clap::ValueEnum;
    use std::path::PathBuf;

    #[test]
    fn cli_flow_variants_map_to_existing_files() {
        let wf = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows");
        for flow in Flow::value_variants() {
            let file = flow.file_name();
            assert!(wf.join(file).exists(), "Flow file '{file}' must exist");
            // name() is the file stem — the wire string shared with the daemon's
            // flow_to_file arm; this keeps the flow triple from drifting.
            assert_eq!(
                format!("{}.dot", flow.name()),
                file,
                "name() + .dot must equal file_name()"
            );
        }
    }
}

#[derive(Debug, Args)]
pub struct RunStartArgs {
    /// Which standalone workflow to run (draft, execute, spike, docs-only,
    /// bugfix-rapid, refactor-only, bootstrap).
    #[arg(long, value_enum)]
    pub flow: Flow,
    /// The run id — an issue id (draft / bugfix-rapid / docs-only / spike), a
    /// frozen-spec id (execute / refactor-only), or a repo label (bootstrap).
    pub id: String,
    /// Optional run-context file for the run.
    #[arg(long)]
    pub context_file: Option<PathBuf>,
    /// Directory holding the workflow `.dot` files.
    #[arg(long, default_value = "workflows")]
    pub workflows_dir: PathBuf,
    /// The agentd daemon base URL for a live run (ignored by `--dry-run`).
    #[arg(long, default_value = "http://127.0.0.1:8787")]
    pub daemon_url: String,
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
