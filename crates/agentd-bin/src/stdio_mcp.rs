//! Line-delimited stdio JSON-RPC host for the agent-facing MCP dispatcher.
//!
//! This P119 entrypoint deliberately reuses `agentd-surface`'s existing
//! transport-agnostic dispatcher. It gives real local agent processes a stable
//! process boundary while the external `rmcp` crate version is settled in a
//! later compatibility slice.

use agentd_surface::host::RunHost;
use agentd_surface::mcp_server::{dispatch, tool_descriptors};
use rmcp::model::{
    CallToolResult, Implementation, InitializeResult, JsonObject, ProtocolVersion,
    ServerCapabilities, Tool,
};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};

/// Handle one JSON-RPC request object.
pub async fn handle_request(host: &dyn RunHost, request: Value) -> Value {
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return error_response(id, -32600, "invalid request", None);
    };

    match method {
        "initialize" => success_response(id, initialize_result()),
        "tools/list" => success_response(id, tools_list_result()),
        "tools/call" => handle_tools_call(host, id, &request).await,
        other => error_response(id, -32601, format!("method not found: {other}"), None),
    }
}

/// Serve line-delimited JSON-RPC requests from `reader`, writing one response
/// line per request to `writer`.
pub async fn serve_json_lines<R, W>(
    host: &dyn RunHost,
    mut reader: R,
    mut writer: W,
) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).await?;
        if read == 0 {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(request) if is_initialized_notification(&request) => continue,
            Ok(request) => handle_request(host, request).await,
            Err(err) => error_response(Value::Null, -32700, format!("parse error: {err}"), None),
        };
        writer.write_all(response.to_string().as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }
    Ok(())
}

async fn handle_tools_call(host: &dyn RunHost, id: Value, request: &Value) -> Value {
    let Some(params) = request.get("params").and_then(Value::as_object) else {
        return error_response(id, -32602, "missing tools/call params", None);
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error_response(id, -32602, "missing tools/call params.name", None);
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));

    match dispatch(host, name, arguments).await {
        Ok(result) => match serde_json::to_value(CallToolResult::structured(result)) {
            Ok(value) => success_response(id, value),
            Err(err) => error_response(id, -32603, format!("encode tool result: {err}"), None),
        },
        Err(err) => error_response(
            id,
            -32000,
            err.to_string(),
            Some(json!({ "code": err.code() })),
        ),
    }
}

fn initialize_result() -> Value {
    let result = InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
        .with_protocol_version(ProtocolVersion::LATEST)
        .with_server_info(Implementation::new("agentd", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "Use tools/list to inspect tools, then tools/call with name and arguments to submit outcomes or reviews."
        );
    serde_json::to_value(result).unwrap_or_else(|err| {
        json!({
            "protocolVersion": ProtocolVersion::LATEST.as_str(),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "agentd", "version": env!("CARGO_PKG_VERSION") },
            "instructions": format!("failed to encode rmcp initialize model: {err}")
        })
    })
}

fn is_initialized_notification(request: &Value) -> bool {
    request.get("id").is_none()
        && request.get("method").and_then(Value::as_str) == Some("notifications/initialized")
}

fn tools_list_result() -> Value {
    let tools: Vec<Value> = tool_descriptors()
        .into_iter()
        .map(|tool| {
            let mcp_tool = Tool::new(
                tool.name,
                tool.description,
                input_schema_for_tool(tool.name),
            );
            serde_json::to_value(mcp_tool).unwrap_or_else(|err| {
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": empty_input_schema(),
                    "_meta": { "encodeError": err.to_string() }
                })
            })
        })
        .collect();
    json!({ "tools": tools })
}

fn input_schema_for_tool(name: &str) -> JsonObject {
    let schema = match name {
        "assign_task" => json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" },
                "node_id": { "type": "string" },
                "agent_id": { "type": "string" }
            },
            "required": ["run_id", "node_id", "agent_id"]
        }),
        "submit_outcome" => json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" },
                "node_id": { "type": "string" },
                "attempt": { "type": "integer", "minimum": 1 },
                "status": {
                    "type": "string",
                    "enum": ["success", "fail", "retry", "partial_success"]
                },
                "context_updates": { "type": "object" },
                "preferred_label": { "type": "string" },
                "suggested_next": {
                    "type": "array",
                    "items": { "type": "string" }
                }
            },
            "required": ["run_id", "node_id", "attempt", "status"]
        }),
        "submit_review" => json!({
            "type": "object",
            "properties": {
                "review_run_id": { "type": "string" },
                "reviewer_id": { "type": "string" },
                "verdict": {
                    "type": "string",
                    "enum": ["pass", "concern", "blocker"]
                },
                "findings": {
                    "type": "array",
                    "items": {}
                }
            },
            "required": ["review_run_id", "reviewer_id", "verdict"]
        }),
        "check_inbox" => json!({
            "type": "object",
            "properties": {
                "agent_id": { "type": "string" },
                "drain": { "type": "boolean" }
            },
            "required": ["agent_id"]
        }),
        "query_run" => json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" }
            },
            "required": ["run_id"]
        }),
        _ => Value::Object(empty_input_schema()),
    };
    match schema {
        Value::Object(map) => map,
        _ => empty_input_schema(),
    }
}

fn empty_input_schema() -> JsonObject {
    let mut schema = Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema
}

fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })
}

fn error_response(id: Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Value {
    let mut error = Map::new();
    error.insert("code".to_string(), json!(code));
    error.insert("message".to_string(), json!(message.into()));
    if let Some(data) = data {
        error.insert("data".to_string(), data);
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": Value::Object(error),
    })
}
