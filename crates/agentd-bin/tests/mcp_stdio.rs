//! P119: daemon-side stdio JSON-RPC entrypoint for the existing MCP dispatcher.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentd_bin::stdio_mcp::{
    handle_proxy_request, handle_proxy_request_with_identity,
    handle_proxy_request_with_identity_and_media_cache, handle_request,
    handle_request_with_identity, serve_json_lines,
};
use agentd_bin::{AgentdCli, AgentdCommand, ProductionRunHost, SystemClock};
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::RunId;
use agentd_store::{SqliteStore, run_repo};
use agentd_surface::host::{AgentRegistration, GroupCreateInput, RunHost};
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

async fn register_agent(host: &ProductionRunHost, name: &str) {
    host.register_agent(AgentRegistration {
        name: name.to_string(),
        role: Some("agent".to_string()),
        capability: None,
        runtime: Some("codex".to_string()),
        model: None,
        tmux_target: None,
        home_dir: None,
        workdir: Some("/tmp/agentd-test".to_string()),
        state_dir: None,
        server: None,
        runtime_profile: json!({}),
    })
    .await
    .expect("register agent");
}

async fn create_group(host: &ProductionRunHost, name: &str, members: &[&str]) {
    host.create_group(GroupCreateInput {
        name: name.to_string(),
        members: members.iter().map(ToString::to_string).collect(),
    })
    .await
    .expect("create group");
}

fn write_attachment(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> String {
    let path = dir.path().join(name);
    fs::write(&path, bytes).expect("write attachment");
    path.to_string_lossy().to_string()
}

async fn spawn_proxy_media_server(
    tool_body: Value,
    media_body: &'static [u8],
    media_status: &'static str,
    expected_requests: usize,
) -> (String, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener addr");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let server_requests = Arc::clone(&requests);
    let tool_body = tool_body.to_string();
    let media_body = media_body.to_vec();
    let handle = tokio::spawn(async move {
        for _ in 0..expected_requests {
            let (socket, _) = listener.accept().await.expect("accept");
            let mut buf = vec![0_u8; 8192];
            let n = socket
                .readable()
                .await
                .and_then(|()| socket.try_read(&mut buf))
                .expect("read");
            let request = String::from_utf8_lossy(&buf[..n]).to_string();
            server_requests
                .lock()
                .expect("request lock")
                .push(request.clone());
            let (status, content_type, body) = if request.starts_with("POST /tools/call ") {
                ("200 OK", "application/json", tool_body.as_bytes())
            } else if request.starts_with("GET /api/media/fetch?") {
                (
                    media_status,
                    if media_status.starts_with("200") {
                        "text/plain"
                    } else {
                        "application/json"
                    },
                    media_body.as_slice(),
                )
            } else {
                (
                    "404 Not Found",
                    "application/json",
                    br#"{"error":"unexpected"}"#.as_slice(),
                )
            };
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            socket.writable().await.expect("writable");
            socket
                .try_write([response.as_bytes(), body].concat().as_slice())
                .expect("write response");
        }
    });
    (format!("http://{addr}"), requests, handle)
}

fn assert_path_inside(path: &str, root: &Path) -> PathBuf {
    let path = PathBuf::from(path);
    assert!(
        path.starts_with(root),
        "path {} should be inside {}",
        path.display(),
        root.display()
    );
    path
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
    assert_eq!(cli.config.db_path, PathBuf::from("state.db"));
    assert_eq!(cli.config.workflows_dir, PathBuf::from("workflows"));
}

#[test]
fn agentd_cli_mcp_stdio_accepts_proxy_url() {
    let cli = AgentdCli::try_parse_from([
        "agentd",
        "mcp-stdio",
        "--proxy-url",
        "http://127.0.0.1:18789",
    ])
    .expect("mcp-stdio proxy parses");

    let Some(AgentdCommand::McpStdio(args)) = cli.command else {
        panic!("expected mcp-stdio command");
    };
    assert_eq!(args.proxy_url.as_deref(), Some("http://127.0.0.1:18789"));
}

#[test]
fn agentd_cli_mcp_stdio_accepts_agent_id() {
    let cli = AgentdCli::try_parse_from(["agentd", "mcp-stdio", "--agent-id", "codex-worker"])
        .expect("mcp-stdio agent identity parses");

    let Some(AgentdCommand::McpStdio(args)) = cli.command else {
        panic!("expected mcp-stdio command");
    };
    assert_eq!(args.agent_id.as_deref(), Some("codex-worker"));
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
            "send_message",
            "post",
            "submit_human_answer",
            "check_inbox",
            "check_group",
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
    assert_eq!(tools.len(), 9);
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
async fn mcp_stdio_tools_list_advertises_send_message_schema() {
    let (host, _dir) = production_host().await;
    let response = handle_request(
        &host,
        json!({"jsonrpc": "2.0", "id": 12, "method": "tools/list"}),
    )
    .await;

    let tools = response["result"]["tools"].as_array().expect("tools array");
    let send_message = tools
        .iter()
        .find(|tool| tool["name"] == "send_message")
        .expect("send_message tool");
    let required = send_message["inputSchema"]["required"]
        .as_array()
        .expect("required array");
    for field in ["from_agent", "to", "summary", "full"] {
        assert!(
            required.iter().any(|value| value == field),
            "missing {field}: {send_message}"
        );
    }
    let message_types = send_message["inputSchema"]["properties"]["type"]["enum"]
        .as_array()
        .expect("type enum");
    for value in ["request", "inform", "reply"] {
        assert!(
            message_types.iter().any(|item| item == value),
            "missing type {value}: {send_message}"
        );
    }
    let priorities = send_message["inputSchema"]["properties"]["priority"]["enum"]
        .as_array()
        .expect("priority enum");
    for value in ["normal", "high", "urgent"] {
        assert!(
            priorities.iter().any(|item| item == value),
            "missing priority {value}: {send_message}"
        );
    }
    assert!(
        send_message["inputSchema"]["properties"]["attachments"].is_object(),
        "send_message schema must advertise attachments: {send_message}"
    );
}

#[tokio::test]
async fn mcp_stdio_schemas_advertise_attachment_inputs() {
    let (host, _dir) = production_host().await;
    let response = handle_request(
        &host,
        json!({"jsonrpc": "2.0", "id": "schema-attachments", "method": "tools/list"}),
    )
    .await;

    let tools = response["result"]["tools"].as_array().expect("tools array");
    let send_message = tools
        .iter()
        .find(|tool| tool["name"] == "send_message")
        .expect("send_message tool");
    assert!(
        send_message["inputSchema"]["properties"]["attachments"].is_object(),
        "send_message schema must advertise attachments: {send_message}"
    );

    let post = tools
        .iter()
        .find(|tool| tool["name"] == "post")
        .expect("post tool");
    assert!(
        post["inputSchema"]["properties"]["attachments"].is_object(),
        "post schema must advertise attachments: {post}"
    );

    let check_inbox = tools
        .iter()
        .find(|tool| tool["name"] == "check_inbox")
        .expect("check_inbox tool");
    assert!(
        check_inbox["inputSchema"]["properties"]["attachments"].is_null(),
        "check_inbox schema must not accept attachments: {check_inbox}"
    );

    let check_group = tools
        .iter()
        .find(|tool| tool["name"] == "check_group")
        .expect("check_group tool");
    assert!(
        check_group["inputSchema"]["properties"]["attachments"].is_null(),
        "check_group schema must not accept attachments: {check_group}"
    );
}

#[tokio::test]
async fn mcp_stdio_tools_list_with_identity_makes_send_and_inbox_identity_implicit() {
    let (host, _dir) = production_host().await;
    let response = handle_request_with_identity(
        &host,
        json!({"jsonrpc": "2.0", "id": 13, "method": "tools/list"}),
        Some("codex-worker"),
    )
    .await;

    let tools = response["result"]["tools"].as_array().expect("tools array");
    let send_message = tools
        .iter()
        .find(|tool| tool["name"] == "send_message")
        .expect("send_message tool");
    let send_required = send_message["inputSchema"]["required"]
        .as_array()
        .expect("send_message required array");
    assert!(
        !send_required.iter().any(|value| value == "from_agent"),
        "identity-bound send_message must not require from_agent: {send_message}"
    );
    for field in ["to", "summary", "full"] {
        assert!(
            send_required.iter().any(|value| value == field),
            "missing {field}: {send_message}"
        );
    }

    let check_inbox = tools
        .iter()
        .find(|tool| tool["name"] == "check_inbox")
        .expect("check_inbox tool");
    let inbox_required = check_inbox["inputSchema"]["required"]
        .as_array()
        .expect("check_inbox required array");
    assert!(
        !inbox_required.iter().any(|value| value == "agent_id"),
        "identity-bound check_inbox must not require agent_id: {check_inbox}"
    );
}

#[tokio::test]
async fn mcp_stdio_identity_sends_without_from_and_reads_own_inbox() {
    let (host, _dir) = production_host().await;
    let sent = handle_request_with_identity(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "send-with-identity",
            "method": "tools/call",
            "params": {
                "name": "send_message",
                "arguments": {
                    "to": "codex-reviewer",
                    "summary": "identity summary",
                    "full": "identity full"
                }
            }
        }),
        Some("codex-worker"),
    )
    .await;

    assert_eq!(sent["jsonrpc"], "2.0");
    assert_eq!(sent["id"], "send-with-identity");
    assert_eq!(
        sent["result"]["structuredContent"]["message"]["from"],
        "codex-worker"
    );
    assert_eq!(
        sent["result"]["structuredContent"]["message"]["to"],
        "codex-reviewer"
    );

    let inbox = handle_request_with_identity(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "own-inbox",
            "method": "tools/call",
            "params": {
                "name": "check_inbox",
                "arguments": { "drain": false }
            }
        }),
        Some("codex-reviewer"),
    )
    .await;

    assert_eq!(inbox["jsonrpc"], "2.0");
    assert_eq!(inbox["id"], "own-inbox");
    let dm = inbox["result"]["structuredContent"]["dm"]
        .as_array()
        .expect("dm array");
    assert_eq!(dm.len(), 1, "inbox response: {inbox}");
    assert_eq!(dm[0]["from"], "codex-worker");
    assert_eq!(dm[0]["summary"], "identity summary");
}

#[tokio::test]
async fn mcp_stdio_identity_rejects_sender_and_inbox_spoofing() {
    let (host, _dir) = production_host().await;
    let spoof_send = handle_request_with_identity(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "spoof-send",
            "method": "tools/call",
            "params": {
                "name": "send_message",
                "arguments": {
                    "from_agent": "other-agent",
                    "to": "codex-reviewer",
                    "summary": "spoof summary",
                    "full": "spoof full"
                }
            }
        }),
        Some("codex-worker"),
    )
    .await;

    assert_eq!(spoof_send["jsonrpc"], "2.0");
    assert_eq!(spoof_send["id"], "spoof-send");
    assert_eq!(spoof_send["error"]["code"], -32000);
    assert_eq!(spoof_send["error"]["data"]["code"], "bad_request");

    let spoof_inbox = handle_request_with_identity(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "spoof-inbox",
            "method": "tools/call",
            "params": {
                "name": "check_inbox",
                "arguments": { "agent_id": "other-agent", "drain": false }
            }
        }),
        Some("codex-worker"),
    )
    .await;

    assert_eq!(spoof_inbox["jsonrpc"], "2.0");
    assert_eq!(spoof_inbox["id"], "spoof-inbox");
    assert_eq!(spoof_inbox["error"]["code"], -32000);
    assert_eq!(spoof_inbox["error"]["data"]["code"], "bad_request");
}

#[tokio::test]
async fn mcp_stdio_tools_list_with_identity_makes_group_tools_identity_implicit() {
    let (host, _dir) = production_host().await;
    let response = handle_request_with_identity(
        &host,
        json!({"jsonrpc": "2.0", "id": 14, "method": "tools/list"}),
        Some("codex-a"),
    )
    .await;

    let tools = response["result"]["tools"].as_array().expect("tools array");
    let post = tools
        .iter()
        .find(|tool| tool["name"] == "post")
        .expect("post tool");
    let post_required = post["inputSchema"]["required"]
        .as_array()
        .expect("post required array");
    assert!(
        !post_required.iter().any(|value| value == "from_agent"),
        "identity-bound post must not require from_agent: {post}"
    );
    for field in ["group", "summary", "full"] {
        assert!(
            post_required.iter().any(|value| value == field),
            "missing {field}: {post}"
        );
    }
    assert!(
        post["inputSchema"]["properties"]["attachments"].is_object(),
        "post schema must advertise p221 attachments: {post}"
    );

    let check_group = tools
        .iter()
        .find(|tool| tool["name"] == "check_group")
        .expect("check_group tool");
    let group_required = check_group["inputSchema"]["required"]
        .as_array()
        .expect("check_group required array");
    assert!(
        !group_required.iter().any(|value| value == "agent_id"),
        "identity-bound check_group must not require agent_id: {check_group}"
    );
    assert!(
        group_required.iter().any(|value| value == "group"),
        "missing group: {check_group}"
    );
    assert!(
        check_group["inputSchema"]["properties"]["attachments"].is_null(),
        "check_group schema must not advertise attachments: {check_group}"
    );
}

#[tokio::test]
async fn mcp_stdio_post_then_check_group_reads_and_consumes_group_message() {
    let (host, _dir) = production_host().await;
    for agent in ["codex-a", "codex-b"] {
        register_agent(&host, agent).await;
    }
    create_group(&host, "factory", &["codex-a", "codex-b"]).await;

    let posted = handle_request_with_identity(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "post-group",
            "method": "tools/call",
            "params": {
                "name": "post",
                "arguments": {
                    "group": "factory",
                    "summary": "stdio group summary",
                    "full": "stdio group full",
                    "mentions": ["codex-b"]
                }
            }
        }),
        Some("codex-a"),
    )
    .await;
    assert_eq!(posted["jsonrpc"], "2.0");
    assert_eq!(posted["result"]["structuredContent"]["ok"], true);

    let preview = handle_request_with_identity(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "check-group-preview",
            "method": "tools/call",
            "params": {
                "name": "check_group",
                "arguments": { "group": "factory" }
            }
        }),
        Some("codex-b"),
    )
    .await;
    assert_eq!(preview["jsonrpc"], "2.0");
    assert_eq!(preview["result"]["structuredContent"]["unread_total"], 1);
    assert_eq!(
        preview["result"]["structuredContent"]["unread"][0]["summary"],
        "stdio group summary"
    );

    let consumed = handle_request_with_identity(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "check-group-consume",
            "method": "tools/call",
            "params": {
                "name": "check_group",
                "arguments": { "group": "factory", "read_all": true }
            }
        }),
        Some("codex-b"),
    )
    .await;
    assert_eq!(consumed["jsonrpc"], "2.0");
    assert_eq!(consumed["result"]["structuredContent"]["advance"], "all");

    let after = handle_request_with_identity(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "check-group-after",
            "method": "tools/call",
            "params": {
                "name": "check_group",
                "arguments": { "group": "factory" }
            }
        }),
        Some("codex-b"),
    )
    .await;
    assert_eq!(after["result"]["structuredContent"]["unread_total"], 0);
}

#[tokio::test]
async fn mcp_stdio_send_message_attachment_round_trips_through_check_inbox() {
    let (host, dir) = production_host().await;
    let attachment_path = write_attachment(&dir, "stdio-note.txt", b"stdio attachment");
    let send = handle_request_with_identity(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "send-attachment",
            "method": "tools/call",
            "params": {
                "name": "send_message",
                "arguments": {
                    "to": "codex-b",
                    "summary": "attachment direct",
                    "full": "attachment direct full",
                    "attachments": [{ "path": attachment_path, "mime": "text/plain" }]
                }
            }
        }),
        Some("codex-a"),
    )
    .await;
    assert_eq!(send["result"]["structuredContent"]["ok"], true, "{send}");

    let inbox = handle_request_with_identity(
        &host,
        json!({
            "jsonrpc": "2.0",
            "id": "read-attachment",
            "method": "tools/call",
            "params": {
                "name": "check_inbox",
                "arguments": { "agent_id": "codex-b", "drain": false }
            }
        }),
        Some("codex-b"),
    )
    .await;
    assert_eq!(
        inbox["result"]["structuredContent"]["dm"][0]["attachments"][0]["name"], "stdio-note.txt",
        "{inbox}"
    );
    assert_eq!(
        inbox["result"]["structuredContent"]["dm"][0]["attachments"][0]["staged"],
        false
    );
    assert_eq!(
        inbox["result"]["structuredContent"]["dm"][0]["attachments"][0]["source_path"],
        inbox["result"]["structuredContent"]["dm"][0]["attachments"][0]["path"]
    );
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
async fn mcp_stdio_proxy_tools_call_posts_to_http_daemon() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener addr");
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        let mut buf = vec![0_u8; 4096];
        let n = socket
            .readable()
            .await
            .and_then(|()| socket.try_read(&mut buf))
            .expect("read");
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(
            request.starts_with("POST /tools/call HTTP/1.1"),
            "{request}"
        );
        assert!(request.contains(r#""name":"query_run""#), "{request}");
        assert!(request.contains(r#""run_id":"r1""#), "{request}");
        let body = r#"{"current_node":"propose_spec"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.writable().await.expect("writable");
        socket
            .try_write(response.as_bytes())
            .expect("write response");
    });

    let response = handle_proxy_request(
        &format!("http://{addr}"),
        json!({
            "jsonrpc": "2.0",
            "id": "proxy-call",
            "method": "tools/call",
            "params": {
                "name": "query_run",
                "arguments": { "run_id": "r1" }
            }
        }),
    )
    .await;
    server.await.expect("server task");

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], "proxy-call");
    assert_eq!(
        response["result"]["structuredContent"]["current_node"],
        "propose_spec"
    );
}

#[tokio::test]
async fn mcp_stdio_proxy_injects_identity_into_forwarded_send_message() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind test listener");
    let addr = listener.local_addr().expect("listener addr");
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.expect("accept");
        let mut buf = vec![0_u8; 4096];
        let n = socket
            .readable()
            .await
            .and_then(|()| socket.try_read(&mut buf))
            .expect("read");
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(
            request.starts_with("POST /tools/call HTTP/1.1"),
            "{request}"
        );
        assert!(request.contains(r#""name":"send_message""#), "{request}");
        assert!(
            request.contains(r#""from_agent":"codex-worker""#),
            "{request}"
        );
        let body = r#"{"ok":true}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.writable().await.expect("writable");
        socket
            .try_write(response.as_bytes())
            .expect("write response");
    });

    let response = handle_proxy_request_with_identity(
        &format!("http://{addr}"),
        json!({
            "jsonrpc": "2.0",
            "id": "proxy-send",
            "method": "tools/call",
            "params": {
                "name": "send_message",
                "arguments": {
                    "to": "codex-reviewer",
                    "summary": "proxy identity",
                    "full": "proxy identity full"
                }
            }
        }),
        Some("codex-worker"),
    )
    .await;
    server.await.expect("server task");

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], "proxy-send");
    assert_eq!(response["result"]["structuredContent"]["ok"], true);
}

#[tokio::test]
async fn mcp_stdio_proxy_check_inbox_localizes_staged_attachments() {
    let cache = tempfile::tempdir().expect("cache dir");
    let source_path = "/daemon/media/source-note.txt";
    let tool_body = json!({
        "messages": [],
        "dm": [{
            "id": "msg1",
            "from": "codex-a",
            "to": "codex-b",
            "summary": "see attachment",
            "full": "see attachment full",
            "attachments": [{
                "path": source_path,
                "name": "source-note.txt",
                "mime": "text/plain",
                "kind": "file",
                "size": 999,
                "staged": true,
                "source_path": "/tmp/source-note.txt"
            }]
        }],
        "group": []
    });
    let (proxy_url, requests, server) =
        spawn_proxy_media_server(tool_body, b"localized bytes", "200 OK", 2).await;

    let response = handle_proxy_request_with_identity_and_media_cache(
        &proxy_url,
        json!({
            "jsonrpc": "2.0",
            "id": "proxy-inbox-localize",
            "method": "tools/call",
            "params": {
                "name": "check_inbox",
                "arguments": { "drain": false }
            }
        }),
        Some("codex-b"),
        Some(cache.path()),
    )
    .await;
    server.await.expect("server task");

    assert_eq!(response["jsonrpc"], "2.0", "{response}");
    let attachment = &response["result"]["structuredContent"]["dm"][0]["attachments"][0];
    let local_path = assert_path_inside(
        attachment["path"]
            .as_str()
            .expect("localized attachment path"),
        cache.path(),
    );
    assert_eq!(
        fs::read(&local_path).expect("cached file"),
        b"localized bytes"
    );
    assert_eq!(attachment["source_path"], "/tmp/source-note.txt");
    assert_eq!(attachment["name"], "source-note.txt");
    assert_eq!(attachment["mime"], "text/plain");
    assert_eq!(attachment["kind"], "file");
    assert_eq!(attachment["size"], 15);
    assert_eq!(attachment["staged"], true);

    let requests = requests.lock().expect("requests").clone();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(requests[0].starts_with("POST /tools/call HTTP/1.1"));
    assert!(requests[1].starts_with("GET /api/media/fetch?"));
    assert!(requests[1].contains("%2Fdaemon%2Fmedia%2Fsource-note.txt"));
}

#[tokio::test]
async fn mcp_stdio_proxy_check_group_localizes_localpath_lines() {
    let cache = tempfile::tempdir().expect("cache dir");
    let source_path = "/daemon/media/report.txt";
    let tool_body = json!({
        "group": "factory",
        "unread": [{
            "id": "grp1",
            "from": "codex-a",
            "group": "factory",
            "summary": "report",
            "full": format!("please inspect\nLocalPath: {source_path}\nthanks"),
            "attachments": []
        }],
        "read": [],
        "unread_total": 1,
        "unread_returned": 1,
        "unread_omitted": 0,
        "advance": "none"
    });
    let (proxy_url, _requests, server) =
        spawn_proxy_media_server(tool_body, b"group bytes", "200 OK", 2).await;

    let response = handle_proxy_request_with_identity_and_media_cache(
        &proxy_url,
        json!({
            "jsonrpc": "2.0",
            "id": "proxy-group-localpath",
            "method": "tools/call",
            "params": {
                "name": "check_group",
                "arguments": { "group": "factory" }
            }
        }),
        Some("codex-b"),
        Some(cache.path()),
    )
    .await;
    server.await.expect("server task");

    let message = &response["result"]["structuredContent"]["unread"][0];
    let full = message["full"].as_str().expect("full text");
    assert!(!full.contains(source_path), "{full}");
    let local_line = full
        .lines()
        .find_map(|line| line.strip_prefix("LocalPath: "))
        .expect("localized LocalPath line");
    let local_path = assert_path_inside(local_line, cache.path());
    assert_eq!(fs::read(&local_path).expect("cached file"), b"group bytes");
    assert_eq!(
        message["attachments"]
            .as_array()
            .expect("attachments")
            .len(),
        1
    );
    assert_eq!(message["attachments"][0]["path"], local_line);
    assert_eq!(message["attachments"][0]["source_path"], source_path);
}

#[tokio::test]
async fn mcp_stdio_proxy_media_localization_reuses_warm_cache() {
    let cache = tempfile::tempdir().expect("cache dir");
    let source_path = "/daemon/media/warm.txt";
    let tool_body = json!({
        "messages": [],
        "dm": [{
            "id": "warm1",
            "from": "codex-a",
            "to": "codex-b",
            "summary": "warm",
            "full": "warm",
            "attachments": [{
                "path": source_path,
                "name": "warm.txt",
                "mime": "text/plain",
                "kind": "file",
                "size": 10,
                "staged": true,
                "source_path": source_path
            }]
        }],
        "group": []
    });
    let (proxy_url, requests, server) =
        spawn_proxy_media_server(tool_body, b"warm bytes", "200 OK", 3).await;

    let request = json!({
        "jsonrpc": "2.0",
        "id": "proxy-warm",
        "method": "tools/call",
        "params": {
            "name": "check_inbox",
            "arguments": { "drain": false }
        }
    });
    let first = handle_proxy_request_with_identity_and_media_cache(
        &proxy_url,
        request.clone(),
        Some("codex-b"),
        Some(cache.path()),
    )
    .await;
    let second = handle_proxy_request_with_identity_and_media_cache(
        &proxy_url,
        request,
        Some("codex-b"),
        Some(cache.path()),
    )
    .await;
    server.await.expect("server task");

    let first_path = first["result"]["structuredContent"]["dm"][0]["attachments"][0]["path"]
        .as_str()
        .expect("first path")
        .to_string();
    let second_path = second["result"]["structuredContent"]["dm"][0]["attachments"][0]["path"]
        .as_str()
        .expect("second path")
        .to_string();
    assert_eq!(first_path, second_path);
    assert_eq!(fs::read(first_path).expect("cached file"), b"warm bytes");

    let requests = requests.lock().expect("requests").clone();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.starts_with("GET /api/media/fetch?"))
            .count(),
        1,
        "{requests:?}"
    );
}

#[tokio::test]
async fn mcp_stdio_proxy_media_localization_warns_without_failing_on_fetch_error() {
    let cache = tempfile::tempdir().expect("cache dir");
    let source_path = "/daemon/media/missing.txt";
    let tool_body = json!({
        "messages": [],
        "dm": [{
            "id": "missing1",
            "from": "codex-a",
            "to": "codex-b",
            "summary": "missing",
            "full": "missing full",
            "attachments": [{
                "path": source_path,
                "name": "missing.txt",
                "mime": "text/plain",
                "kind": "file",
                "size": 10,
                "staged": true,
                "source_path": source_path
            }]
        }],
        "group": []
    });
    let (proxy_url, _requests, server) = spawn_proxy_media_server(
        tool_body,
        br#"{"error":"file not found"}"#,
        "404 Not Found",
        2,
    )
    .await;

    let response = handle_proxy_request_with_identity_and_media_cache(
        &proxy_url,
        json!({
            "jsonrpc": "2.0",
            "id": "proxy-fetch-error",
            "method": "tools/call",
            "params": {
                "name": "check_inbox",
                "arguments": { "drain": false }
            }
        }),
        Some("codex-b"),
        Some(cache.path()),
    )
    .await;
    server.await.expect("server task");

    assert_eq!(response["jsonrpc"], "2.0", "{response}");
    let message = &response["result"]["structuredContent"]["dm"][0];
    assert_eq!(message["attachments"][0]["path"], source_path);
    let warnings = message["media_warnings"]
        .as_array()
        .expect("media warnings");
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .is_some_and(|value| value.contains(source_path))),
        "{message}"
    );
}

#[tokio::test]
async fn mcp_stdio_proxy_media_localization_keeps_tool_schemas_unchanged() {
    let (host, _dir) = production_host().await;
    let response = handle_request_with_identity(
        &host,
        json!({"jsonrpc": "2.0", "id": "schema-p223", "method": "tools/list"}),
        Some("codex-b"),
    )
    .await;

    let tools = response["result"]["tools"].as_array().expect("tools array");
    let check_inbox = tools
        .iter()
        .find(|tool| tool["name"] == "check_inbox")
        .expect("check_inbox tool");
    assert!(check_inbox["inputSchema"]["properties"]["attachments"].is_null());
    let check_group = tools
        .iter()
        .find(|tool| tool["name"] == "check_group")
        .expect("check_group tool");
    assert!(check_group["inputSchema"]["properties"]["attachments"].is_null());
    assert!(
        tools
            .iter()
            .all(|tool| tool["name"] != "stage_media" && tool["name"] != "fetch_media"),
        "{tools:?}"
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

#[tokio::test]
async fn mcp_stdio_tools_list_includes_submit_human_answer_schema() {
    let (host, _dir) = production_host().await;
    let response = handle_request(
        &host,
        json!({"jsonrpc": "2.0", "id": 12, "method": "tools/list"}),
    )
    .await;

    let tools = response["result"]["tools"].as_array().expect("tools array");
    let submit_human_answer = tools
        .iter()
        .find(|tool| tool["name"] == "submit_human_answer")
        .expect("submit_human_answer tool");
    let required = submit_human_answer["inputSchema"]["required"]
        .as_array()
        .expect("required array");
    for field in ["wait_id", "answer"] {
        assert!(
            required.iter().any(|value| value == field),
            "missing {field}: {submit_human_answer}"
        );
    }
    assert!(
        !required.iter().any(|value| value == "feedback"),
        "feedback should be optional: {submit_human_answer}"
    );
    assert_eq!(
        submit_human_answer["inputSchema"]["properties"]["feedback"]["type"],
        "string"
    );
}
