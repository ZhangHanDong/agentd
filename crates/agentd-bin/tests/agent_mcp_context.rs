use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use agentd_bin::DaemonConfig;
use agentd_bin::agent_mcp_context::{
    AGENTD_MCP_STDIO_CMD_ENV, McpStdioContextBackend, mcp_stdio_command,
};
use agentd_core::CoreError;
use agentd_core::ports::AgentBackend;
use agentd_core::types::{
    AgentHandle, AgentId, BackendKind, CliKind, LaunchStrategy, SpawnRequest,
};

#[derive(Debug, Clone)]
struct RecordingBackend {
    spawned: Arc<Mutex<Vec<SpawnRequest>>>,
}

impl RecordingBackend {
    fn new() -> Self {
        Self {
            spawned: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn spawned(&self) -> Vec<SpawnRequest> {
        self.spawned.lock().expect("spawned lock").clone()
    }
}

#[async_trait::async_trait]
impl AgentBackend for RecordingBackend {
    async fn spawn(&self, req: SpawnRequest) -> Result<AgentHandle, CoreError> {
        let agent_id = req.agent_id.clone();
        self.spawned.lock().expect("spawned lock").push(req);
        Ok(AgentHandle {
            agent_id: agent_id.clone(),
            backend: BackendKind::Tmux,
            address: format!("fake://{}", agent_id.as_str()),
            pane_id: Some("%1".to_string()),
            pid: Some(120),
            session_name: format!("agentd-{}", agent_id.as_str()),
            spawned_at: SystemTime::UNIX_EPOCH,
        })
    }
}

fn request(initial_prompt: Option<&str>) -> SpawnRequest {
    let mut env_overrides = HashMap::new();
    env_overrides.insert("EXISTING_ENV".to_string(), "kept".to_string());
    SpawnRequest {
        agent_id: AgentId::parsed("implementer"),
        mxid: None,
        cli: CliKind::ClaudeCode,
        worktree: PathBuf::from("/tmp/agentd-task-wt"),
        initial_prompt: initial_prompt.map(str::to_string),
        env_overrides,
        launch_strategy: LaunchStrategy::Direct,
    }
}

fn config_with_relative_paths() -> DaemonConfig {
    DaemonConfig {
        db_path: PathBuf::from("state's dir/agentd.db"),
        port: 8787,
        workflows_dir: PathBuf::from("workflows"),
        repo_dir: PathBuf::from("."),
        worktree_base: PathBuf::from(".agentd/worktrees"),
        log_level: "debug".to_string(),
    }
}

#[test]
fn mcp_stdio_command_uses_absolute_config_paths_and_shell_quotes() {
    let command = mcp_stdio_command(
        Path::new("/opt/Agent Bin/agentd"),
        &config_with_relative_paths(),
        Path::new("/repo root/agentd"),
    );

    assert!(
        command.contains("'/opt/Agent Bin/agentd'"),
        "executable path is shell-quoted: {command}"
    );
    assert!(
        command.contains("--db-path '/repo root/agentd/state'\\''s dir/agentd.db'"),
        "relative db path is absolutized and apostrophe-safe: {command}"
    );
    assert!(
        command.contains("--workflows-dir '/repo root/agentd/workflows'"),
        "workflows dir is absolute: {command}"
    );
    assert!(
        command.contains("--repo-dir '/repo root/agentd'"),
        "repo dir is absolute: {command}"
    );
    assert!(
        command.contains("--worktree-base '/repo root/agentd/.agentd/worktrees'"),
        "worktree base is absolute: {command}"
    );
    assert!(
        command.contains("--log-level 'error'"),
        "stdio command forces quiet stdout logs: {command}"
    );
    assert!(
        command.ends_with(" mcp-stdio"),
        "command ends with the stdio subcommand: {command}"
    );
}

#[tokio::test]
async fn mcp_context_backend_exports_command_and_appends_prompt() {
    let inner = RecordingBackend::new();
    let backend = McpStdioContextBackend::new(
        Box::new(inner.clone()),
        "agentd --db-path '/tmp/agentd.db' mcp-stdio",
    );

    backend
        .spawn(request(Some("existing task prompt")))
        .await
        .expect("spawn succeeds");

    let spawned = inner.spawned();
    assert_eq!(spawned.len(), 1);
    let req = &spawned[0];
    assert_eq!(
        req.env_overrides.get("EXISTING_ENV").map(String::as_str),
        Some("kept"),
        "existing env override is preserved"
    );
    assert_eq!(
        req.env_overrides
            .get(AGENTD_MCP_STDIO_CMD_ENV)
            .map(String::as_str),
        Some("agentd --db-path '/tmp/agentd.db' mcp-stdio")
    );
    let prompt = req.initial_prompt.as_deref().expect("prompt exists");
    assert!(
        prompt.contains("existing task prompt"),
        "existing prompt is preserved: {prompt}"
    );
    assert!(prompt.contains("agentd_mcp_stdio"), "{prompt}");
    assert!(prompt.contains("tools/list"), "{prompt}");
    assert!(prompt.contains("tools/call"), "{prompt}");
}

#[tokio::test]
async fn mcp_context_backend_creates_prompt_when_missing() {
    let inner = RecordingBackend::new();
    let backend = McpStdioContextBackend::new(Box::new(inner.clone()), "agentd mcp-stdio");

    backend.spawn(request(None)).await.expect("spawn succeeds");

    let spawned = inner.spawned();
    assert_eq!(spawned.len(), 1);
    let prompt = spawned[0].initial_prompt.as_deref().expect("prompt exists");
    assert!(prompt.contains("agentd mcp-stdio"), "{prompt}");
    assert!(prompt.contains(AGENTD_MCP_STDIO_CMD_ENV), "{prompt}");
}

#[tokio::test]
async fn mcp_context_backend_prompt_names_agentd_server() {
    let inner = RecordingBackend::new();
    let backend = McpStdioContextBackend::new(Box::new(inner.clone()), "agentd mcp-stdio");

    backend
        .spawn(request(Some("task prompt")))
        .await
        .expect("spawn succeeds");

    let spawned = inner.spawned();
    let prompt = spawned[0].initial_prompt.as_deref().expect("prompt exists");
    assert!(
        prompt.contains("server: agentd"),
        "prompt should name the MCP server configured by the launcher: {prompt}"
    );
    assert!(prompt.contains("tools/list"), "{prompt}");
    assert!(prompt.contains("tools/call"), "{prompt}");
}
