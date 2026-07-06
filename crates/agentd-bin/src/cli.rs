//! The `agentd` daemon's command-line configuration (P0.9 9b).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// Top-level `agentd` CLI.
#[derive(Debug, Parser)]
#[command(name = "agentd", version, about = "The agentd local workflow daemon")]
pub struct AgentdCli {
    /// Shared daemon/store/worktree configuration.
    #[command(flatten)]
    pub config: DaemonConfig,

    /// Optional maintenance command. Omitted means "serve the daemon".
    #[command(subcommand)]
    pub command: Option<AgentdCommand>,
}

/// Offline maintenance commands.
#[derive(Debug, Subcommand)]
pub enum AgentdCommand {
    /// List or release failed-run worktrees.
    CleanupWorktrees(CleanupWorktreesArgs),
    /// Serve the agent-facing MCP dispatcher over line-delimited stdio JSON-RPC.
    McpStdio,
}

/// Arguments for `agentd cleanup-worktrees`.
#[derive(Debug, Args)]
pub struct CleanupWorktreesArgs {
    /// Actually release failed-run worktrees. Without this flag, only prints a dry-run plan.
    #[arg(long)]
    pub execute: bool,
}

/// The agentd workflow daemon.
#[derive(Debug, Args)]
pub struct DaemonConfig {
    /// Path to the `SQLite` database (migrations apply on connect).
    #[arg(long, default_value = "agentd.db", global = true)]
    pub db_path: PathBuf,

    /// TCP port for the HTTP/SSE surface.
    #[arg(long, default_value_t = 8787, global = true)]
    pub port: u16,

    /// Directory holding the workflow `.dot` files.
    #[arg(long, default_value = "workflows", global = true)]
    pub workflows_dir: PathBuf,

    /// Git repository root used for per-task_run worktree allocation.
    #[arg(long, default_value = ".", global = true)]
    pub repo_dir: PathBuf,

    /// Directory where agentd creates disposable git worktrees.
    #[arg(long, default_value = ".agentd/worktrees", global = true)]
    pub worktree_base: PathBuf,

    /// Tracing log level (`error`/`warn`/`info`/`debug`/`trace`).
    #[arg(long, default_value = "info", global = true)]
    pub log_level: String,
}

#[cfg(test)]
mod tests {
    use super::{AgentdCli, AgentdCommand};
    use clap::Parser;
    use clap::error::ErrorKind;
    use std::path::PathBuf;

    #[test]
    fn agentd_cli_cleanup_worktrees_is_dry_run_by_default() {
        let cli = AgentdCli::parse_from(["agentd", "cleanup-worktrees"]);
        let Some(AgentdCommand::CleanupWorktrees(args)) = cli.command else {
            panic!("expected cleanup-worktrees command");
        };
        assert!(!args.execute, "cleanup-worktrees is dry-run by default");
    }

    #[test]
    fn agentd_cli_without_subcommand_uses_daemon_defaults() {
        let cli = AgentdCli::try_parse_from(["agentd"]).expect("daemon defaults parse");

        assert!(
            cli.command.is_none(),
            "omitted subcommand serves the daemon"
        );
        assert_eq!(PathBuf::from("agentd.db"), cli.config.db_path);
        assert_eq!(8787, cli.config.port);
        assert_eq!(PathBuf::from(".agentd/worktrees"), cli.config.worktree_base);
    }

    #[test]
    fn agentd_cli_cleanup_accepts_shared_options_before_subcommand() {
        let cli = AgentdCli::try_parse_from([
            "agentd",
            "--db-path",
            "state/agentd.db",
            "--repo-dir",
            "/tmp/repo",
            "--worktree-base",
            "/tmp/wt",
            "cleanup-worktrees",
            "--execute",
        ])
        .expect("shared options before cleanup-worktrees parse");
        let Some(AgentdCommand::CleanupWorktrees(args)) = cli.command else {
            panic!("expected cleanup-worktrees command");
        };

        assert!(args.execute);
        assert_eq!(PathBuf::from("state/agentd.db"), cli.config.db_path);
        assert_eq!(PathBuf::from("/tmp/repo"), cli.config.repo_dir);
        assert_eq!(PathBuf::from("/tmp/wt"), cli.config.worktree_base);
    }

    #[test]
    fn agentd_cli_cleanup_accepts_shared_options_after_subcommand() {
        let cli = AgentdCli::try_parse_from([
            "agentd",
            "cleanup-worktrees",
            "--db-path",
            "state/agentd.db",
            "--repo-dir",
            "/tmp/repo",
            "--worktree-base",
            "/tmp/wt",
            "--execute",
        ])
        .expect("shared options after cleanup-worktrees parse");
        let Some(AgentdCommand::CleanupWorktrees(args)) = cli.command else {
            panic!("expected cleanup-worktrees command");
        };

        assert!(args.execute);
        assert_eq!(PathBuf::from("state/agentd.db"), cli.config.db_path);
        assert_eq!(PathBuf::from("/tmp/repo"), cli.config.repo_dir);
        assert_eq!(PathBuf::from("/tmp/wt"), cli.config.worktree_base);
    }

    #[test]
    fn agentd_cli_rejects_unknown_subcommand() {
        let err = AgentdCli::try_parse_from(["agentd", "cleanup-worktree"])
            .expect_err("unknown maintenance command is rejected");

        assert_eq!(ErrorKind::InvalidSubcommand, err.kind());
    }

    #[test]
    fn agentd_cli_mcp_stdio_accepts_shared_options() {
        let cli = AgentdCli::try_parse_from([
            "agentd",
            "--db-path",
            "state.db",
            "--workflows-dir",
            "workflows",
            "mcp-stdio",
        ])
        .expect("mcp-stdio parses");

        assert!(matches!(cli.command, Some(AgentdCommand::McpStdio)));
        assert_eq!(PathBuf::from("state.db"), cli.config.db_path);
        assert_eq!(PathBuf::from("workflows"), cli.config.workflows_dir);
    }
}
