//! The MCP tool-call seam. `agentd-mempal` speaks to mempal ONLY through this
//! trait — agentd never touches mempal's on-disk database (design §3.1). Tests
//! inject a `RecordingToolCaller`; the production rmcp-backed transport lands in P0.7.

use crate::error::MempalError;

/// Call a mempal MCP tool by name with JSON args, returning the JSON result.
/// Object-safe so the daemon can hold an `Arc<dyn McpToolCaller>` seam (D3).
#[async_trait::async_trait]
pub trait McpToolCaller: Send + Sync {
    /// Invoke `tool` with `args`, returning its JSON result.
    ///
    /// # Errors
    /// [`MempalError::Transport`] when the call cannot be delivered.
    async fn call_tool(
        &self,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value, MempalError>;
}
