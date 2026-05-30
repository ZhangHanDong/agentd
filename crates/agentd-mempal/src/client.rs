//! `MempalMcpClient` — implements the core `MempalClient` port (design §4.12.2)
//! over the injected `McpToolCaller` seam. Reads are best-effort with a timeout
//! (§3.4); the public boundary returns `CoreError` (D5).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use agentd_core::CoreError;
use agentd_core::ports::{DrawerHit, MempalClient};
use serde_json::{Value, json};

use crate::error::MempalError;
use crate::transport::McpToolCaller;

/// Timing config for the mempal client.
#[derive(Debug, Clone)]
pub struct MempalConfig {
    /// `pre_tools` read timeout (design §3.4). A read that exceeds it returns an
    /// error; the caller (e.g. `codergen.rs`) substitutes empty results.
    pub pre_tools_timeout: Duration,
}

impl Default for MempalConfig {
    fn default() -> Self {
        Self {
            pre_tools_timeout: Duration::from_secs(3),
        }
    }
}

/// The mempal MCP client: maps `MempalClient` methods to mempal tool calls
/// (§4.12.2) through the injected transport.
pub struct MempalMcpClient {
    caller: Arc<dyn McpToolCaller>,
    cfg: MempalConfig,
}

impl fmt::Debug for MempalMcpClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `dyn McpToolCaller` is not `Debug`, so the caller is elided.
        f.debug_struct("MempalMcpClient")
            .field("cfg", &self.cfg)
            .finish_non_exhaustive()
    }
}

impl MempalMcpClient {
    /// Build a client over `caller` with `cfg`.
    #[must_use]
    pub fn new(caller: Arc<dyn McpToolCaller>, cfg: MempalConfig) -> Self {
        Self { caller, cfg }
    }

    /// Run a best-effort read tool under the `pre_tools` timeout (§3.4). A
    /// timeout is `MempalError::Timeout` (and a warning) — never swallowed to
    /// empty; the caller owns that fallback (D4).
    async fn read_tool(&self, tool: &str, args: Value) -> Result<Value, MempalError> {
        match tokio::time::timeout(
            self.cfg.pre_tools_timeout,
            self.caller.call_tool(tool, args),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                tracing::warn!(tool, "mempal read timed out; caller substitutes empty");
                Err(MempalError::Timeout(self.cfg.pre_tools_timeout))
            }
        }
    }
}

/// Parse an array of `{drawer_id, body, score}` objects under `field` into
/// `DrawerHit`s. A missing `field` is an empty result; a present-but-malformed
/// payload (not an array, or a hit without `drawer_id`) is `Decode`.
#[allow(clippy::cast_possible_truncation)] // score is a 0..1 similarity; f64→f32 loss is fine
fn parse_hits(value: &Value, field: &str) -> Result<Vec<DrawerHit>, MempalError> {
    let array = match value.get(field) {
        Some(Value::Array(array)) => array,
        Some(_) => return Err(MempalError::Decode(format!("`{field}` is not an array"))),
        None => return Ok(Vec::new()),
    };
    let mut hits = Vec::with_capacity(array.len());
    for elem in array {
        let drawer_id = elem
            .get("drawer_id")
            .and_then(Value::as_str)
            .ok_or_else(|| MempalError::Decode("hit is missing drawer_id".to_string()))?;
        let body = elem.get("body").and_then(Value::as_str).unwrap_or_default();
        let score = elem.get("score").and_then(Value::as_f64).unwrap_or(0.0) as f32;
        hits.push(DrawerHit {
            drawer_id: drawer_id.to_string(),
            body: body.to_string(),
            score,
        });
    }
    Ok(hits)
}

#[async_trait::async_trait]
impl MempalClient for MempalMcpClient {
    async fn search(
        &self,
        query: &str,
        wing: &str,
        kind: &str,
    ) -> Result<Vec<DrawerHit>, CoreError> {
        let args = json!({ "query": query, "wing": wing, "kind": kind });
        let result = self.read_tool("mempal_search", args).await?;
        Ok(parse_hits(&result, "hits")?)
    }

    async fn ingest(&self, wing: &str, kind: &str, body: &str) -> Result<(), CoreError> {
        let args = json!({ "wing": wing, "kind": kind, "body": body });
        self.caller.call_tool("mempal_ingest", args).await?;
        Ok(())
    }

    async fn kg_add(&self, subject: &str, predicate: &str, object: &str) -> Result<(), CoreError> {
        let args =
            json!({ "op": "add", "subject": subject, "predicate": predicate, "object": object });
        self.caller.call_tool("mempal_kg", args).await?;
        Ok(())
    }

    async fn fact_check(&self, claim: &str) -> Result<Vec<DrawerHit>, CoreError> {
        let args = json!({ "text": claim });
        let result = self.read_tool("mempal_fact_check", args).await?;
        Ok(parse_hits(&result, "issues")?)
    }
}
