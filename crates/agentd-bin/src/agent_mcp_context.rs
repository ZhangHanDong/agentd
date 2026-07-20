//! Startup context that lets spawned agents call back into the daemon through
//! the stdio MCP entrypoint added in P119.

use std::path::{Component, Path, PathBuf};

use agentd_core::CoreError;
use agentd_core::ports::{AgentAllocation, AgentBackend};
use agentd_core::types::{AgentHandle, SpawnRequest};

use crate::DaemonConfig;

/// Environment variable exported into spawned agent launchers.
pub const AGENTD_MCP_STDIO_CMD_ENV: &str = "AGENTD_MCP_STDIO_CMD";

/// Backend decorator that injects stdio MCP command context before delegating.
pub struct McpStdioContextBackend {
    inner: Box<dyn AgentBackend>,
    command: String,
}

impl std::fmt::Debug for McpStdioContextBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpStdioContextBackend")
            .field("command", &self.command)
            .finish_non_exhaustive()
    }
}

impl McpStdioContextBackend {
    #[must_use]
    pub fn new(inner: Box<dyn AgentBackend>, command: impl Into<String>) -> Self {
        Self {
            inner,
            command: command.into(),
        }
    }
}

#[async_trait::async_trait]
impl AgentBackend for McpStdioContextBackend {
    async fn spawn(&self, mut req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        let command = command_for_agent(&self.command, req.agent_id.as_str());
        inject_context(&mut req, &command);
        self.inner.spawn(req).await
    }

    async fn dispatch_allocated(
        &self,
        mut req: SpawnRequest,
        allocation: &AgentAllocation,
    ) -> Result<AgentHandle, CoreError> {
        let command = command_for_agent(&self.command, req.agent_id.as_str());
        inject_context(&mut req, &command);
        self.inner.dispatch_allocated(req, allocation).await
    }
}

/// Render the stdio command for a production daemon config.
#[must_use]
pub fn mcp_stdio_command(agentd_exe: &Path, config: &DaemonConfig, daemon_cwd: &Path) -> String {
    let db_path = absolute_path(daemon_cwd, &config.db_path);
    let workflows_dir = absolute_path(daemon_cwd, &config.workflows_dir);
    let repo_dir = absolute_path(daemon_cwd, &config.repo_dir);
    let worktree_base = absolute_path(daemon_cwd, &config.worktree_base);
    let mut parts = vec![
        sh_quote_path(agentd_exe),
        "--db-path".to_string(),
        sh_quote_path(&db_path),
        "--workflows-dir".to_string(),
        sh_quote_path(&workflows_dir),
        "--repo-dir".to_string(),
        sh_quote_path(&repo_dir),
        "--worktree-base".to_string(),
        sh_quote_path(&worktree_base),
        "--log-level".to_string(),
        sh_quote("error"),
    ];
    if config.accept_workflow_change {
        parts.push("--accept-workflow-change".to_string());
    }
    parts.push("mcp-stdio".to_string());
    parts.push("--proxy-url".to_string());
    parts.push(sh_quote(&format!("http://127.0.0.1:{}", config.port)));
    parts.join(" ")
}

/// Render the stdio command using the currently running daemon process.
pub fn mcp_stdio_command_from_current_process(config: &DaemonConfig) -> Result<String, CoreError> {
    let agentd_exe = std::env::current_exe()?;
    let daemon_cwd = std::env::current_dir()?;
    Ok(mcp_stdio_command(&agentd_exe, config, &daemon_cwd))
}

fn inject_context(req: &mut SpawnRequest, command: &str) {
    req.env_overrides
        .insert(AGENTD_MCP_STDIO_CMD_ENV.to_string(), command.to_string());
    let block = prompt_block(command);
    req.initial_prompt = Some(match req.initial_prompt.take() {
        Some(existing) if !existing.trim().is_empty() => format!("{existing}\n\n{block}"),
        _ => block,
    });
}

fn command_for_agent(command: &str, agent_id: &str) -> String {
    format!("{command} --agent-id {}", sh_quote(agent_id))
}

fn prompt_block(command: &str) -> String {
    format!(
        "agentd_mcp_stdio:\n\
         server: agentd\n\
         {AGENTD_MCP_STDIO_CMD_ENV}: {command}\n\
         protocol: line-delimited JSON-RPC 2.0 over stdin/stdout\n\
         usage: run the command, call tools/list to inspect tools, then call tools/call with name and arguments to submit outcomes or reviews"
    )
}

fn absolute_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        clean_path(path)
    } else {
        clean_path(&base.join(path))
    }
}

fn clean_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::Normal(part) => out.push(part),
        }
    }
    out
}

fn sh_quote_path(path: &Path) -> String {
    sh_quote(&path.to_string_lossy())
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
