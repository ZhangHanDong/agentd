//! P119: daemon-side stdio JSON-RPC entrypoint for the existing MCP dispatcher.

use std::path::PathBuf;

use agentd_bin::stdio_mcp::{handle_request, serve_json_lines};
use agentd_bin::{AgentdCli, AgentdCommand, ProductionRunHost, SystemClock};
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::RunId;
use agentd_store::{SqliteStore, run_repo};
use clap::Parser;
use rmcp::model::ProtocolVersion;
use serde_json::{Value, json};
use tokio::io::BufReader;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crate dir has parent")
        .parent()
        .expect("crates dir has parent")
        .to_path_buf()
}

fn workflows_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
}

async fn production_host() -> (ProductionRunHost, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    let host = ProductionRunHost::new(
        store,
        Box::new(FakeBackend::new()),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    (host, dir)
}

async fn production_host_with_started_draft() -> (ProductionRunHost, tempfile::TempDir) {
    let (host, dir) = production_host().await;
    let run = RunId::from_string("r1");
    run_repo::record_run(host.store().pool(), &run, "draft.dot", "sha")
        .await
        .expect("record run");
    host.start_run(&run).await.expect("start draft");
    (host, dir)
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
    assert_eq!(cli.config.db_path, PathBuf::from("state.db"));
    assert_eq!(cli.config.workflows_dir, PathBuf::from("workflows"));
}

#[tokio::test]
async fn mcp_stdio_tools_list_returns_registered_tools() {
    let (host, _dir) = production_host().await;
    let response = handle_request(
        &host,
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/list"}),
    )
    .await;

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 7);
    let names: Vec<&str> = response["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect();
    assert_eq!(
        names,
        [
            "assign_task",
            "submit_outcome",
            "submit_review",
            "check_inbox",
            "query_run"
        ]
    );
}

#[tokio::test]
async fn mcp_stdio_initialize_returns_server_capabilities() {
    let (host, _dir) = production_host().await;
    let response = handle_request(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "init-1",
            "method": "initialize",
            "params": {
                "protocolVersion": ProtocolVersion::LATEST.as_str(),
                "capabilities": {},
                "clientInfo": { "name": "agentd-test-client", "version": "0.0.0" }
            }
        }),
    )
    .await;

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], "init-1");
    assert_eq!(
        response["result"]["protocolVersion"],
        ProtocolVersion::LATEST.as_str()
    );
    assert!(response["result"]["capabilities"]["tools"].is_object());
    assert_eq!(response["result"]["serverInfo"]["name"], "agentd");
    assert!(
        response["result"]["instructions"]
            .as_str()
            .expect("instructions")
            .contains("tools/list")
    );
}

#[tokio::test]
async fn mcp_stdio_tools_list_includes_input_schemas() {
    let (host, _dir) = production_host().await;
    let response = handle_request(
        &host,
        json!({"jsonrpc": "2.0", "id": 11, "method": "tools/list"}),
    )
    .await;

    let tools = response["result"]["tools"].as_array().expect("tools array");
    assert_eq!(tools.len(), 5);
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
    }
    let submit_outcome = tools
        .iter()
        .find(|tool| tool["name"] == "submit_outcome")
        .expect("submit_outcome tool");
    let required = submit_outcome["inputSchema"]["required"]
        .as_array()
        .expect("required array");
    for field in ["run_id", "node_id", "attempt", "status"] {
        assert!(
            required.iter().any(|value| value == field),
            "missing {field}: {submit_outcome}"
        );
    }
}

#[tokio::test]
async fn mcp_stdio_tools_call_routes_to_dispatch() {
    let (host, _dir) = production_host_with_started_draft().await;
    let response = handle_request(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "call-1",
            "method": "tools/call",
            "params": {
                "name": "query_run",
                "arguments": { "run_id": "r1" }
            }
        }),
    )
    .await;

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], "call-1");
    assert_eq!(
        response["result"]["structuredContent"]["current_node"],
        "propose_spec"
    );
}

#[tokio::test]
async fn mcp_stdio_tools_call_returns_call_tool_result_shape() {
    let (host, _dir) = production_host_with_started_draft().await;
    let response = handle_request(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "call-2",
            "method": "tools/call",
            "params": {
                "name": "query_run",
                "arguments": { "run_id": "r1" }
            }
        }),
    )
    .await;

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], "call-2");
    assert_eq!(response["result"]["isError"], false);
    assert_eq!(
        response["result"]["structuredContent"]["current_node"],
        "propose_spec"
    );
    let content = response["result"]["content"].as_array().expect("content");
    assert_eq!(content.len(), 1);
    assert_eq!(content[0]["type"], "text");
    assert!(
        content[0]["text"]
            .as_str()
            .expect("text")
            .contains("propose_spec")
    );
}

#[tokio::test]
async fn mcp_stdio_unknown_method_returns_json_rpc_error() {
    let (host, _dir) = production_host().await;
    let response = handle_request(
        &host,
        json!({"jsonrpc": "2.0", "id": 8, "method": "resources/list"}),
    )
    .await;

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 8);
    assert_eq!(response["error"]["code"], -32601);
}

#[tokio::test]
async fn mcp_stdio_tool_failure_preserves_surface_code() {
    let (host, _dir) = production_host().await;
    let response = handle_request(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": 9,
            "method": "tools/call",
            "params": {
                "name": "query_run",
                "arguments": { "run_id": "ghost" }
            }
        }),
    )
    .await;

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 9);
    assert_eq!(response["error"]["code"], -32000);
    assert_eq!(response["error"]["data"]["code"], "not_found");
}

#[tokio::test]
async fn mcp_stdio_loop_writes_json_lines_to_stdout() {
    let (host, _dir) = production_host().await;
    let input = br#"{"jsonrpc":"2.0","id":10,"method":"tools/list"}
"#;
    let reader = BufReader::new(&input[..]);
    let mut output = Vec::new();

    serve_json_lines(&host, reader, &mut output)
        .await
        .expect("stdio loop");

    let stdout = String::from_utf8(output).expect("utf8");
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 1, "stdout must contain one response: {stdout}");
    assert!(
        lines[0].starts_with('{'),
        "stdout must start with JSON, not tracing text: {stdout}"
    );
    let response: Value = serde_json::from_str(lines[0]).expect("json response");
    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 10);
}

#[tokio::test]
async fn mcp_stdio_loop_ignores_initialized_notification() {
    let (host, _dir) = production_host().await;
    let input = br#"{"jsonrpc":"2.0","method":"notifications/initialized"}
"#;
    let reader = BufReader::new(&input[..]);
    let mut output = Vec::new();

    serve_json_lines(&host, reader, &mut output)
        .await
        .expect("stdio loop");

    assert!(output.is_empty(), "stdout must be empty for notifications");
}

#[test]
fn rmcp_workspace_dependency_is_version_aligned() {
    let root = workspace_root();
    let workspace_toml =
        std::fs::read_to_string(root.join("Cargo.toml")).expect("workspace Cargo.toml");
    let bin_toml =
        std::fs::read_to_string(root.join("crates/agentd-bin/Cargo.toml")).expect("bin Cargo.toml");

    assert!(
        workspace_toml.contains(r#"rmcp = { version = "2.1""#),
        "{workspace_toml}"
    );
    assert!(
        workspace_toml.contains("default-features = false"),
        "{workspace_toml}"
    );
    assert!(
        workspace_toml.contains(r#"features = ["server"]"#),
        "{workspace_toml}"
    );
    assert!(bin_toml.contains("rmcp = { workspace = true"), "{bin_toml}");
}
