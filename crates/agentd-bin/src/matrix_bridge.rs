//! Matrix bridge one-shot command composition.

use std::fmt::Write as FmtWrite;
use std::io::{Read, Write as IoWrite};
use std::net::TcpStream;
use std::time::Duration;

use agentd_matrix::{
    BridgeConfig, BridgeError, BridgeOnceConfig, BridgeOncePuppetAccountConfig, BridgeOnceReport,
    MatrixBotCommandAcl, MatrixClientBridgeOnceConfig, MatrixClientPort,
    MatrixClientTransportConfig, MatrixPuppetDirectory, MatrixPuppetHttpAccountConfig,
    MatrixPuppetProvisioningConfig, MatrixTrustMode,
};
#[cfg(feature = "matrix-sdk-adapter")]
use agentd_matrix::{SdkMatrixClient, SdkMatrixClientConfig};
use serde_json::Value;

use crate::cli::{DaemonConfig, MatrixBridgeOnceArgs, MatrixClientBridgeServiceArgs};

/// Bounded daemon-side service configuration for SDK-facing Matrix bridge runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixClientBridgeServiceConfig {
    /// One bridge iteration configuration reused for each bounded service pass.
    pub once: MatrixClientBridgeOnceConfig,
    /// Positive number of iterations to execute.
    pub iterations: usize,
}

/// Summary of a bounded Matrix client bridge service run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixClientBridgeServiceReport {
    /// Per-iteration reports in execution order.
    pub iterations: Vec<BridgeOnceReport>,
    /// Confirmed cursor after the last fully successful iteration.
    pub next_from_seq: i64,
    /// Total Matrix bot command replies sent across all iterations.
    pub bot_command_replies_sent: usize,
}

/// Operator preflight report for the Matrix client bridge service path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixClientBridgePreflightReport {
    /// Validated positive bounded iteration count from the service config.
    pub iterations: usize,
    /// Whether optional Matrix puppet account provisioning is configured.
    pub puppet_accounts_configured: bool,
    /// Read-only Matrix homeserver probe result.
    pub homeserver: MatrixHomeserverPreflightReport,
}

/// Read-only Matrix homeserver probe result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatrixHomeserverPreflightReport {
    /// Normalized Matrix homeserver URL used for HTTP probes.
    pub homeserver_url: String,
    /// Versions advertised by `/_matrix/client/versions`.
    pub versions: Vec<String>,
    /// User id returned by `/account/whoami` when an access token is supplied.
    pub whoami_user_id: Option<String>,
}

/// Run one deterministic Matrix bridge iteration from the CLI arguments.
pub fn run_matrix_bridge_once(
    config: &DaemonConfig,
    args: &MatrixBridgeOnceArgs,
) -> Result<BridgeOnceReport, BridgeError> {
    let puppet_accounts = matrix_bridge_once_puppet_account_config(args)?;
    let mut bridge_config = BridgeConfig::new(&args.agentd_api)?;
    if let Some(token) = config.auth_config().api_token {
        bridge_config = bridge_config.with_operator_token(token);
    }
    let once_config = BridgeOnceConfig {
        bridge_config,
        state_path: args.state.clone(),
        rooms_json_path: args.rooms_json.clone(),
        inbound_json_path: args.inbound_json.clone(),
        sent_log_jsonl_path: args.sent_log_jsonl.clone(),
        puppet_accounts,
    };
    agentd_matrix::run_bridge_once(&once_config)
}

/// Build a bounded Matrix client bridge service config from daemon and CLI args.
pub fn matrix_client_bridge_service_config(
    config: &DaemonConfig,
    args: &MatrixClientBridgeServiceArgs,
) -> Result<MatrixClientBridgeServiceConfig, BridgeError> {
    if args.iterations == 0 {
        return Err(BridgeError::invalid_config(
            "--iterations must be greater than 0 for matrix-client-bridge-service",
        ));
    }

    let mut bridge_config = BridgeConfig::new(&args.agentd_api)?;
    if let Some(token) = config.auth_config().api_token {
        bridge_config = bridge_config.with_operator_token(token);
    }
    Ok(MatrixClientBridgeServiceConfig {
        once: MatrixClientBridgeOnceConfig {
            bridge_config,
            state_path: args.state.clone(),
            transport_config: matrix_client_transport_config(args)?,
            puppet_accounts: matrix_client_service_puppet_account_config(args)?,
        },
        iterations: args.iterations,
    })
}

/// Validate Matrix client bridge service configuration and homeserver reachability.
pub fn run_matrix_client_bridge_preflight(
    config: &DaemonConfig,
    args: &MatrixClientBridgeServiceArgs,
) -> Result<MatrixClientBridgePreflightReport, BridgeError> {
    let service_config = matrix_client_bridge_service_config(config, args)?;
    let homeserver_url = required_cli_value(
        args.matrix_homeserver_url.as_ref(),
        "--matrix-homeserver-url",
    )?;
    let probe = MatrixHomeserverPreflightProbe::new(&homeserver_url)?;
    let versions = probe.versions()?;
    let whoami_user_id = match clean_cli_value(args.matrix_access_token.as_ref()) {
        Some(access_token) => Some(probe.whoami(&access_token)?),
        None => None,
    };

    Ok(MatrixClientBridgePreflightReport {
        iterations: service_config.iterations,
        puppet_accounts_configured: service_config.once.puppet_accounts.is_some(),
        homeserver: MatrixHomeserverPreflightReport {
            homeserver_url: probe.homeserver_url,
            versions,
            whoami_user_id,
        },
    })
}

/// Run a bounded SDK-facing Matrix bridge service with an injected client.
pub fn run_matrix_client_bridge_service<C>(
    config: &MatrixClientBridgeServiceConfig,
    client: &mut C,
) -> Result<MatrixClientBridgeServiceReport, BridgeError>
where
    C: MatrixClientPort,
{
    if config.iterations == 0 {
        return Err(BridgeError::invalid_config(
            "Matrix client bridge service iterations must be greater than 0",
        ));
    }

    let mut iterations = Vec::with_capacity(config.iterations);
    for _ in 0..config.iterations {
        iterations.push(agentd_matrix::run_matrix_client_bridge_once(
            &config.once,
            &mut *client,
        )?);
    }
    let next_from_seq = iterations.last().map_or(0, |report| report.next_from_seq);
    let bot_command_replies_sent = iterations
        .iter()
        .map(|report| report.run.bot_command_replies_sent)
        .sum();
    Ok(MatrixClientBridgeServiceReport {
        iterations,
        next_from_seq,
        bot_command_replies_sent,
    })
}

/// Run the real SDK-backed Matrix bridge service when the SDK feature is enabled.
#[cfg(feature = "matrix-sdk-adapter")]
pub fn run_matrix_sdk_bridge_service(
    config: &DaemonConfig,
    args: &MatrixClientBridgeServiceArgs,
) -> Result<MatrixClientBridgeServiceReport, BridgeError> {
    let service_config = matrix_client_bridge_service_config(config, args)?;
    let sdk_config = matrix_sdk_client_config(args)?;
    let mut client = SdkMatrixClient::build(sdk_config)?;
    run_matrix_client_bridge_service(&service_config, &mut client)
}

/// Default builds keep the Matrix SDK dependency disabled.
#[cfg(not(feature = "matrix-sdk-adapter"))]
pub fn run_matrix_sdk_bridge_service(
    _config: &DaemonConfig,
    _args: &MatrixClientBridgeServiceArgs,
) -> Result<MatrixClientBridgeServiceReport, BridgeError> {
    Err(BridgeError::invalid_config(
        "agentd matrix-client-bridge-service requires the agentd-bin matrix-sdk-adapter feature",
    ))
}

fn matrix_client_transport_config(
    args: &MatrixClientBridgeServiceArgs,
) -> Result<MatrixClientTransportConfig, BridgeError> {
    Ok(MatrixClientTransportConfig {
        bot_user_id: clean_cli_value(args.matrix_bot_user_id.as_ref()),
        agent_user_prefix: clean_string(&args.matrix_agent_prefix)
            .unwrap_or_else(|| "ac_".to_owned()),
        matrix_server_name: clean_cli_value(args.matrix_server_name.as_ref()),
        known_agent_names: clean_cli_values(&args.matrix_agents),
        skip_agent_names: clean_cli_values(&args.matrix_skip_agents),
        trust_mode: matrix_trust_mode(&args.matrix_trust_mode)?,
        trusted_inviter_mxids: clean_cli_values(&args.matrix_trusted_inviters),
        ignored_sender_mxids: clean_cli_values(&args.matrix_ignored_senders),
        bot_command_acl: MatrixBotCommandAcl {
            operator_mxids: clean_cli_values(&args.matrix_operator_mxids),
            admin_mxids: clean_cli_values(&args.matrix_admin_mxids),
        },
    })
}

fn matrix_trust_mode(value: &str) -> Result<MatrixTrustMode, BridgeError> {
    match value.trim().to_ascii_lowercase().as_str() {
        "audit" | "" => Ok(MatrixTrustMode::Audit),
        "enforce" => Ok(MatrixTrustMode::Enforce),
        other => Err(BridgeError::invalid_config(format!(
            "unsupported --matrix-trust-mode {other:?}; expected audit or enforce"
        ))),
    }
}

#[cfg(feature = "matrix-sdk-adapter")]
fn matrix_sdk_client_config(
    args: &MatrixClientBridgeServiceArgs,
) -> Result<SdkMatrixClientConfig, BridgeError> {
    let homeserver_url = required_cli_value(
        args.matrix_homeserver_url.as_ref(),
        "--matrix-homeserver-url",
    )?;
    let mut config = SdkMatrixClientConfig::new(homeserver_url)
        .with_sync_timeout_ms(args.matrix_sync_timeout_ms);
    if let Some(path) = &args.matrix_sdk_store {
        config = config.with_sqlite_store_path(path);
    }

    let has_password_login = clean_cli_value(args.matrix_username.as_ref()).is_some()
        || clean_cli_value(args.matrix_password.as_ref()).is_some();
    if has_password_login {
        let username = required_cli_value(args.matrix_username.as_ref(), "--matrix-username")?;
        let password = required_cli_value(args.matrix_password.as_ref(), "--matrix-password")?;
        config = config.with_password_login(username, password);
    }

    let has_token_restore = clean_cli_value(args.matrix_user_id.as_ref()).is_some()
        || clean_cli_value(args.matrix_access_token.as_ref()).is_some()
        || clean_cli_value(args.matrix_device_id.as_ref()).is_some();
    if has_token_restore {
        let user_id = required_cli_value(args.matrix_user_id.as_ref(), "--matrix-user-id")?;
        let access_token =
            required_cli_value(args.matrix_access_token.as_ref(), "--matrix-access-token")?;
        config = config.with_access_token(user_id, access_token);
        if let Some(device_id) = clean_cli_value(args.matrix_device_id.as_ref()) {
            config = config.with_device_id(device_id);
        }
    }

    config.validate()?;
    Ok(config)
}

fn matrix_bridge_once_puppet_account_config(
    args: &MatrixBridgeOnceArgs,
) -> Result<Option<BridgeOncePuppetAccountConfig>, BridgeError> {
    matrix_puppet_account_config(MatrixPuppetAccountConfigInput {
        homeserver_url: args.matrix_homeserver_url.as_ref(),
        server_name: args.matrix_server_name.as_ref(),
        agent_prefix: &args.matrix_agent_prefix,
        matrix_agents: &args.matrix_agents,
        matrix_skip_agents: &args.matrix_skip_agents,
        token_state_path: args.matrix_puppet_state.as_ref(),
        password_secret: args.matrix_agent_password_secret.as_ref(),
        password_template: args.matrix_agent_password_template.as_ref(),
        allow_legacy_password: args.matrix_allow_legacy_agent_password,
        registration_token: args.matrix_registration_token.as_ref(),
        agent_names_trigger_puppet: true,
    })
}

fn matrix_client_service_puppet_account_config(
    args: &MatrixClientBridgeServiceArgs,
) -> Result<Option<BridgeOncePuppetAccountConfig>, BridgeError> {
    matrix_puppet_account_config(MatrixPuppetAccountConfigInput {
        homeserver_url: args.matrix_homeserver_url.as_ref(),
        server_name: args.matrix_server_name.as_ref(),
        agent_prefix: &args.matrix_agent_prefix,
        matrix_agents: &args.matrix_agents,
        matrix_skip_agents: &args.matrix_skip_agents,
        token_state_path: args.matrix_puppet_state.as_ref(),
        password_secret: args.matrix_agent_password_secret.as_ref(),
        password_template: args.matrix_agent_password_template.as_ref(),
        allow_legacy_password: args.matrix_allow_legacy_agent_password,
        registration_token: args.matrix_registration_token.as_ref(),
        agent_names_trigger_puppet: false,
    })
}

#[derive(Clone, Copy)]
struct MatrixPuppetAccountConfigInput<'a> {
    homeserver_url: Option<&'a String>,
    server_name: Option<&'a String>,
    agent_prefix: &'a str,
    matrix_agents: &'a [String],
    matrix_skip_agents: &'a [String],
    token_state_path: Option<&'a std::path::PathBuf>,
    password_secret: Option<&'a String>,
    password_template: Option<&'a String>,
    allow_legacy_password: bool,
    registration_token: Option<&'a String>,
    agent_names_trigger_puppet: bool,
}

fn matrix_puppet_account_config(
    input: MatrixPuppetAccountConfigInput<'_>,
) -> Result<Option<BridgeOncePuppetAccountConfig>, BridgeError> {
    if !has_matrix_puppet_account_config(&input) {
        return Ok(None);
    }

    let homeserver_url = required_cli_value(input.homeserver_url, "--matrix-homeserver-url")?;
    let server_name = required_cli_value(input.server_name, "--matrix-server-name")?;
    let token_state_path = input.token_state_path.cloned().ok_or_else(|| {
        BridgeError::invalid_config(
            "--matrix-puppet-state is required when Matrix puppet account provisioning is configured",
        )
    })?;
    if input
        .matrix_agents
        .iter()
        .all(|agent_name| agent_name.trim().is_empty())
    {
        return Err(BridgeError::invalid_config(
            "at least one --matrix-agent is required when Matrix puppet account provisioning is configured",
        ));
    }

    let directory = MatrixPuppetDirectory::new(
        &server_name,
        input.agent_prefix,
        input.matrix_agents,
        input.matrix_skip_agents,
    )?;
    if directory.accounts().is_empty() {
        return Err(BridgeError::invalid_config(
            "at least one non-skipped --matrix-agent is required when Matrix puppet account provisioning is configured",
        ));
    }

    let provisioning_config = MatrixPuppetProvisioningConfig {
        password_secret: clean_cli_value(input.password_secret),
        legacy_password_template: clean_cli_value(input.password_template),
        allow_legacy_password: input.allow_legacy_password,
        registration_token: clean_cli_value(input.registration_token),
    };
    let mut http_account_config = MatrixPuppetHttpAccountConfig::new(homeserver_url)?;
    if let Some(registration_token) = provisioning_config.registration_token.clone() {
        http_account_config = http_account_config.with_registration_token(registration_token);
    }

    Ok(Some(BridgeOncePuppetAccountConfig {
        directory,
        provisioning_config,
        http_account_config,
        token_state_path,
    }))
}

fn has_matrix_puppet_account_config(input: &MatrixPuppetAccountConfigInput<'_>) -> bool {
    let account_specific = input.token_state_path.is_some()
        || clean_cli_value(input.password_secret).is_some()
        || clean_cli_value(input.password_template).is_some()
        || input.allow_legacy_password
        || clean_cli_value(input.registration_token).is_some();
    if input.agent_names_trigger_puppet {
        account_specific
            || clean_cli_value(input.homeserver_url).is_some()
            || clean_cli_value(input.server_name).is_some()
            || !input.matrix_agents.is_empty()
            || !input.matrix_skip_agents.is_empty()
    } else {
        account_specific
    }
}

fn required_cli_value(value: Option<&String>, name: &str) -> Result<String, BridgeError> {
    clean_cli_value(value).ok_or_else(|| {
        BridgeError::invalid_config(format!("{name} is required for this Matrix bridge path"))
    })
}

fn clean_cli_value(value: Option<&String>) -> Option<String> {
    value.and_then(|value| clean_string(value))
}

fn clean_string(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn clean_cli_values(values: &[String]) -> Vec<String> {
    values
        .iter()
        .filter_map(|value| clean_string(value))
        .collect()
}

#[derive(Debug, Clone)]
struct MatrixHomeserverPreflightProbe {
    homeserver_url: String,
    endpoint: MatrixPreflightHttpEndpoint,
}

impl MatrixHomeserverPreflightProbe {
    fn new(homeserver_url: &str) -> Result<Self, BridgeError> {
        let homeserver_url = homeserver_url.trim().trim_end_matches('/').to_owned();
        if homeserver_url.is_empty() {
            return Err(BridgeError::invalid_config(
                "--matrix-homeserver-url is required for matrix-client-bridge-preflight",
            ));
        }
        Ok(Self {
            endpoint: MatrixPreflightHttpEndpoint::parse(&homeserver_url)?,
            homeserver_url,
        })
    }

    fn versions(&self) -> Result<Vec<String>, BridgeError> {
        let value = self.request_json("/_matrix/client/versions", None)?;
        let versions = value
            .get("versions")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                BridgeError::transport(
                    "Matrix versions response missing string array field versions",
                )
            })?;
        let mut decoded = Vec::with_capacity(versions.len());
        for version in versions {
            let version = version.as_str().ok_or_else(|| {
                BridgeError::transport("Matrix versions response contains a non-string version")
            })?;
            decoded.push(version.to_owned());
        }
        Ok(decoded)
    }

    fn whoami(&self, access_token: &str) -> Result<String, BridgeError> {
        let value = self.request_json("/_matrix/client/v3/account/whoami", Some(access_token))?;
        required_json_string(&value, "user_id", "Matrix whoami response")
    }

    fn request_json(&self, path: &str, bearer_token: Option<&str>) -> Result<Value, BridgeError> {
        let request_path = self.endpoint.path(path);
        let response = self.http_get(&request_path, bearer_token)?;
        if !(200..300).contains(&response.status) {
            return Err(BridgeError::transport(format!(
                "GET {request_path} returned status {}: {}",
                response.status,
                String::from_utf8_lossy(&response.body)
            )));
        }
        serde_json::from_slice(&response.body).map_err(|err| {
            BridgeError::transport(format!("decode JSON from GET {request_path}: {err}"))
        })
    }

    fn http_get(
        &self,
        request_path: &str,
        bearer_token: Option<&str>,
    ) -> Result<MatrixPreflightHttpResponse, BridgeError> {
        let address = self.endpoint.address();
        let mut stream = TcpStream::connect(&address)
            .map_err(|err| BridgeError::transport(format!("connect {address}: {err}")))?;
        let timeout = Some(Duration::from_secs(5));
        stream
            .set_read_timeout(timeout)
            .map_err(|err| BridgeError::transport(format!("set read timeout: {err}")))?;
        stream
            .set_write_timeout(timeout)
            .map_err(|err| BridgeError::transport(format!("set write timeout: {err}")))?;

        let mut request = format!(
            "GET {request_path} HTTP/1.1\r\nHost: {}\r\nAccept: application/json\r\nConnection: close\r\n",
            self.endpoint.host_header
        );
        if let Some(token) = bearer_token {
            let _ = write!(request, "Authorization: Bearer {token}\r\n");
        }
        request.push_str("\r\n");

        stream
            .write_all(request.as_bytes())
            .map_err(|err| BridgeError::transport(format!("write HTTP request: {err}")))?;
        let mut response = Vec::new();
        stream
            .read_to_end(&mut response)
            .map_err(|err| BridgeError::transport(format!("read HTTP response: {err}")))?;
        MatrixPreflightHttpResponse::parse(&response)
    }
}

#[derive(Debug, Clone)]
struct MatrixPreflightHttpEndpoint {
    host: String,
    port: u16,
    host_header: String,
    base_path: String,
}

impl MatrixPreflightHttpEndpoint {
    fn parse(url: &str) -> Result<Self, BridgeError> {
        let rest = url.strip_prefix("http://").ok_or_else(|| {
            BridgeError::invalid_config(
                "matrix_homeserver_url must use http:// for the standard-library HTTP adapter",
            )
        })?;
        let (host_port, path) = rest
            .split_once('/')
            .map_or((rest, ""), |(host, path)| (host, path));
        if host_port.is_empty() {
            return Err(BridgeError::invalid_config(
                "matrix_homeserver_url host is required",
            ));
        }
        let (host, port) = match host_port.rsplit_once(':') {
            Some((host, port)) if !host.is_empty() => {
                let port = port.parse::<u16>().map_err(|err| {
                    BridgeError::invalid_config(format!(
                        "matrix_homeserver_url port is invalid: {err}"
                    ))
                })?;
                (host.to_owned(), port)
            }
            _ => (host_port.to_owned(), 80),
        };
        let path = path.trim_matches('/');
        let base_path = if path.is_empty() {
            String::new()
        } else {
            format!("/{path}")
        };
        Ok(Self {
            host,
            port,
            host_header: host_port.to_owned(),
            base_path,
        })
    }

    fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    fn path(&self, path: &str) -> String {
        format!("{}{}", self.base_path, path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MatrixPreflightHttpResponse {
    status: u16,
    body: Vec<u8>,
}

impl MatrixPreflightHttpResponse {
    fn parse(bytes: &[u8]) -> Result<Self, BridgeError> {
        let header_end = bytes
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .ok_or_else(|| BridgeError::transport("HTTP response missing header terminator"))?;
        let headers = std::str::from_utf8(&bytes[..header_end])
            .map_err(|err| BridgeError::transport(format!("decode HTTP headers: {err}")))?;
        let status = headers
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .ok_or_else(|| BridgeError::transport("HTTP response missing status"))?
            .parse::<u16>()
            .map_err(|err| BridgeError::transport(format!("parse HTTP status: {err}")))?;
        Ok(Self {
            status,
            body: bytes[(header_end + 4)..].to_vec(),
        })
    }
}

fn required_json_string(value: &Value, field: &str, context: &str) -> Result<String, BridgeError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| BridgeError::transport(format!("{context} missing string field {field}")))
}
