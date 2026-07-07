//! See design §4.2. The engine cares about the *shape* of a handle, not how it's produced.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::types::ids::AgentId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendKind {
    Tmux,
    // Future: Headless, McpOnly
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CliKind {
    ClaudeCode,
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnRequest {
    pub agent_id: AgentId,
    pub mxid: Option<String>,
    pub cli: CliKind,
    pub worktree: PathBuf,
    pub initial_prompt: Option<String>,
    pub env_overrides: HashMap<String, String>,
    pub launch_strategy: LaunchStrategy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LaunchStrategy {
    Direct,
    Systemd { scope_name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHandle {
    pub agent_id: AgentId,
    pub backend: BackendKind,
    pub address: String,
    pub pane_id: Option<String>,
    pub pid: Option<u32>,
    pub session_name: String,
    pub spawned_at: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Gone,
    UnexpectedShell { current_command: String },
    Idle { last_output_age: Duration },
    Busy { last_output_age: Duration },
    Starting,
}
