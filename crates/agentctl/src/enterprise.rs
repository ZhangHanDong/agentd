//! Authenticated AD-E7 operator client.

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, HeaderValue};
use reqwest::{Client, Method, Url};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use crate::cli::{
    EnterpriseCmd, EnterpriseDaemonArgs, EnterpriseExplainArgs, EnterpriseLegalHoldReleaseArgs,
    EnterpriseMutationFileArgs,
};

const EXIT_INVALID: u8 = 2;
const EXIT_DAEMON: u8 = 3;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[must_use]
pub fn run(command: &EnterpriseCmd) -> ExitCode {
    let result = run_async(execute(command));
    match result {
        Ok(value) => match serde_json::to_string_pretty(&value) {
            Ok(output) => {
                println!("{output}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::from(EXIT_DAEMON)
            }
        },
        Err(EnterpriseCliError::Invalid(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(EXIT_INVALID)
        }
        Err(EnterpriseCliError::Daemon(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(EXIT_DAEMON)
        }
    }
}

async fn execute(command: &EnterpriseCmd) -> Result<Value, EnterpriseCliError> {
    match command {
        EnterpriseCmd::Status(daemon) => request(daemon, Method::GET, "/api/enterprise/status", None).await,
        EnterpriseCmd::Explain(args) => explain(args).await,
        EnterpriseCmd::Rollout(args) => mutate(args, "declare-rollout").await,
        EnterpriseCmd::RolloutObserve(args) => mutate(args, "observe-rollout").await,
        EnterpriseCmd::ZonePolicy(args) => mutate(args, "upsert-zone-pool").await,
        EnterpriseCmd::Capacity(args) => mutate(args, "recommend-capacity").await,
        EnterpriseCmd::ReplicationPlan(args) => mutate(args, "create-replication-plan").await,
        EnterpriseCmd::ReplicaAck(args) => mutate(args, "acknowledge-replica").await,
        EnterpriseCmd::TenantKey(args) => mutate(args, "register-tenant-key").await,
        EnterpriseCmd::Retention(args) => mutate(args, "set-retention-policy").await,
        EnterpriseCmd::LegalHold(args) => mutate(args, "place-legal-hold").await,
        EnterpriseCmd::LegalHoldRelease(args) => release_legal_hold(args).await,
        EnterpriseCmd::DrCheckpoint(args) => mutate(args, "record-dr-checkpoint").await,
        EnterpriseCmd::DrDrill(args) => mutate(args, "record-dr-drill").await,
        EnterpriseCmd::LoadModel(args) => mutate(args, "register-load-model").await,
        EnterpriseCmd::ServiceLevel(args) => mutate(args, "record-service-level").await,
    }
}

async fn explain(args: &EnterpriseExplainArgs) -> Result<Value, EnterpriseCliError> {
    let task_id = args.execution_task_id.trim();
    if !task_id.starts_with("tr_")
        || task_id.len() > 128
        || !task_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(EnterpriseCliError::Invalid(
            "execution task id must be a bounded tr_ identifier".to_string(),
        ));
    }
    request(
        &args.daemon,
        Method::GET,
        &format!("/api/enterprise/tasks/{task_id}/explain"),
        None,
    )
    .await
}

async fn mutate(
    args: &EnterpriseMutationFileArgs,
    operation: &str,
) -> Result<Value, EnterpriseCliError> {
    let body = read_json(&args.file)?;
    request(
        &args.daemon,
        Method::POST,
        &format!("/api/enterprise/mutations/{operation}"),
        Some(body),
    )
    .await
}

async fn release_legal_hold(
    args: &EnterpriseLegalHoldReleaseArgs,
) -> Result<Value, EnterpriseCliError> {
    let hold_id = args.legal_hold_id.trim();
    if !hold_id.starts_with("lh_") || hold_id.len() > 128 || args.released_at < 0 {
        return Err(EnterpriseCliError::Invalid(
            "legal hold id or release time is invalid".to_string(),
        ));
    }
    request(
        &args.daemon,
        Method::POST,
        "/api/enterprise/mutations/release-legal-hold",
        Some(json!({
            "legal_hold_id": hold_id,
            "released_at": args.released_at,
        })),
    )
    .await
}

async fn request(
    daemon: &EnterpriseDaemonArgs,
    method: Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value, EnterpriseCliError> {
    let mut base = Url::parse(daemon.daemon_url.trim())
        .map_err(|error| EnterpriseCliError::Invalid(format!("invalid daemon URL: {error}")))?;
    validate_url(&base, daemon.allow_loopback_http)?;
    base.set_path(path);
    base.set_query(None);

    let token = Zeroizing::new(
        daemon
            .api_token
            .clone()
            .or_else(|| std::env::var("AGENTD_API_TOKEN").ok())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty() && value.len() <= 16 * 1024)
            .ok_or_else(|| {
                EnterpriseCliError::Invalid(
                    "enterprise operator bearer token is required".to_string(),
                )
            })?,
    );
    if token.chars().any(|character| matches!(character, '\r' | '\n')) {
        return Err(EnterpriseCliError::Invalid(
            "enterprise operator bearer token is invalid".to_string(),
        ));
    }
    let mut authorization = HeaderValue::from_str(&format!("Bearer {}", token.as_str()))
        .map_err(|error| EnterpriseCliError::Invalid(error.to_string()))?;
    authorization.set_sensitive(true);

    let client = Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| EnterpriseCliError::Daemon(error.to_string()))?;
    let mut builder = client.request(method, base).header(AUTHORIZATION, authorization);
    if let Some(body) = body {
        builder = builder.json(&body);
    }
    let mut response = builder
        .send()
        .await
        .map_err(|error| EnterpriseCliError::Daemon(error.to_string()))?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| EnterpriseCliError::Daemon(error.to_string()))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(EnterpriseCliError::Daemon(
                "enterprise daemon response exceeds 2 MiB".to_string(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        EnterpriseCliError::Daemon(format!("enterprise daemon returned invalid JSON: {error}"))
    })?;
    if status.is_success() {
        Ok(value)
    } else {
        Err(EnterpriseCliError::Daemon(format!(
            "enterprise daemon returned HTTP {}: {}",
            status.as_u16(),
            value
        )))
    }
}

fn validate_url(url: &Url, allow_loopback_http: bool) -> Result<(), EnterpriseCliError> {
    if url.username() != "" || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err(EnterpriseCliError::Invalid(
            "daemon URL must not contain credentials, query, or fragment".to_string(),
        ));
    }
    if url.scheme() == "https" {
        return Ok(());
    }
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "::1" | "localhost"));
    if url.scheme() == "http" && allow_loopback_http && loopback {
        return Ok(());
    }
    Err(EnterpriseCliError::Invalid(
        "enterprise daemon URL must use HTTPS; loopback HTTP requires --allow-loopback-http"
            .to_string(),
    ))
}

fn read_json(path: &Path) -> Result<Value, EnterpriseCliError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        EnterpriseCliError::Invalid(format!("cannot inspect {}: {error}", path.display()))
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_FILE_BYTES {
        return Err(EnterpriseCliError::Invalid(
            "enterprise mutation file must be a non-empty regular JSON file <=2 MiB".to_string(),
        ));
    }
    let bytes = std::fs::read(path).map_err(|error| {
        EnterpriseCliError::Invalid(format!("cannot read {}: {error}", path.display()))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        EnterpriseCliError::Invalid(format!("invalid JSON in {}: {error}", path.display()))
    })
}

fn run_async<F, T>(future: F) -> Result<T, EnterpriseCliError>
where
    F: std::future::Future<Output = Result<T, EnterpriseCliError>>,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| EnterpriseCliError::Daemon(error.to_string()))?
        .block_on(future)
}

enum EnterpriseCliError {
    Invalid(String),
    Daemon(String),
}
