//! Line-delimited stdio JSON-RPC host for the agent-facing MCP dispatcher.
//!
//! This P119 entrypoint deliberately reuses `agentd-surface`'s existing
//! transport-agnostic dispatcher. It gives real local agent processes a stable
//! process boundary while the external `rmcp` crate version is settled in a
//! later compatibility slice.

use std::collections::BTreeMap;
use std::env;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use agentd_surface::error::SurfaceError;
use agentd_surface::host::RunHost;
use agentd_surface::mcp_server::{dispatch, tool_descriptors};
use rmcp::model::{
    CallToolResult, Implementation, InitializeResult, JsonObject, ProtocolVersion,
    ServerCapabilities, Tool,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

pub const AGENTD_AGENT_ID_ENV: &str = "AGENTD_AGENT_ID";
pub const AGENTD_AGENT_NAME_ENV: &str = "AGENTD_AGENT_NAME";
pub const AGENT_CHAT_AGENT_NAME_ENV: &str = "AGENT_NAME";
pub const AGENTD_MCP_MEDIA_CACHE_DIR_ENV: &str = "AGENTD_MCP_MEDIA_CACHE_DIR";
const LOCALIZED_MEDIA_MAX_BYTES: usize = 20 * 1024 * 1024;

/// Resolve the stdio session's agent identity.
///
/// CLI wins, then agentd-specific env vars, then agent-chat's `AGENT_NAME`
/// compatibility variable.
#[must_use]
pub fn identity_from_cli_or_env(cli_agent_id: Option<&str>) -> Option<String> {
    clean_identity(cli_agent_id)
        .or_else(|| clean_identity(std::env::var(AGENTD_AGENT_ID_ENV).ok().as_deref()))
        .or_else(|| clean_identity(std::env::var(AGENTD_AGENT_NAME_ENV).ok().as_deref()))
        .or_else(|| clean_identity(std::env::var(AGENT_CHAT_AGENT_NAME_ENV).ok().as_deref()))
}

fn clean_identity(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Handle one JSON-RPC request object.
pub async fn handle_request(host: &dyn RunHost, request: Value) -> Value {
    handle_request_with_identity(host, request, None).await
}

/// Handle one JSON-RPC request object for an identity-bound stdio session.
pub async fn handle_request_with_identity(
    host: &dyn RunHost,
    request: Value,
    agent_id: Option<&str>,
) -> Value {
    let identity = clean_identity(agent_id);
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return error_response(id, -32600, "invalid request", None);
    };

    match method {
        "initialize" => success_response(id, initialize_result(identity.as_deref())),
        "tools/list" => success_response(id, tools_list_result(identity.as_deref())),
        "tools/call" => handle_tools_call(host, id, &request, identity.as_deref()).await,
        other => error_response(id, -32601, format!("method not found: {other}"), None),
    }
}

/// Handle one JSON-RPC request object in proxy mode.
pub async fn handle_proxy_request(proxy_url: &str, request: Value) -> Value {
    handle_proxy_request_with_identity(proxy_url, request, None).await
}

/// Handle one JSON-RPC request object in proxy mode for an identity-bound
/// stdio session.
pub async fn handle_proxy_request_with_identity(
    proxy_url: &str,
    request: Value,
    agent_id: Option<&str>,
) -> Value {
    handle_proxy_request_with_identity_and_media_cache(proxy_url, request, agent_id, None).await
}

/// Handle one JSON-RPC request object in proxy mode with an explicit media
/// cache root. Public for deterministic proxy localization tests.
pub async fn handle_proxy_request_with_identity_and_media_cache(
    proxy_url: &str,
    request: Value,
    agent_id: Option<&str>,
    media_cache_dir: Option<&Path>,
) -> Value {
    let identity = clean_identity(agent_id);
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return error_response(id, -32600, "invalid request", None);
    };

    match method {
        "initialize" => success_response(id, initialize_result(identity.as_deref())),
        "tools/list" => success_response(id, tools_list_result(identity.as_deref())),
        "tools/call" => {
            handle_proxy_tools_call(
                proxy_url,
                id,
                &request,
                identity.as_deref(),
                media_cache_dir,
            )
            .await
        }
        other => error_response(id, -32601, format!("method not found: {other}"), None),
    }
}

/// Serve line-delimited JSON-RPC requests from `reader`, writing one response
/// line per request to `writer`.
pub async fn serve_json_lines<R, W>(host: &dyn RunHost, reader: R, writer: W) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    serve_json_lines_with_identity(host, reader, writer, None).await
}

/// Serve line-delimited JSON-RPC requests for an identity-bound stdio session.
pub async fn serve_json_lines_with_identity<R, W>(
    host: &dyn RunHost,
    mut reader: R,
    mut writer: W,
    agent_id: Option<&str>,
) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let identity = clean_identity(agent_id);
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
            Ok(request) => handle_request_with_identity(host, request, identity.as_deref()).await,
            Err(err) => error_response(Value::Null, -32700, format!("parse error: {err}"), None),
        };
        writer.write_all(response.to_string().as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }
    Ok(())
}

/// Serve line-delimited JSON-RPC requests in proxy mode.
pub async fn serve_proxy_json_lines<R, W>(
    proxy_url: &str,
    reader: R,
    writer: W,
) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    serve_proxy_json_lines_with_identity(proxy_url, reader, writer, None).await
}

/// Serve line-delimited JSON-RPC requests in proxy mode for an identity-bound
/// stdio session.
pub async fn serve_proxy_json_lines_with_identity<R, W>(
    proxy_url: &str,
    mut reader: R,
    mut writer: W,
    agent_id: Option<&str>,
) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let identity = clean_identity(agent_id);
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
            Ok(request) => {
                handle_proxy_request_with_identity(proxy_url, request, identity.as_deref()).await
            }
            Err(err) => error_response(Value::Null, -32700, format!("parse error: {err}"), None),
        };
        writer.write_all(response.to_string().as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }
    Ok(())
}

async fn handle_tools_call(
    host: &dyn RunHost,
    id: Value,
    request: &Value,
    identity: Option<&str>,
) -> Value {
    let (name, arguments) = match parse_tools_call(&id, request) {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let arguments = match apply_identity(name, arguments, identity) {
        Ok(arguments) => arguments,
        Err(err) => {
            return error_response(
                id,
                -32000,
                err.to_string(),
                Some(json!({ "code": err.code() })),
            );
        }
    };

    match dispatch(host, name, arguments).await {
        Ok(result) => call_tool_success_response(id, result),
        Err(err) => error_response(
            id,
            -32000,
            err.to_string(),
            Some(json!({ "code": err.code() })),
        ),
    }
}

async fn handle_proxy_tools_call(
    proxy_url: &str,
    id: Value,
    request: &Value,
    identity: Option<&str>,
    media_cache_dir: Option<&Path>,
) -> Value {
    let (name, arguments) = match parse_tools_call(&id, request) {
        Ok(parsed) => parsed,
        Err(response) => return response,
    };
    let arguments = match apply_identity(name, arguments, identity) {
        Ok(arguments) => arguments,
        Err(err) => {
            return error_response(
                id,
                -32000,
                err.to_string(),
                Some(json!({ "code": err.code() })),
            );
        }
    };

    match proxy_tool_call(proxy_url, name, arguments).await {
        Ok(result) => {
            let result =
                localize_proxy_tool_result(proxy_url, name, result, identity, media_cache_dir)
                    .await;
            call_tool_success_response(id, result)
        }
        Err(err) => error_response(
            id,
            -32000,
            format!("proxy tools/call failed: {err}"),
            Some(json!({ "code": "proxy" })),
        ),
    }
}

fn parse_tools_call<'a>(id: &Value, request: &'a Value) -> Result<(&'a str, Value), Value> {
    let Some(params) = request.get("params").and_then(Value::as_object) else {
        return Err(error_response(
            id.clone(),
            -32602,
            "missing tools/call params",
            None,
        ));
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return Err(error_response(
            id.clone(),
            -32602,
            "missing tools/call params.name",
            None,
        ));
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    Ok((name, arguments))
}

fn apply_identity(
    name: &str,
    arguments: Value,
    identity: Option<&str>,
) -> Result<Value, SurfaceError> {
    let Some(identity) = clean_identity(identity) else {
        return Ok(arguments);
    };
    match name {
        "send_message" => bind_send_message_identity(arguments, &identity),
        "check_inbox" => bind_check_inbox_identity(arguments, &identity),
        "post" => bind_post_identity(arguments, &identity),
        "check_group" => bind_check_group_identity(arguments, &identity),
        _ => Ok(arguments),
    }
}

fn bind_send_message_identity(arguments: Value, identity: &str) -> Result<Value, SurfaceError> {
    let mut args = object_arguments(arguments, "send_message arguments must be an object")?;
    let explicit = clean_identity(value_str(args.get("from_agent")))
        .or_else(|| clean_identity(value_str(args.get("from"))));
    if let Some(explicit) = explicit.as_deref()
        && explicit != identity
    {
        return Err(SurfaceError::BadRequest(format!(
            "from_agent does not match stdio identity {identity}"
        )));
    }
    args.remove("from");
    args.insert(
        "from_agent".to_string(),
        Value::String(identity.to_string()),
    );
    Ok(Value::Object(args))
}

fn bind_check_inbox_identity(arguments: Value, identity: &str) -> Result<Value, SurfaceError> {
    let mut args = object_arguments(arguments, "check_inbox arguments must be an object")?;
    if let Some(explicit) = clean_identity(value_str(args.get("agent_id"))).as_deref()
        && explicit != identity
    {
        return Err(SurfaceError::BadRequest(format!(
            "agent_id does not match stdio identity {identity}"
        )));
    }
    args.insert("agent_id".to_string(), Value::String(identity.to_string()));
    Ok(Value::Object(args))
}

fn bind_post_identity(arguments: Value, identity: &str) -> Result<Value, SurfaceError> {
    let mut args = object_arguments(arguments, "post arguments must be an object")?;
    let explicit = clean_identity(value_str(args.get("from_agent")))
        .or_else(|| clean_identity(value_str(args.get("from"))));
    if let Some(explicit) = explicit.as_deref()
        && explicit != identity
    {
        return Err(SurfaceError::BadRequest(format!(
            "from_agent does not match stdio identity {identity}"
        )));
    }
    args.remove("from");
    args.insert(
        "from_agent".to_string(),
        Value::String(identity.to_string()),
    );
    Ok(Value::Object(args))
}

fn bind_check_group_identity(arguments: Value, identity: &str) -> Result<Value, SurfaceError> {
    let mut args = object_arguments(arguments, "check_group arguments must be an object")?;
    if let Some(explicit) = clean_identity(value_str(args.get("agent_id"))).as_deref()
        && explicit != identity
    {
        return Err(SurfaceError::BadRequest(format!(
            "agent_id does not match stdio identity {identity}"
        )));
    }
    args.insert("agent_id".to_string(), Value::String(identity.to_string()));
    Ok(Value::Object(args))
}

fn object_arguments(arguments: Value, message: &str) -> Result<Map<String, Value>, SurfaceError> {
    match arguments {
        Value::Object(map) => Ok(map),
        _ => Err(SurfaceError::BadRequest(message.to_string())),
    }
}

fn value_str(value: Option<&Value>) -> Option<&str> {
    value.and_then(Value::as_str)
}

fn call_tool_success_response(id: Value, result: Value) -> Value {
    match serde_json::to_value(CallToolResult::structured(result)) {
        Ok(value) => success_response(id, value),
        Err(err) => error_response(id, -32603, format!("encode tool result: {err}"), None),
    }
}

#[derive(Debug)]
struct ProxyEndpoint {
    host: String,
    port: u16,
    base_path: String,
    tool_path: String,
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

struct MediaLocalizationCtx<'a> {
    proxy_url: &'a str,
    cache_root: PathBuf,
}

async fn proxy_tool_call(proxy_url: &str, name: &str, arguments: Value) -> Result<Value, String> {
    let endpoint = parse_proxy_endpoint(proxy_url)?;
    let body = json!({
        "name": name,
        "arguments": arguments
    })
    .to_string();
    let mut stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
        .await
        .map_err(|err| format!("connect {}:{}: {err}", endpoint.host, endpoint.port))?;
    let request = format!(
        "POST {} HTTP/1.1\r\n\
         Host: {}:{}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n\
         {}",
        endpoint.tool_path,
        endpoint.host,
        endpoint.port,
        body.len(),
        body
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| format!("write request: {err}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|err| format!("read response: {err}"))?;
    decode_http_json_response(&response)
}

fn parse_proxy_endpoint(proxy_url: &str) -> Result<ProxyEndpoint, String> {
    let rest = proxy_url
        .strip_prefix("http://")
        .ok_or_else(|| "only http:// proxy URLs are supported".to_string())?;
    let (authority, base_path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, format!("/{path}")),
        None => (rest, String::new()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, raw_port)) if !raw_port.is_empty() => {
            let port = raw_port
                .parse::<u16>()
                .map_err(|err| format!("invalid proxy port {raw_port}: {err}"))?;
            (host.to_string(), port)
        }
        _ => (authority.to_string(), 80),
    };
    if host.is_empty() {
        return Err("proxy host is empty".to_string());
    }
    let prefix = base_path.trim_end_matches('/');
    let tool_path = if prefix.is_empty() {
        "/tools/call".to_string()
    } else {
        format!("{prefix}/tools/call")
    };
    Ok(ProxyEndpoint {
        host,
        port,
        base_path: prefix.to_string(),
        tool_path,
    })
}

fn decode_http_json_response(response: &[u8]) -> Result<Value, String> {
    let response = decode_http_response(response)?;
    if !(200..300).contains(&response.status) {
        let body = String::from_utf8_lossy(&response.body);
        return Err(format!("daemon returned HTTP {}: {body}", response.status));
    }
    serde_json::from_slice(&response.body).map_err(|err| format!("decode JSON body: {err}"))
}

fn decode_http_response(response: &[u8]) -> Result<HttpResponse, String> {
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "response missing header terminator".to_string())?;
    let head = std::str::from_utf8(&response[..header_end])
        .map_err(|err| format!("response header is not UTF-8: {err}"))?;
    let mut lines = head.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| "response missing status line".to_string())?;
    let mut status_parts = status_line.split_whitespace();
    let _http_version = status_parts
        .next()
        .ok_or_else(|| "response missing HTTP version".to_string())?;
    let raw_status = status_parts
        .next()
        .ok_or_else(|| "response missing status code".to_string())?;
    let status = raw_status
        .parse::<u16>()
        .map_err(|err| format!("invalid status code {raw_status}: {err}"))?;
    let mut headers = BTreeMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Ok(HttpResponse {
        status,
        headers,
        body: response[header_end + 4..].to_vec(),
    })
}

async fn localize_proxy_tool_result(
    proxy_url: &str,
    name: &str,
    result: Value,
    identity: Option<&str>,
    media_cache_dir: Option<&Path>,
) -> Value {
    if name != "check_inbox" && name != "check_group" {
        return result;
    }
    let Value::Object(mut map) = result else {
        return result;
    };
    let ctx = MediaLocalizationCtx {
        proxy_url,
        cache_root: media_cache_root(media_cache_dir, identity),
    };
    if name == "check_inbox" {
        for key in ["messages", "dm", "group"] {
            localize_message_array(&mut map, key, &ctx).await;
        }
    } else {
        for key in ["unread", "read"] {
            localize_message_array(&mut map, key, &ctx).await;
        }
    }
    Value::Object(map)
}

fn media_cache_root(explicit: Option<&Path>, identity: Option<&str>) -> PathBuf {
    if let Some(path) = explicit {
        return path.to_path_buf();
    }
    if let Some(path) = env_path(AGENTD_MCP_MEDIA_CACHE_DIR_ENV) {
        return path;
    }
    if let Some(state_dir) = env_path("AGENTCHAT_AGENT_STATE_DIR") {
        return state_dir.join("mcp-media-cache");
    }
    let agent = safe_path_segment(identity.unwrap_or("agent"), "agent");
    if let Some(runtime_dir) = env_path("AGENT_CHAT_RUNTIME_DIR") {
        return runtime_dir.join("data").join("mcp-media-cache").join(agent);
    }
    env::temp_dir().join("agentd-mcp-media-cache").join(agent)
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

async fn localize_message_array(
    map: &mut Map<String, Value>,
    key: &str,
    ctx: &MediaLocalizationCtx<'_>,
) {
    let Some(Value::Array(messages)) = map.get_mut(key) else {
        return;
    };
    for message in messages {
        let original = std::mem::take(message);
        *message = localize_message_media(original, ctx).await;
    }
}

async fn localize_message_media(message: Value, ctx: &MediaLocalizationCtx<'_>) -> Value {
    let Value::Object(mut map) = message else {
        return message;
    };
    let mut warnings = Vec::new();
    let mut localized_attachments = Vec::new();
    if let Some(Value::Array(attachments)) = map.remove("attachments") {
        for raw in attachments {
            match localize_attachment_value(&raw, ctx).await {
                Ok(localized) => localized_attachments.push(localized),
                Err(err) => {
                    warnings.push(err);
                    localized_attachments.push(raw);
                }
            }
        }
    }

    for field in ["full", "summary"] {
        let Some(text) = map.get(field).and_then(Value::as_str).map(str::to_string) else {
            continue;
        };
        let (text, attachments, text_warnings) = localize_text_local_paths(&text, ctx).await;
        map.insert(field.to_string(), Value::String(text));
        localized_attachments.extend(attachments);
        warnings.extend(text_warnings);
    }

    map.insert(
        "attachments".to_string(),
        Value::Array(merge_attachments_by_path(localized_attachments)),
    );
    if !warnings.is_empty() {
        let mut all = map
            .remove("media_warnings")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        all.extend(warnings.into_iter().map(Value::String));
        map.insert("media_warnings".to_string(), Value::Array(all));
    }
    Value::Object(map)
}

async fn localize_text_local_paths(
    text: &str,
    ctx: &MediaLocalizationCtx<'_>,
) -> (String, Vec<Value>, Vec<String>) {
    if !text.contains("LocalPath:") {
        return (text.to_string(), Vec::new(), Vec::new());
    }
    let mut lines = Vec::new();
    let mut attachments = Vec::new();
    let mut warnings = Vec::new();
    for line in text.lines() {
        let Some(source) = line.strip_prefix("LocalPath:") else {
            lines.push(line.to_string());
            continue;
        };
        let source = source.trim();
        if source.is_empty() {
            lines.push(line.to_string());
            continue;
        }
        let hint = json!({
            "path": source,
            "name": path_basename(source).unwrap_or_else(|| "file".to_string()),
            "source_path": source,
            "staged": true,
        });
        match localize_attachment_value(&hint, ctx).await {
            Ok(localized) => {
                let local_path = localized
                    .get("path")
                    .and_then(Value::as_str)
                    .unwrap_or(source);
                lines.push(format!("LocalPath: {local_path}"));
                attachments.push(localized);
            }
            Err(err) => {
                warnings.push(err);
                lines.push(line.to_string());
            }
        }
    }
    (lines.join("\n"), attachments, warnings)
}

async fn localize_attachment_value(
    raw: &Value,
    ctx: &MediaLocalizationCtx<'_>,
) -> Result<Value, String> {
    let Value::Object(raw_map) = raw else {
        return Err("media localization skipped non-object attachment".to_string());
    };
    let source = string_field(raw_map, "path")
        .ok_or_else(|| "media localization skipped attachment without path".to_string())?;
    let source_path = string_field(raw_map, "source_path").unwrap_or_else(|| source.clone());
    let fallback_name = path_basename(&source).unwrap_or_else(|| "file".to_string());
    let name = normalize_attachment_name(string_field(raw_map, "name"), &fallback_name);
    let mime = normalize_mime(string_field(raw_map, "mime"));
    let kind = normalize_kind(string_field(raw_map, "kind"), mime.as_deref(), &name);

    if let Some(size) = readable_file_size(Path::new(&source)) {
        return Ok(localized_attachment(
            raw_map,
            &source,
            &name,
            mime,
            &kind,
            size,
            &source_path,
        ));
    }

    let cached_path = media_cache_path(&ctx.cache_root, &source, &name, mime.as_deref());
    if let Some(size) = readable_file_size(&cached_path) {
        return Ok(localized_attachment(
            raw_map,
            &cached_path.to_string_lossy(),
            &name,
            mime,
            &kind,
            size,
            &source_path,
        ));
    }

    let fetched = fetch_media(ctx.proxy_url, &source).await?;
    if fetched.body.is_empty() {
        return Err(format!("media fetch returned empty file for {source}"));
    }
    if fetched.body.len() > LOCALIZED_MEDIA_MAX_BYTES {
        return Err(format!(
            "media fetch too large for {source}: {} bytes > {LOCALIZED_MEDIA_MAX_BYTES}",
            fetched.body.len()
        ));
    }
    let header_name = fetched
        .headers
        .get("content-disposition")
        .and_then(|value| parse_content_disposition_filename(value));
    let final_name = normalize_attachment_name(header_name.or(Some(name)), &fallback_name);
    let final_mime = fetched
        .headers
        .get("content-type")
        .and_then(|value| normalize_mime(Some(value.clone())))
        .or(mime);
    let final_kind = normalize_kind(None, final_mime.as_deref(), &final_name);
    let target_path =
        media_cache_path(&ctx.cache_root, &source, &final_name, final_mime.as_deref());
    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| format!("create media cache {}: {err}", parent.display()))?;
    }
    fs::write(&target_path, &fetched.body)
        .map_err(|err| format!("write media cache {}: {err}", target_path.display()))?;
    Ok(localized_attachment(
        raw_map,
        &target_path.to_string_lossy(),
        &final_name,
        final_mime,
        &final_kind,
        fetched.body.len() as u64,
        &source_path,
    ))
}

fn localized_attachment(
    raw: &Map<String, Value>,
    path: &str,
    name: &str,
    mime: Option<String>,
    kind: &str,
    size: u64,
    source_path: &str,
) -> Value {
    let mut out = raw.clone();
    out.insert("path".to_string(), Value::String(path.to_string()));
    out.insert("name".to_string(), Value::String(name.to_string()));
    out.insert("mime".to_string(), mime.map_or(Value::Null, Value::String));
    out.insert("kind".to_string(), Value::String(kind.to_string()));
    out.insert("size".to_string(), Value::from(size));
    out.insert("staged".to_string(), Value::Bool(true));
    out.insert(
        "source_path".to_string(),
        Value::String(source_path.to_string()),
    );
    Value::Object(out)
}

async fn fetch_media(proxy_url: &str, source: &str) -> Result<HttpResponse, String> {
    let endpoint = parse_proxy_endpoint(proxy_url)?;
    let media_path = if endpoint.base_path.is_empty() {
        format!("/api/media/fetch?path={}", percent_encode_query(source))
    } else {
        format!(
            "{}/api/media/fetch?path={}",
            endpoint.base_path,
            percent_encode_query(source)
        )
    };
    let request = format!(
        "GET {media_path} HTTP/1.1\r\n\
         Host: {}:{}\r\n\
         Connection: close\r\n\
         \r\n",
        endpoint.host, endpoint.port
    );
    let mut stream = TcpStream::connect((endpoint.host.as_str(), endpoint.port))
        .await
        .map_err(|err| format!("connect {}:{}: {err}", endpoint.host, endpoint.port))?;
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| format!("write media fetch request: {err}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|err| format!("read media fetch response: {err}"))?;
    let response = decode_http_response(&response)?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "media fetch failed for {source}: HTTP {}",
            response.status
        ));
    }
    Ok(response)
}

fn media_cache_path(root: &Path, source: &str, name: &str, mime: Option<&str>) -> PathBuf {
    let ext = Path::new(name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .or_else(|| mime.and_then(ext_from_mime))
        .or_else(|| {
            Path::new(source)
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| format!(".{value}"))
        })
        .unwrap_or_else(|| ".bin".to_string());
    let stem_fallback = path_basename(name).unwrap_or_else(|| "file".to_string());
    let stem = Path::new(&stem_fallback)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let stem = safe_path_segment(stem, "file");
    root.join(format!("{}-{stem}{ext}", sha256_hex_16(source.as_bytes())))
}

fn sha256_hex_16(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::new();
    for byte in digest.iter().take(8) {
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn percent_encode_query(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            let _ = write!(&mut out, "%{byte:02X}");
        }
    }
    out
}

fn readable_file_size(path: &Path) -> Option<u64> {
    let stat = fs::metadata(path).ok()?;
    (stat.is_file() && stat.len() > 0).then_some(stat.len())
}

fn string_field(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn path_basename(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn normalize_attachment_name(value: Option<String>, fallback: &str) -> String {
    let raw = value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_string());
    let base = Path::new(&raw)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(fallback);
    safe_path_segment(base, fallback)
}

fn safe_path_segment(value: &str, fallback: &str) -> String {
    let mut out = String::new();
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let out = out.trim_matches('.').to_string();
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

fn normalize_mime(value: Option<String>) -> Option<String> {
    let value = value?.split(';').next()?.trim().to_ascii_lowercase();
    let (left, right) = value.split_once('/')?;
    let valid = |part: &str| {
        !part.is_empty()
            && part.chars().all(|ch| {
                ch.is_ascii_alphanumeric()
                    || matches!(ch, '!' | '#' | '$' | '&' | '^' | '_' | '.' | '+' | '-')
            })
    };
    (valid(left) && valid(right)).then_some(value)
}

fn normalize_kind(raw_kind: Option<String>, mime: Option<&str>, name: &str) -> String {
    if let Some(kind) = raw_kind
        && (kind == "image" || kind == "file")
    {
        return kind;
    }
    if mime.is_some_and(|value| value.starts_with("image/")) {
        return "image".to_string();
    }
    let lower = name.to_ascii_lowercase();
    if [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".svg", ".avif", ".heic", ".heif",
        ".tif", ".tiff",
    ]
    .iter()
    .any(|suffix| lower.ends_with(suffix))
    {
        return "image".to_string();
    }
    "file".to_string()
}

fn ext_from_mime(mime: &str) -> Option<String> {
    let ext = match mime {
        "image/png" => ".png",
        "image/jpeg" => ".jpg",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "image/bmp" => ".bmp",
        "image/svg+xml" => ".svg",
        "application/pdf" => ".pdf",
        "text/plain" => ".txt",
        "text/markdown" => ".md",
        "application/json" => ".json",
        _ => return None,
    };
    Some(ext.to_string())
}

fn parse_content_disposition_filename(value: &str) -> Option<String> {
    for part in value.split(';').map(str::trim) {
        let Some(raw) = part.strip_prefix("filename=") else {
            continue;
        };
        let trimmed = raw.trim_matches('"').trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    None
}

fn merge_attachments_by_path(items: Vec<Value>) -> Vec<Value> {
    let mut out = Vec::new();
    let mut seen = Vec::new();
    for item in items {
        let Some(path) = item
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if seen.iter().any(|existing| existing == path) {
            continue;
        }
        seen.push(path.to_string());
        out.push(item);
    }
    out
}

fn initialize_result(identity: Option<&str>) -> Value {
    let instructions = match identity {
        Some(agent_id) => format!(
            "This stdio session is bound to agent '{agent_id}'. Use tools/list to inspect tools, then tools/call with name and arguments to submit outcomes, reviews, or messages."
        ),
        None => "Use tools/list to inspect tools, then tools/call with name and arguments to submit outcomes or reviews.".to_string(),
    };
    let result = InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
        .with_protocol_version(ProtocolVersion::LATEST)
        .with_server_info(Implementation::new("agentd", env!("CARGO_PKG_VERSION")))
        .with_instructions(instructions);
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

fn tools_list_result(identity: Option<&str>) -> Value {
    let tools: Vec<Value> = tool_descriptors()
        .into_iter()
        .map(|tool| {
            let mcp_tool = Tool::new(
                tool.name,
                tool.description,
                input_schema_for_tool(tool.name, identity),
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

fn input_schema_for_tool(name: &str, identity: Option<&str>) -> JsonObject {
    let identity_bound = identity.is_some();
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
        "submit_human_answer" => json!({
            "type": "object",
            "properties": {
                "wait_id": { "type": "string" },
                "answer": { "type": "string" },
                "feedback": { "type": "string" }
            },
            "required": ["wait_id", "answer"]
        }),
        "send_message" => send_message_schema(identity_bound),
        "post" => post_schema(identity_bound),
        "check_inbox" => check_inbox_schema(identity_bound),
        "check_group" => check_group_schema(identity_bound),
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

fn post_schema(identity_bound: bool) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "from_agent": {
                "type": "string",
                "description": if identity_bound {
                    "Sender is bound to this stdio session; omit normally. If provided, it must match the bound identity."
                } else {
                    "Sender agent name. The JSON alias 'from' is also accepted by tools/call."
                }
            },
            "group": { "type": "string" },
            "summary": { "type": "string" },
            "full": { "type": "string" },
            "type": {
                "type": "string",
                "enum": ["request", "inform", "reply", "human"],
                "default": "inform"
            },
            "priority": {
                "type": "string",
                "enum": ["normal", "high", "urgent"],
                "default": "normal"
            },
            "mentions": {
                "type": "array",
                "items": { "type": "string" }
            },
            "attachments": attachment_array_schema(),
            "reply_to": { "type": "string" },
            "schema": { "type": "object" }
        }
    });
    schema["required"] = if identity_bound {
        json!(["group", "summary", "full"])
    } else {
        json!(["from_agent", "group", "summary", "full"])
    };
    schema
}

fn send_message_schema(identity_bound: bool) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "from_agent": {
                "type": "string",
                "description": if identity_bound {
                    "Sender is bound to this stdio session; omit normally. If provided, it must match the bound identity."
                } else {
                    "Sender agent name. The JSON alias 'from' is also accepted by tools/call."
                }
            },
            "to": { "type": "string" },
            "summary": { "type": "string" },
            "full": { "type": "string" },
            "type": {
                "type": "string",
                "enum": ["request", "inform", "reply"],
                "default": "inform"
            },
            "priority": {
                "type": "string",
                "enum": ["normal", "high", "urgent"],
                "default": "normal"
            },
            "attachments": attachment_array_schema(),
            "reply_to": { "type": "string" }
        }
    });
    schema["required"] = if identity_bound {
        json!(["to", "summary", "full"])
    } else {
        json!(["from_agent", "to", "summary", "full"])
    };
    schema
}

fn check_inbox_schema(identity_bound: bool) -> Value {
    json!({
        "type": "object",
        "properties": {
            "agent_id": {
                "type": "string",
                "description": if identity_bound {
                    "Agent id is bound to this stdio session; omit normally. If provided, it must match the bound identity."
                } else {
                    "Agent id whose direct inbox should be read."
                }
            },
            "drain": { "type": "boolean" }
        },
        "required": if identity_bound {
            json!([])
        } else {
            json!(["agent_id"])
        }
    })
}

fn attachment_array_schema() -> Value {
    json!({
        "type": "array",
        "maxItems": 8,
        "items": {
            "anyOf": [
                {
                    "type": "string",
                    "description": "Local file path on this machine"
                },
                {
                    "type": "object",
                    "properties": {
                        "path": {
                            "type": "string",
                            "description": "Local file path on this machine"
                        },
                        "name": { "type": "string" },
                        "mime": { "type": "string" },
                        "kind": {
                            "type": "string",
                            "enum": ["image", "file"]
                        }
                    },
                    "required": ["path"]
                }
            ]
        }
    })
}

fn check_group_schema(identity_bound: bool) -> Value {
    json!({
        "type": "object",
        "properties": {
            "group": { "type": "string" },
            "agent_id": {
                "type": "string",
                "description": if identity_bound {
                    "Agent id is bound to this stdio session; omit normally. If provided, it must match the bound identity."
                } else {
                    "Agent id reading group history."
                }
            },
            "limit": { "type": "integer", "minimum": 1, "maximum": 200 },
            "unread_limit": { "type": "integer", "minimum": 1, "maximum": 500 },
            "read_all": { "type": "boolean" }
        },
        "required": if identity_bound {
            json!(["group"])
        } else {
            json!(["group", "agent_id"])
        }
    })
}

fn empty_input_schema() -> JsonObject {
    let mut schema = Map::new();
    schema.insert("type".to_string(), Value::String("object".to_string()));
    schema
}

fn success_response(id: Value, result: Value) -> Value {
    let mut response = Map::new();
    response.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
    response.insert("id".to_string(), id);
    response.insert("result".to_string(), result);
    Value::Object(response)
}

fn error_response(id: Value, code: i64, message: impl Into<String>, data: Option<Value>) -> Value {
    let mut error = Map::new();
    error.insert("code".to_string(), json!(code));
    error.insert("message".to_string(), json!(message.into()));
    if let Some(data) = data {
        error.insert("data".to_string(), data);
    }
    let mut response = Map::new();
    response.insert("jsonrpc".to_string(), Value::String("2.0".to_string()));
    response.insert("id".to_string(), id);
    response.insert("error".to_string(), Value::Object(error));
    Value::Object(response)
}
