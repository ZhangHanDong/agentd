//! The `agentd` daemon's command-line configuration (P0.9 9b).

use std::collections::BTreeMap;
use std::path::PathBuf;

use agentd_surface::http::{AgentTokenMode, AuthConfig};
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
#[allow(clippy::large_enum_variant)]
pub enum AgentdCommand {
    /// List or release failed-run worktrees.
    CleanupWorktrees(CleanupWorktreesArgs),
    /// Validate Matrix client bridge service configuration and homeserver reachability.
    MatrixClientBridgePreflight(MatrixClientBridgePreflightArgs),
    /// Run a bounded SDK-facing Matrix client bridge service loop.
    MatrixClientBridgeService(MatrixClientBridgeServiceArgs),
    /// Run one deterministic Matrix bridge iteration from local JSON fixture files.
    MatrixBridgeOnce(MatrixBridgeOnceArgs),
    /// Serve the agent-facing MCP dispatcher over line-delimited stdio JSON-RPC.
    McpStdio(McpStdioArgs),
}

/// Arguments for `agentd cleanup-worktrees`.
#[derive(Debug, Args)]
pub struct CleanupWorktreesArgs {
    /// Actually release failed-run worktrees. Without this flag, only prints a dry-run plan.
    #[arg(long)]
    pub execute: bool,
}

/// Arguments for `agentd mcp-stdio`.
#[derive(Debug, Args)]
pub struct McpStdioArgs {
    /// Proxy tools/call requests to a running central daemon instead of advancing locally.
    #[arg(long)]
    pub proxy_url: Option<String>,

    /// Bind this stdio MCP session to one agent identity.
    #[arg(long)]
    pub agent_id: Option<String>,
}

/// Arguments for `agentd matrix-bridge-once`.
#[derive(Debug, Args)]
pub struct MatrixBridgeOnceArgs {
    /// Agentd HTTP API base URL.
    #[arg(long)]
    pub agentd_api: String,

    /// JSON state file containing the bridge `nextFromSeq` cursor.
    #[arg(long)]
    pub state: PathBuf,

    /// JSON array of Matrix room registrations.
    #[arg(long)]
    pub rooms_json: PathBuf,

    /// JSON array of inbound Matrix events.
    #[arg(long)]
    pub inbound_json: PathBuf,

    /// JSONL file where sent Matrix outbound messages are appended.
    #[arg(long)]
    pub sent_log_jsonl: PathBuf,

    /// Matrix homeserver URL for optional puppet account provisioning.
    #[arg(long)]
    pub matrix_homeserver_url: Option<String>,

    /// Local Matrix server name used when deriving puppet MXIDs.
    #[arg(long)]
    pub matrix_server_name: Option<String>,

    /// Localpart prefix for Matrix puppet accounts.
    #[arg(long, default_value = "ac_")]
    pub matrix_agent_prefix: String,

    /// Known agent name eligible for a Matrix puppet account. Repeatable.
    #[arg(long = "matrix-agent")]
    pub matrix_agents: Vec<String>,

    /// Known agent name to exclude from Matrix puppet account provisioning. Repeatable.
    #[arg(long = "matrix-skip-agent")]
    pub matrix_skip_agents: Vec<String>,

    /// Agent-chat-style bridge-state JSON file storing puppet `agentTokens`.
    #[arg(long)]
    pub matrix_puppet_state: Option<PathBuf>,

    /// Secret used to derive Matrix puppet account passwords.
    #[arg(long)]
    pub matrix_agent_password_secret: Option<String>,

    /// Legacy Matrix puppet password template used only when legacy fallback is enabled.
    #[arg(long)]
    pub matrix_agent_password_template: Option<String>,

    /// Allow the legacy Matrix puppet password template as a fallback.
    #[arg(long)]
    pub matrix_allow_legacy_agent_password: bool,

    /// Matrix registration token for puppet account UIA registration.
    #[arg(long)]
    pub matrix_registration_token: Option<String>,
}

/// Arguments for `agentd matrix-client-bridge-service`.
#[derive(Debug, Args)]
pub struct MatrixClientBridgeServiceArgs {
    /// Agentd HTTP API base URL.
    #[arg(long)]
    pub agentd_api: String,

    /// JSON state file containing the bridge `nextFromSeq` cursor.
    #[arg(long)]
    pub state: PathBuf,

    /// Positive bounded iteration count for this service run.
    #[arg(long, default_value_t = 1)]
    pub iterations: usize,

    /// Matrix homeserver URL for the SDK client and optional puppet account provisioning.
    #[arg(long)]
    pub matrix_homeserver_url: Option<String>,

    /// Matrix username for password login.
    #[arg(long)]
    pub matrix_username: Option<String>,

    /// Matrix password for password login.
    #[arg(long)]
    pub matrix_password: Option<String>,

    /// Matrix user id for access-token session restore.
    #[arg(long)]
    pub matrix_user_id: Option<String>,

    /// Matrix device id for access-token session restore.
    #[arg(long)]
    pub matrix_device_id: Option<String>,

    /// Matrix access token for session restore.
    #[arg(long)]
    pub matrix_access_token: Option<String>,

    /// Timeout for one SDK sync request in milliseconds.
    #[arg(long, default_value_t = 0)]
    pub matrix_sync_timeout_ms: u64,

    /// Optional Matrix SDK `SQLite` store directory.
    #[arg(long)]
    pub matrix_sdk_store: Option<PathBuf>,

    /// Matrix bot user id override for loop suppression.
    #[arg(long)]
    pub matrix_bot_user_id: Option<String>,

    /// Local Matrix server name used when deriving puppet MXIDs.
    #[arg(long)]
    pub matrix_server_name: Option<String>,

    /// Localpart prefix for Matrix puppet accounts.
    #[arg(long, default_value = "ac_")]
    pub matrix_agent_prefix: String,

    /// Known agent name eligible for Matrix target resolution and puppet accounts. Repeatable.
    #[arg(long = "matrix-agent")]
    pub matrix_agents: Vec<String>,

    /// Known agent name to exclude from Matrix puppet account provisioning. Repeatable.
    #[arg(long = "matrix-skip-agent")]
    pub matrix_skip_agents: Vec<String>,

    /// Matrix trust mode for untrusted invites: audit or enforce.
    #[arg(long, default_value = "audit", value_parser = ["audit", "enforce"])]
    pub matrix_trust_mode: String,

    /// Matrix MXID allowed to invite the bridge into trusted rooms. Repeatable.
    #[arg(long = "matrix-trusted-inviter")]
    pub matrix_trusted_inviters: Vec<String>,

    /// Matrix MXID that should never be forwarded into agentd. Repeatable.
    #[arg(long = "matrix-ignore-sender")]
    pub matrix_ignored_senders: Vec<String>,

    /// Matrix MXID allowed to run non-admin bot commands. Repeatable.
    #[arg(long = "matrix-operator")]
    pub matrix_operator_mxids: Vec<String>,

    /// Matrix MXID allowed to run admin bot commands. Repeatable.
    #[arg(long = "matrix-admin")]
    pub matrix_admin_mxids: Vec<String>,

    /// Agent-chat-style bridge-state JSON file storing puppet `agentTokens`.
    #[arg(long)]
    pub matrix_puppet_state: Option<PathBuf>,

    /// Secret used to derive Matrix puppet account passwords.
    #[arg(long)]
    pub matrix_agent_password_secret: Option<String>,

    /// Legacy Matrix puppet password template used only when legacy fallback is enabled.
    #[arg(long)]
    pub matrix_agent_password_template: Option<String>,

    /// Allow the legacy Matrix puppet password template as a fallback.
    #[arg(long)]
    pub matrix_allow_legacy_agent_password: bool,

    /// Matrix registration token for puppet account UIA registration.
    #[arg(long)]
    pub matrix_registration_token: Option<String>,
}

/// Arguments for `agentd matrix-client-bridge-preflight`.
#[derive(Debug, Args)]
pub struct MatrixClientBridgePreflightArgs {
    /// Matrix client bridge service options validated by the preflight.
    #[command(flatten)]
    pub service: MatrixClientBridgeServiceArgs,
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

    /// Allow checkpoint resume when the current workflow content SHA differs from the checkpoint.
    #[arg(long, global = true)]
    pub accept_workflow_change: bool,

    /// Tracing log level (`error`/`warn`/`info`/`debug`/`trace`).
    #[arg(long, default_value = "info", global = true)]
    pub log_level: String,

    /// Operator bearer token for authenticated API calls. Falls back to `AGENTD_API_TOKEN`.
    #[arg(long, global = true)]
    pub api_token: Option<String>,

    /// Per-agent token assignment in NAME=TOKEN form. Repeatable.
    #[arg(long = "agent-token", global = true)]
    pub agent_tokens: Vec<String>,

    /// Per-agent token enforcement mode: hard or audit.
    #[arg(long, default_value = "audit", global = true)]
    pub agent_token_mode: String,
}

impl DaemonConfig {
    #[must_use]
    pub fn auth_config(&self) -> AuthConfig {
        let api_token = clean_token(self.api_token.as_deref()).or_else(|| {
            std::env::var("AGENTD_API_TOKEN")
                .ok()
                .and_then(|v| clean_token(Some(&v)))
        });
        let mut agent_tokens = BTreeMap::new();
        for assignment in &self.agent_tokens {
            if let Some((name, token)) = assignment.split_once('=') {
                if let (Some(name), Some(token)) =
                    (clean_token(Some(name)), clean_token(Some(token)))
                {
                    agent_tokens.insert(name, token);
                }
            }
        }
        let agent_token_mode = match self.agent_token_mode.trim().to_ascii_lowercase().as_str() {
            "hard" => AgentTokenMode::Hard,
            _ => AgentTokenMode::Audit,
        };
        AuthConfig {
            api_token,
            agent_token_mode,
            agent_tokens,
        }
    }
}

fn clean_token(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::{AgentdCli, AgentdCommand, CleanupWorktreesArgs, MatrixBridgeOnceArgs};
    use clap::Parser;
    use clap::error::ErrorKind;
    use std::path::PathBuf;

    fn cleanup_worktrees_args(cli: &AgentdCli) -> &CleanupWorktreesArgs {
        match cli.command.as_ref() {
            Some(AgentdCommand::CleanupWorktrees(args)) => Some(args),
            _ => None,
        }
        .expect("expected cleanup-worktrees command")
    }

    fn matrix_bridge_once_args(cli: &AgentdCli) -> &MatrixBridgeOnceArgs {
        match cli.command.as_ref() {
            Some(AgentdCommand::MatrixBridgeOnce(args)) => Some(args),
            _ => None,
        }
        .expect("expected matrix-bridge-once command")
    }

    fn matrix_client_bridge_service_args(cli: &AgentdCli) -> &super::MatrixClientBridgeServiceArgs {
        match cli.command.as_ref() {
            Some(AgentdCommand::MatrixClientBridgeService(args)) => Some(args),
            _ => None,
        }
        .expect("expected matrix-client-bridge-service command")
    }

    fn matrix_client_bridge_preflight_args(
        cli: &AgentdCli,
    ) -> &super::MatrixClientBridgePreflightArgs {
        match cli.command.as_ref() {
            Some(AgentdCommand::MatrixClientBridgePreflight(args)) => Some(args),
            _ => None,
        }
        .expect("expected matrix-client-bridge-preflight command")
    }

    const MATRIX_CLIENT_BRIDGE_SERVICE_CLI: &[&str] = &[
        "agentd",
        "--api-token",
        "bridge-secret",
        "matrix-client-bridge-service",
        "--agentd-api",
        "http://127.0.0.1:8787",
        "--state",
        "state/matrix-client.json",
        "--iterations",
        "3",
        "--matrix-homeserver-url",
        "http://127.0.0.1:8008",
        "--matrix-username",
        "agentd-bot",
        "--matrix-password",
        "bot-secret",
        "--matrix-sync-timeout-ms",
        "250",
        "--matrix-sdk-store",
        "state/matrix-sdk",
        "--matrix-bot-user-id",
        "@agentd-bot:matrix.test",
        "--matrix-server-name",
        "matrix.test",
        "--matrix-agent-prefix",
        "ac_",
        "--matrix-agent",
        "codex-worker",
        "--matrix-agent",
        "codex-reviewer",
        "--matrix-skip-agent",
        "codex-reviewer",
        "--matrix-trust-mode",
        "enforce",
        "--matrix-trusted-inviter",
        "@alex:matrix.test",
        "--matrix-ignore-sender",
        "@noise:matrix.test",
        "--matrix-operator",
        "@operator:matrix.test",
        "--matrix-admin",
        "@admin:matrix.test",
        "--matrix-puppet-state",
        "state/bridge-state.json",
        "--matrix-agent-password-secret",
        "matrix-secret",
        "--matrix-agent-password-template",
        "legacy-{{agent}}",
        "--matrix-allow-legacy-agent-password",
        "--matrix-registration-token",
        "registration-token",
    ];

    #[test]
    fn agentd_cli_cleanup_worktrees_is_dry_run_by_default() {
        let cli = AgentdCli::parse_from(["agentd", "cleanup-worktrees"]);
        let args = cleanup_worktrees_args(&cli);
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
        let args = cleanup_worktrees_args(&cli);

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
        let args = cleanup_worktrees_args(&cli);

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

        assert!(matches!(cli.command, Some(AgentdCommand::McpStdio(_))));
        assert_eq!(PathBuf::from("state.db"), cli.config.db_path);
        assert_eq!(PathBuf::from("workflows"), cli.config.workflows_dir);
    }

    #[test]
    fn agentd_cli_accepts_agent_api_auth_options() {
        let cli = AgentdCli::try_parse_from([
            "agentd",
            "--api-token",
            "operator-secret",
            "--agent-token",
            "codex-worker=agent-secret",
            "--agent-token-mode",
            "hard",
        ])
        .expect("auth config parses");

        assert_eq!(cli.config.api_token.as_deref(), Some("operator-secret"));
        assert_eq!(
            cli.config.agent_tokens,
            vec!["codex-worker=agent-secret".to_string()]
        );
        assert_eq!(cli.config.agent_token_mode, "hard");
    }

    #[test]
    fn agentd_cli_accepts_accept_workflow_change_flag() {
        let default = AgentdCli::try_parse_from(["agentd"]).expect("daemon defaults parse");
        assert!(!default.config.accept_workflow_change);

        let cli = AgentdCli::try_parse_from(["agentd", "--accept-workflow-change"])
            .expect("accept workflow change flag parses");
        assert!(cli.config.accept_workflow_change);
    }

    #[test]
    fn agentd_cli_matrix_bridge_once_accepts_files_api_and_auth() {
        let cli = AgentdCli::try_parse_from([
            "agentd",
            "--api-token",
            "bridge-secret",
            "matrix-bridge-once",
            "--agentd-api",
            "http://127.0.0.1:8787",
            "--state",
            "state/matrix.json",
            "--rooms-json",
            "fixtures/rooms.json",
            "--inbound-json",
            "fixtures/inbound.json",
            "--sent-log-jsonl",
            "logs/sent.jsonl",
        ])
        .expect("matrix-bridge-once parses");
        let args = matrix_bridge_once_args(&cli);

        assert_eq!(cli.config.api_token.as_deref(), Some("bridge-secret"));
        assert_eq!(args.agentd_api, "http://127.0.0.1:8787");
        assert_eq!(args.state, PathBuf::from("state/matrix.json"));
        assert_eq!(args.rooms_json, PathBuf::from("fixtures/rooms.json"));
        assert_eq!(args.inbound_json, PathBuf::from("fixtures/inbound.json"));
        assert_eq!(args.sent_log_jsonl, PathBuf::from("logs/sent.jsonl"));
    }

    #[test]
    fn agentd_cli_matrix_bridge_once_accepts_puppet_account_options() {
        let cli = AgentdCli::try_parse_from([
            "agentd",
            "matrix-bridge-once",
            "--agentd-api",
            "http://127.0.0.1:8787",
            "--state",
            "state/matrix.json",
            "--rooms-json",
            "fixtures/rooms.json",
            "--inbound-json",
            "fixtures/inbound.json",
            "--sent-log-jsonl",
            "logs/sent.jsonl",
            "--matrix-homeserver-url",
            "http://127.0.0.1:8008",
            "--matrix-server-name",
            "matrix.test",
            "--matrix-agent-prefix",
            "ac_",
            "--matrix-agent",
            "codex-worker",
            "--matrix-agent",
            "codex-reviewer",
            "--matrix-skip-agent",
            "codex-reviewer",
            "--matrix-puppet-state",
            "state/bridge-state.json",
            "--matrix-agent-password-secret",
            "matrix-secret",
            "--matrix-agent-password-template",
            "legacy-{{agent}}",
            "--matrix-allow-legacy-agent-password",
            "--matrix-registration-token",
            "registration-token",
        ])
        .expect("matrix-bridge-once puppet account options parse");
        let args = matrix_bridge_once_args(&cli);

        assert_eq!(
            args.matrix_homeserver_url.as_deref(),
            Some("http://127.0.0.1:8008")
        );
        assert_eq!(args.matrix_server_name.as_deref(), Some("matrix.test"));
        assert_eq!(args.matrix_agent_prefix, "ac_");
        assert_eq!(
            args.matrix_agents,
            vec!["codex-worker".to_owned(), "codex-reviewer".to_owned()]
        );
        assert_eq!(args.matrix_skip_agents, vec!["codex-reviewer".to_owned()]);
        assert_eq!(
            args.matrix_puppet_state,
            Some(PathBuf::from("state/bridge-state.json"))
        );
        assert_eq!(
            args.matrix_agent_password_secret.as_deref(),
            Some("matrix-secret")
        );
        assert_eq!(
            args.matrix_agent_password_template.as_deref(),
            Some("legacy-{{agent}}")
        );
        assert!(args.matrix_allow_legacy_agent_password);
        assert_eq!(
            args.matrix_registration_token.as_deref(),
            Some("registration-token")
        );
    }

    #[test]
    fn agentd_cli_matrix_client_bridge_service_accepts_sdk_transport_and_puppet_options() {
        let cli = AgentdCli::try_parse_from(MATRIX_CLIENT_BRIDGE_SERVICE_CLI.iter().copied())
            .expect("matrix-client-bridge-service parses");
        let args = matrix_client_bridge_service_args(&cli);

        assert_eq!(cli.config.api_token.as_deref(), Some("bridge-secret"));
        assert_eq!(args.agentd_api, "http://127.0.0.1:8787");
        assert_eq!(args.state, PathBuf::from("state/matrix-client.json"));
        assert_eq!(args.iterations, 3);
        assert_eq!(
            args.matrix_homeserver_url.as_deref(),
            Some("http://127.0.0.1:8008")
        );
        assert_eq!(args.matrix_username.as_deref(), Some("agentd-bot"));
        assert_eq!(args.matrix_password.as_deref(), Some("bot-secret"));
        assert_eq!(args.matrix_sync_timeout_ms, 250);
        assert_eq!(
            args.matrix_sdk_store,
            Some(PathBuf::from("state/matrix-sdk"))
        );
        assert_eq!(
            args.matrix_bot_user_id.as_deref(),
            Some("@agentd-bot:matrix.test")
        );
        assert_eq!(args.matrix_server_name.as_deref(), Some("matrix.test"));
        assert_eq!(args.matrix_agent_prefix, "ac_");
        assert_eq!(
            args.matrix_agents,
            vec!["codex-worker".to_owned(), "codex-reviewer".to_owned()]
        );
        assert_eq!(args.matrix_skip_agents, vec!["codex-reviewer".to_owned()]);
        assert_eq!(args.matrix_trust_mode, "enforce");
        assert_eq!(
            args.matrix_trusted_inviters,
            vec!["@alex:matrix.test".to_owned()]
        );
        assert_eq!(
            args.matrix_ignored_senders,
            vec!["@noise:matrix.test".to_owned()]
        );
        assert_eq!(
            args.matrix_operator_mxids,
            vec!["@operator:matrix.test".to_owned()]
        );
        assert_eq!(
            args.matrix_admin_mxids,
            vec!["@admin:matrix.test".to_owned()]
        );
        assert_eq!(
            args.matrix_puppet_state,
            Some(PathBuf::from("state/bridge-state.json"))
        );
        assert_eq!(
            args.matrix_agent_password_secret.as_deref(),
            Some("matrix-secret")
        );
        assert_eq!(
            args.matrix_agent_password_template.as_deref(),
            Some("legacy-{{agent}}")
        );
        assert!(args.matrix_allow_legacy_agent_password);
        assert_eq!(
            args.matrix_registration_token.as_deref(),
            Some("registration-token")
        );
    }

    #[test]
    fn agentd_cli_matrix_client_bridge_preflight_reuses_service_options() {
        let preflight_cli = MATRIX_CLIENT_BRIDGE_SERVICE_CLI
            .iter()
            .map(|arg| match *arg {
                "matrix-client-bridge-service" => "matrix-client-bridge-preflight",
                other => other,
            })
            .collect::<Vec<_>>();
        let cli = AgentdCli::try_parse_from(preflight_cli).expect("preflight parses");
        let args = &matrix_client_bridge_preflight_args(&cli).service;

        assert_eq!(cli.config.api_token.as_deref(), Some("bridge-secret"));
        assert_eq!(args.agentd_api, "http://127.0.0.1:8787");
        assert_eq!(args.state, PathBuf::from("state/matrix-client.json"));
        assert_eq!(args.iterations, 3);
        assert_eq!(
            args.matrix_homeserver_url.as_deref(),
            Some("http://127.0.0.1:8008")
        );
        assert_eq!(args.matrix_username.as_deref(), Some("agentd-bot"));
        assert_eq!(args.matrix_password.as_deref(), Some("bot-secret"));
        assert_eq!(args.matrix_sync_timeout_ms, 250);
        assert_eq!(
            args.matrix_sdk_store,
            Some(PathBuf::from("state/matrix-sdk"))
        );
        assert_eq!(
            args.matrix_bot_user_id.as_deref(),
            Some("@agentd-bot:matrix.test")
        );
        assert_eq!(args.matrix_server_name.as_deref(), Some("matrix.test"));
        assert_eq!(args.matrix_agent_prefix, "ac_");
        assert_eq!(
            args.matrix_agents,
            vec!["codex-worker".to_owned(), "codex-reviewer".to_owned()]
        );
        assert_eq!(args.matrix_skip_agents, vec!["codex-reviewer".to_owned()]);
        assert_eq!(args.matrix_trust_mode, "enforce");
        assert_eq!(
            args.matrix_trusted_inviters,
            vec!["@alex:matrix.test".to_owned()]
        );
        assert_eq!(
            args.matrix_ignored_senders,
            vec!["@noise:matrix.test".to_owned()]
        );
        assert_eq!(
            args.matrix_operator_mxids,
            vec!["@operator:matrix.test".to_owned()]
        );
        assert_eq!(
            args.matrix_admin_mxids,
            vec!["@admin:matrix.test".to_owned()]
        );
        assert_eq!(
            args.matrix_puppet_state,
            Some(PathBuf::from("state/bridge-state.json"))
        );
        assert_eq!(
            args.matrix_agent_password_secret.as_deref(),
            Some("matrix-secret")
        );
        assert_eq!(
            args.matrix_agent_password_template.as_deref(),
            Some("legacy-{{agent}}")
        );
        assert!(args.matrix_allow_legacy_agent_password);
        assert_eq!(
            args.matrix_registration_token.as_deref(),
            Some("registration-token")
        );
    }
}
