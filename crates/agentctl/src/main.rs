//! `agentctl` — CLI client for the agentd daemon.
//! P0.0: only `--version` works. Subcommands come in later phases.

#![warn(clippy::unwrap_used, clippy::panic)]

use clap::Parser;

/// agentd control CLI.
#[derive(Debug, Parser)]
#[command(name = "agentctl", version)]
struct Cli {
    /// Reserved for future subcommands.
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Debug, clap::Subcommand)]
enum Cmd {
    /// Placeholder; replaced in P0.1.
    Noop,
}

fn main() {
    let _cli = Cli::parse();
}
