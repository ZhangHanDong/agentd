//! Outbound-only enterprise worker pull loop and executor adapter.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use agentd_core::ports::{FleetAssignment, WorkerAvailability};
use agentd_core::types::{
    DataClassification, TaskLeaseGrant, WorkerId, WorkerIncarnationId, WorkerStatus,
};
use reqwest::{Certificate, Client, Identity, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use zeroize::Zeroizing;

use crate::cli::EnterpriseWorkerArgs;

const PROTOCOL_VERSION: u32 = 1;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_EXECUTOR_OUTPUT_BYTES: usize = 2 * 1024 * 1024;
const LEASE_SECONDS: i64 = 60;
const MAX_IDENTITY_PEM_BYTES: u64 = 256 * 1024;
const MAX_CA_PEM_BYTES: u64 = 128 * 1024;
const MAX_IDENTITY_CONFIG_BYTES: u64 = 64 * 1024;
const MAX_STALE_WORKSPACES: usize = 1_024;

#[derive(Debug, Error)]
pub enum EnterpriseWorkerError {
    #[error("invalid enterprise worker configuration: {0}")]
    Invalid(String),
    #[error("enterprise worker transport failed: {0}")]
    Transport(String),
    #[error("enterprise worker executor failed: {0}")]
    Executor(String),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "status", rename_all = "snake_case")]
enum ExecutorResult {
    Completed {
        outcome_sha256: String,
    },
    Failed {
        failure_code: String,
        retryable: bool,
    },
}

#[derive(Serialize)]
struct ExecutorRequest<'a> {
    protocol_version: u32,
    assignment: &'a FleetAssignment,
    executor_work_dir: &'a Path,
    admission: ExecutorAdmission,
}

#[derive(Serialize)]
struct ExecutorAdmission {
    artifact_acknowledgement_url: String,
    side_effect_admission_url: String,
    transport: &'static str,
    client_identity_pem: Option<PathBuf>,
    server_ca_pem: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerIdentityFile {
    worker_id: String,
    worker_incarnation_id: String,
    client_identity_pem: Option<PathBuf>,
    server_ca_pem: Option<PathBuf>,
}

struct WorkerConfig {
    worker_id: WorkerId,
    worker_incarnation_id: WorkerIncarnationId,
    region: String,
    zone: String,
    resource_class: String,
    image_digest: String,
    image_signature_verified: bool,
    executor: PathBuf,
    executor_work_root: PathBuf,
    heartbeat_interval: Duration,
    pull_interval: Duration,
    renew_interval: Duration,
    capabilities: BTreeSet<String>,
    data_classifications: BTreeSet<DataClassification>,
    egress_profile_ids: BTreeSet<String>,
    tenant_cache_namespaces: BTreeSet<String>,
    dedicated_pool: bool,
    client_identity_pem: Option<PathBuf>,
    server_ca_pem: Option<PathBuf>,
}

struct ExecutorWorkspace {
    path: PathBuf,
}

#[derive(Debug, Default)]
struct ExecutorProcessGroup {
    #[cfg(unix)]
    pid: Option<rustix::process::Pid>,
}

impl ExecutorProcessGroup {
    fn from_child(child: &tokio::process::Child) -> Result<Self, EnterpriseWorkerError> {
        #[cfg(unix)]
        {
            let raw_pid = child.id().ok_or_else(|| {
                EnterpriseWorkerError::Executor(
                    "executor process id is unavailable after spawn".to_string(),
                )
            })?;
            let raw_pid = i32::try_from(raw_pid).map_err(|_| {
                EnterpriseWorkerError::Executor(
                    "executor process id exceeds the platform range".to_string(),
                )
            })?;
            let pid = rustix::process::Pid::from_raw(raw_pid).ok_or_else(|| {
                EnterpriseWorkerError::Executor("executor process id is invalid".to_string())
            })?;
            Ok(Self { pid: Some(pid) })
        }
        #[cfg(not(unix))]
        {
            let _ = child;
            Ok(Self {})
        }
    }

    fn terminate(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid.take() {
            if let Err(error) =
                rustix::process::kill_process_group(pid, rustix::process::Signal::KILL)
            {
                if error != rustix::io::Errno::SRCH {
                    tracing::warn!(
                        error_code = "executor_process_group_cleanup_failed",
                        "enterprise executor process group cleanup failed"
                    );
                }
            }
        }
    }
}

impl Drop for ExecutorProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}

impl ExecutorWorkspace {
    fn create(root: &Path, assignment: &FleetAssignment) -> Result<Self, EnterpriseWorkerError> {
        clear_executor_work_root(root)?;
        let claim = assignment.lease.claim();
        let path = root.join(format!(
            "{}__{}__{}",
            claim.execution_task_id, claim.lease_id, claim.fencing_token
        ));
        remove_workspace_entry(&path)?;
        std::fs::create_dir(&path).map_err(|error| {
            EnterpriseWorkerError::Executor(format!(
                "cannot create isolated executor workspace: {error}"
            ))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            if let Err(error) =
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            {
                let _ = remove_workspace_entry(&path);
                return Err(EnterpriseWorkerError::Executor(format!(
                    "cannot restrict executor workspace permissions: {error}"
                )));
            }
        }
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ExecutorWorkspace {
    fn drop(&mut self) {
        if remove_workspace_entry(&self.path).is_err() {
            tracing::warn!(
                error_code = "executor_workspace_cleanup_failed",
                "enterprise executor workspace cleanup failed"
            );
        }
    }
}

impl WorkerConfig {
    fn from_args(args: &EnterpriseWorkerArgs) -> Result<Self, EnterpriseWorkerError> {
        let identity = load_identity(args)?;
        validate_id(&identity.worker_id, "wk_", "worker id")?;
        validate_id(
            &identity.worker_incarnation_id,
            "wi_",
            "worker incarnation id",
        )?;
        if args.total_slots != 1 {
            return Err(EnterpriseWorkerError::Invalid(
                "the native enterprise worker currently requires --total-slots 1".to_string(),
            ));
        }
        for (field, value) in [
            ("region", args.region.as_str()),
            ("zone", args.zone.as_str()),
            ("resource class", args.resource_class.as_str()),
        ] {
            if value != value.trim()
                || value.is_empty()
                || value.len() > 256
                || value.chars().any(char::is_control)
            {
                return Err(EnterpriseWorkerError::Invalid(format!(
                    "{field} must contain 1..=256 bytes"
                )));
            }
        }
        if !valid_image_digest(&args.image_digest) {
            return Err(EnterpriseWorkerError::Invalid(
                "worker image digest must be sha256:<64 lowercase hex>".to_string(),
            ));
        }
        if !args.image_signature_verified {
            return Err(EnterpriseWorkerError::Invalid(
                "enterprise worker requires a signature-verified image".to_string(),
            ));
        }
        let executor = std::fs::canonicalize(&args.executor).map_err(|error| {
            EnterpriseWorkerError::Invalid(format!("executor is unavailable: {error}"))
        })?;
        if !executor.is_file() {
            return Err(EnterpriseWorkerError::Invalid(
                "executor must be a regular file".to_string(),
            ));
        }
        let executor_work_root =
            std::fs::canonicalize(&args.executor_work_root).map_err(|error| {
                EnterpriseWorkerError::Invalid(format!(
                    "executor work root is unavailable: {error}"
                ))
            })?;
        if !executor_work_root.is_dir() {
            return Err(EnterpriseWorkerError::Invalid(
                "executor work root must be a directory".to_string(),
            ));
        }
        if args.heartbeat_seconds == 0
            || args.pull_seconds == 0
            || args.renew_seconds == 0
            || args.heartbeat_seconds > 60
            || args.pull_seconds > 60
            || args.renew_seconds >= u64::try_from(LEASE_SECONDS).unwrap_or(u64::MAX)
        {
            return Err(EnterpriseWorkerError::Invalid(
                "heartbeat/pull must be 1..=60s and renew must be shorter than the lease"
                    .to_string(),
            ));
        }
        let data_classifications = args
            .data_classifications
            .iter()
            .map(|value| parse_classification(value))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if data_classifications.is_empty() {
            return Err(EnterpriseWorkerError::Invalid(
                "at least one data classification is required".to_string(),
            ));
        }
        Ok(Self {
            worker_id: WorkerId::from_string(identity.worker_id.trim()),
            worker_incarnation_id: WorkerIncarnationId::from_string(
                identity.worker_incarnation_id.trim(),
            ),
            region: args.region.trim().to_string(),
            zone: args.zone.trim().to_string(),
            resource_class: args.resource_class.trim().to_string(),
            image_digest: args.image_digest.clone(),
            image_signature_verified: args.image_signature_verified,
            executor,
            executor_work_root,
            heartbeat_interval: Duration::from_secs(args.heartbeat_seconds),
            pull_interval: Duration::from_secs(args.pull_seconds),
            renew_interval: Duration::from_secs(args.renew_seconds),
            capabilities: bounded_set(&args.capabilities, "capability")?,
            data_classifications,
            egress_profile_ids: bounded_set(&args.egress_profile_ids, "egress profile")?,
            tenant_cache_namespaces: bounded_set(
                &args.tenant_cache_namespaces,
                "tenant cache namespace",
            )?,
            dedicated_pool: args.dedicated_pool,
            client_identity_pem: identity.client_identity_pem,
            server_ca_pem: identity.server_ca_pem,
        })
    }

    fn availability(&self, sequence: u64, status: WorkerStatus) -> WorkerAvailability {
        let available_slots = u32::from(status == WorkerStatus::Online);
        WorkerAvailability {
            worker_id: self.worker_id.clone(),
            worker_incarnation_id: self.worker_incarnation_id.clone(),
            heartbeat_sequence: sequence,
            worker_status: status,
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_min: PROTOCOL_VERSION,
            protocol_max: PROTOCOL_VERSION,
            region: self.region.clone(),
            zone: self.zone.clone(),
            resource_class: self.resource_class.clone(),
            capabilities: self.capabilities.clone(),
            total_slots: 1,
            available_slots,
            data_classifications: self.data_classifications.clone(),
            image_digest: self.image_digest.clone(),
            image_signature_verified: self.image_signature_verified,
            dedicated_pool: self.dedicated_pool,
            egress_profile_ids: self.egress_profile_ids.clone(),
            tenant_cache_namespaces: self.tenant_cache_namespaces.clone(),
        }
    }
}

#[derive(Clone)]
struct FleetHttpClient {
    client: Client,
    base_url: Url,
}

impl FleetHttpClient {
    fn new(
        args: &EnterpriseWorkerArgs,
        config: &WorkerConfig,
    ) -> Result<Self, EnterpriseWorkerError> {
        let mut base_url = Url::parse(args.control_plane_url.trim())
            .map_err(|error| EnterpriseWorkerError::Invalid(error.to_string()))?;
        if !base_url.username().is_empty()
            || base_url.password().is_some()
            || base_url.query().is_some()
            || base_url.fragment().is_some()
        {
            return Err(EnterpriseWorkerError::Invalid(
                "control-plane URL must not contain credentials, query, or fragment".to_string(),
            ));
        }
        let loopback = matches!(base_url.host_str(), Some("127.0.0.1" | "::1" | "localhost"));
        match base_url.scheme() {
            "https" if !args.mesh_mtls => {}
            "http" if args.mesh_mtls && loopback => {}
            _ => {
                return Err(EnterpriseWorkerError::Invalid(
                    "control-plane URL requires direct HTTPS or loopback HTTP with --mesh-mtls"
                        .to_string(),
                ));
            }
        }
        if !base_url.path().ends_with('/') {
            base_url.set_path(&format!("{}/", base_url.path()));
        }
        let mut builder = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none());
        if base_url.scheme() == "https" {
            let identity_path = config.client_identity_pem.as_deref().ok_or_else(|| {
                EnterpriseWorkerError::Invalid(
                    "HTTPS worker transport requires --client-identity-pem".to_string(),
                )
            })?;
            let ca_path = config.server_ca_pem.as_deref().ok_or_else(|| {
                EnterpriseWorkerError::Invalid(
                    "HTTPS worker transport requires --server-ca-pem".to_string(),
                )
            })?;
            let identity_bytes = Zeroizing::new(read_bounded_file(
                identity_path,
                MAX_IDENTITY_PEM_BYTES,
                "client identity",
            )?);
            let ca_bytes = read_bounded_file(ca_path, MAX_CA_PEM_BYTES, "server CA")?;
            let identity = Identity::from_pem(&identity_bytes).map_err(|error| {
                EnterpriseWorkerError::Invalid(format!("invalid client identity PEM: {error}"))
            })?;
            let server_ca = Certificate::from_pem(&ca_bytes).map_err(|error| {
                EnterpriseWorkerError::Invalid(format!("invalid server CA PEM: {error}"))
            })?;
            builder = builder
                .tls_built_in_root_certs(false)
                .identity(identity)
                .add_root_certificate(server_ca);
        } else if config.client_identity_pem.is_some() || config.server_ca_pem.is_some() {
            return Err(EnterpriseWorkerError::Invalid(
                "mesh-mTLS HTTP must not also configure direct TLS identity files".to_string(),
            ));
        }
        let client = builder
            .build()
            .map_err(|error| EnterpriseWorkerError::Transport(error.to_string()))?;
        Ok(Self { client, base_url })
    }

    fn endpoint(&self, path: &str) -> Result<String, EnterpriseWorkerError> {
        self.base_url
            .join(path)
            .map(|url| url.to_string())
            .map_err(|error| EnterpriseWorkerError::Invalid(error.to_string()))
    }

    async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: Value,
    ) -> Result<T, EnterpriseWorkerError> {
        let url = self
            .base_url
            .join(path)
            .map_err(|error| EnterpriseWorkerError::Invalid(error.to_string()))?;
        let mut response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|error| EnterpriseWorkerError::Transport(error.to_string()))?;
        let status = response.status();
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| EnterpriseWorkerError::Transport(error.to_string()))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(EnterpriseWorkerError::Transport(
                    "control-plane response exceeds 2 MiB".to_string(),
                ));
            }
            bytes.extend_from_slice(&chunk);
        }
        if !status.is_success() {
            let code = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
            return Err(EnterpriseWorkerError::Transport(code));
        }
        serde_json::from_slice(&bytes)
            .map_err(|error| EnterpriseWorkerError::Transport(error.to_string()))
    }
}

pub async fn run(args: &EnterpriseWorkerArgs) -> Result<(), EnterpriseWorkerError> {
    let config = WorkerConfig::from_args(args)?;
    let client = FleetHttpClient::new(args, &config)?;
    let mut heartbeat_sequence = heartbeat_sequence_seed()?;
    heartbeat(&client, &config, heartbeat_sequence, WorkerStatus::Online).await?;
    let mut heartbeat_interval = tokio::time::interval_at(
        tokio::time::Instant::now() + config.heartbeat_interval,
        config.heartbeat_interval,
    );
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut pull_interval = tokio::time::interval_at(
        tokio::time::Instant::now() + config.pull_interval,
        config.pull_interval,
    );
    pull_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                heartbeat_sequence = heartbeat_sequence.saturating_add(1);
                let _ = heartbeat(&client, &config, heartbeat_sequence, WorkerStatus::Offline).await;
                return Ok(());
            }
            _ = heartbeat_interval.tick() => {
                heartbeat_sequence = heartbeat_sequence.saturating_add(1);
                heartbeat(&client, &config, heartbeat_sequence, WorkerStatus::Online).await?;
            }
            _ = pull_interval.tick() => {
                let lease_expires_at = now_unix()?.saturating_add(LEASE_SECONDS);
                let assignment: Option<FleetAssignment> = client
                    .post(
                        "api/enterprise/fleet/pull",
                        json!({
                            "protocol_version": PROTOCOL_VERSION,
                            "heartbeat_max_age_seconds": config.heartbeat_interval.as_secs().saturating_mul(3),
                            "lease_expires_at": lease_expires_at,
                        }),
                    )
                    .await?;
                if let Some(assignment) = assignment {
                    tokio::select! {
                        result = execute_assignment(
                            &client,
                            &config,
                            assignment,
                            heartbeat_sequence,
                        ) => {
                            heartbeat_sequence = result?;
                        }
                        _ = &mut shutdown => {
                            heartbeat_sequence = heartbeat_sequence.saturating_add(1);
                            let _ = heartbeat(
                                &client,
                                &config,
                                heartbeat_sequence,
                                WorkerStatus::Offline,
                            ).await;
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

async fn heartbeat(
    client: &FleetHttpClient,
    config: &WorkerConfig,
    sequence: u64,
    status: WorkerStatus,
) -> Result<WorkerAvailability, EnterpriseWorkerError> {
    client
        .post(
            "api/enterprise/fleet/heartbeat",
            json!({ "availability": config.availability(sequence, status) }),
        )
        .await
}

async fn execute_assignment(
    client: &FleetHttpClient,
    config: &WorkerConfig,
    assignment: FleetAssignment,
    mut heartbeat_sequence: u64,
) -> Result<u64, EnterpriseWorkerError> {
    heartbeat_sequence = heartbeat_sequence.saturating_add(1);
    heartbeat(client, config, heartbeat_sequence, WorkerStatus::Draining).await?;
    let result =
        run_executor_with_renewal(client, config, &assignment, &mut heartbeat_sequence).await;
    let claim = assignment.lease.claim();
    let idempotency_suffix = format!(
        "{}:{}:{}",
        claim.execution_task_id, claim.lease_id, claim.fencing_token
    );
    match result {
        Ok(ExecutorResult::Completed { outcome_sha256 }) => {
            let _: agentd_core::ports::FleetTaskRecord = client
                .post(
                    "api/enterprise/fleet/complete",
                    json!({
                        "claim": claim,
                        "idempotency_key": format!("worker-complete:{idempotency_suffix}"),
                        "outcome_sha256": outcome_sha256,
                    }),
                )
                .await?;
        }
        Ok(ExecutorResult::Failed {
            failure_code,
            retryable,
        }) => {
            let _: agentd_core::ports::FleetTaskRecord = client
                .post(
                    "api/enterprise/fleet/fail",
                    json!({
                        "claim": claim,
                        "idempotency_key": format!("worker-fail:{idempotency_suffix}"),
                        "failure_code": failure_code,
                        "retryable": retryable,
                    }),
                )
                .await?;
        }
        Err(error) => {
            let _: agentd_core::ports::FleetTaskRecord = client
                .post(
                    "api/enterprise/fleet/fail",
                    json!({
                        "claim": claim,
                        "idempotency_key": format!("worker-fail:{idempotency_suffix}"),
                        "failure_code": "executor_failed",
                        "retryable": true,
                    }),
                )
                .await?;
            tracing::warn!(error_code = %executor_error_code(&error), "enterprise executor failed");
        }
    }
    heartbeat_sequence = heartbeat_sequence.saturating_add(1);
    heartbeat(client, config, heartbeat_sequence, WorkerStatus::Online).await?;
    Ok(heartbeat_sequence)
}

async fn run_executor_with_renewal(
    client: &FleetHttpClient,
    config: &WorkerConfig,
    assignment: &FleetAssignment,
    heartbeat_sequence: &mut u64,
) -> Result<ExecutorResult, EnterpriseWorkerError> {
    let workspace = ExecutorWorkspace::create(&config.executor_work_root, assignment)?;
    let mut command = Command::new(&config.executor);
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env(
            "AGENTD_EXECUTION_TASK_ID",
            assignment.task.execution_task_id.as_str(),
        )
        .current_dir(workspace.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command
        .spawn()
        .map_err(|error| EnterpriseWorkerError::Executor(error.to_string()))?;
    let mut process_group = ExecutorProcessGroup::from_child(&child)?;
    let input = serde_json::to_vec(&ExecutorRequest {
        protocol_version: PROTOCOL_VERSION,
        assignment,
        executor_work_dir: workspace.path(),
        admission: ExecutorAdmission {
            artifact_acknowledgement_url: client.endpoint("api/enterprise/fleet/artifacts/ack")?,
            side_effect_admission_url: client
                .endpoint("api/enterprise/fleet/side-effects/admit")?,
            transport: if config.client_identity_pem.is_some() {
                "direct_mtls"
            } else {
                "mesh_mtls"
            },
            client_identity_pem: config.client_identity_pem.clone(),
            server_ca_pem: config.server_ca_pem.clone(),
        },
    })
    .map_err(|error| EnterpriseWorkerError::Executor(error.to_string()))?;
    let mut stdin = child.stdin.take().ok_or_else(|| {
        EnterpriseWorkerError::Executor("executor stdin is unavailable".to_string())
    })?;
    stdin
        .write_all(&input)
        .await
        .map_err(|error| EnterpriseWorkerError::Executor(error.to_string()))?;
    drop(stdin);
    let stdout = child.stdout.take().ok_or_else(|| {
        EnterpriseWorkerError::Executor("executor stdout is unavailable".to_string())
    })?;
    let stderr = child.stderr.take().ok_or_else(|| {
        EnterpriseWorkerError::Executor("executor stderr is unavailable".to_string())
    })?;
    let stdout_task = tokio::spawn(read_bounded(stdout, MAX_EXECUTOR_OUTPUT_BYTES));
    let stderr_task = tokio::spawn(read_bounded(stderr, MAX_EXECUTOR_OUTPUT_BYTES));
    let mut last_renewal = tokio::time::Instant::now();
    let mut last_heartbeat = tokio::time::Instant::now();
    let status = loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if let Some(status) = child
            .try_wait()
            .map_err(|error| EnterpriseWorkerError::Executor(error.to_string()))?
        {
            break status;
        }
        if last_renewal.elapsed() >= config.renew_interval {
            let expires_at = now_unix()?.saturating_add(LEASE_SECONDS);
            let renewed: TaskLeaseGrant = client
                .post(
                    "api/enterprise/fleet/renew",
                    json!({
                        "claim": assignment.lease.claim(),
                        "snapshot_ref": assignment.task.snapshot_ref,
                        "pinned_revocation_epoch": assignment.task.policy_revocation_epoch,
                        "expires_at": expires_at,
                    }),
                )
                .await?;
            if renewed.fencing_token != assignment.lease.fencing_token {
                let _ = child.start_kill();
                return Err(EnterpriseWorkerError::Executor(
                    "lease renewal changed the fencing token".to_string(),
                ));
            }
            last_renewal = tokio::time::Instant::now();
        }
        if last_heartbeat.elapsed() >= config.heartbeat_interval {
            *heartbeat_sequence = (*heartbeat_sequence).saturating_add(1);
            heartbeat(client, config, *heartbeat_sequence, WorkerStatus::Draining).await?;
            last_heartbeat = tokio::time::Instant::now();
        }
    };
    process_group.terminate();
    let stdout = stdout_task
        .await
        .map_err(|error| EnterpriseWorkerError::Executor(error.to_string()))??;
    let stderr = stderr_task
        .await
        .map_err(|error| EnterpriseWorkerError::Executor(error.to_string()))??;
    let _stderr_sha256 = format!("{:x}", Sha256::digest(&stderr));
    if !status.success() {
        return Err(EnterpriseWorkerError::Executor(
            "executor returned a non-zero status".to_string(),
        ));
    }
    let result: ExecutorResult = serde_json::from_slice(&stdout).map_err(|error| {
        EnterpriseWorkerError::Executor(format!("invalid result JSON: {error}"))
    })?;
    match &result {
        ExecutorResult::Completed { outcome_sha256 } if !valid_sha256(outcome_sha256) => {
            Err(EnterpriseWorkerError::Executor(
                "executor outcome_sha256 must be 64 lowercase hex characters".to_string(),
            ))
        }
        ExecutorResult::Failed { failure_code, .. }
            if failure_code.trim().is_empty()
                || failure_code.len() > 128
                || failure_code.bytes().any(|byte| {
                    !(byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
                }) =>
        {
            Err(EnterpriseWorkerError::Executor(
                "executor failure_code is invalid".to_string(),
            ))
        }
        _ => Ok(result),
    }
}

fn clear_executor_work_root(root: &Path) -> Result<(), EnterpriseWorkerError> {
    let entries = std::fs::read_dir(root).map_err(|error| {
        EnterpriseWorkerError::Executor(format!("cannot inspect executor work root: {error}"))
    })?;
    for (index, entry) in entries.enumerate() {
        if index >= MAX_STALE_WORKSPACES {
            return Err(EnterpriseWorkerError::Executor(
                "executor work root contains too many stale entries".to_string(),
            ));
        }
        let entry = entry.map_err(|error| {
            EnterpriseWorkerError::Executor(format!(
                "cannot inspect stale executor workspace: {error}"
            ))
        })?;
        remove_workspace_entry(&entry.path())?;
    }
    Ok(())
}

fn remove_workspace_entry(path: &Path) -> Result<(), EnterpriseWorkerError> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(EnterpriseWorkerError::Executor(format!(
                "cannot inspect executor workspace: {error}"
            )));
        }
    };
    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    result.map_err(|error| {
        EnterpriseWorkerError::Executor(format!("cannot remove executor workspace: {error}"))
    })
}

async fn read_bounded<R: AsyncRead + Unpin>(
    mut reader: R,
    maximum_bytes: usize,
) -> Result<Vec<u8>, EnterpriseWorkerError> {
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut exceeded = false;
    loop {
        let count = reader
            .read(&mut buffer)
            .await
            .map_err(|error| EnterpriseWorkerError::Executor(error.to_string()))?;
        if count == 0 {
            break;
        }
        if retained.len().saturating_add(count) <= maximum_bytes {
            retained.extend_from_slice(&buffer[..count]);
        } else {
            exceeded = true;
        }
    }
    if exceeded {
        return Err(EnterpriseWorkerError::Executor(format!(
            "executor output exceeds {maximum_bytes} bytes"
        )));
    }
    Ok(retained)
}

fn validate_id(value: &str, prefix: &str, field: &str) -> Result<(), EnterpriseWorkerError> {
    let value = value.trim();
    if value.len() != 29
        || !value.starts_with(prefix)
        || !value
            .as_bytes()
            .get(prefix.len())
            .is_some_and(|byte| matches!(byte, b'0'..=b'7'))
        || value[prefix.len()..].bytes().any(|byte| {
            !matches!(
                byte,
                b'0'..=b'9'
                    | b'A'..=b'H'
                    | b'J'..=b'K'
                    | b'M'..=b'N'
                    | b'P'..=b'T'
                    | b'V'..=b'Z'
            )
        })
    {
        return Err(EnterpriseWorkerError::Invalid(format!(
            "{field} must be {prefix}<26-character ULID>"
        )));
    }
    Ok(())
}

fn load_identity(args: &EnterpriseWorkerArgs) -> Result<WorkerIdentityFile, EnterpriseWorkerError> {
    if let Some(path) = args.identity_config_file.as_deref() {
        if args.worker_id.is_some()
            || args.worker_incarnation_id.is_some()
            || args.client_identity_pem.is_some()
            || args.server_ca_pem.is_some()
        {
            return Err(EnterpriseWorkerError::Invalid(
                "--identity-config-file cannot be combined with direct identity arguments"
                    .to_string(),
            ));
        }
        let bytes = read_bounded_file(path, MAX_IDENTITY_CONFIG_BYTES, "identity config")?;
        return serde_json::from_slice(&bytes).map_err(|error| {
            EnterpriseWorkerError::Invalid(format!("invalid identity config JSON: {error}"))
        });
    }
    let worker_id = args.worker_id.clone().ok_or_else(|| {
        EnterpriseWorkerError::Invalid(
            "--worker-id or --identity-config-file is required".to_string(),
        )
    })?;
    let worker_incarnation_id = args.worker_incarnation_id.clone().ok_or_else(|| {
        EnterpriseWorkerError::Invalid(
            "--worker-incarnation-id or --identity-config-file is required".to_string(),
        )
    })?;
    Ok(WorkerIdentityFile {
        worker_id,
        worker_incarnation_id,
        client_identity_pem: args.client_identity_pem.clone(),
        server_ca_pem: args.server_ca_pem.clone(),
    })
}

fn read_bounded_file(
    path: &std::path::Path,
    maximum_bytes: u64,
    field: &str,
) -> Result<Vec<u8>, EnterpriseWorkerError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        EnterpriseWorkerError::Invalid(format!("cannot inspect {field} file: {error}"))
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum_bytes {
        return Err(EnterpriseWorkerError::Invalid(format!(
            "{field} must be a non-empty regular file <= {maximum_bytes} bytes"
        )));
    }
    std::fs::read(path).map_err(|error| {
        EnterpriseWorkerError::Invalid(format!("cannot read {field} file: {error}"))
    })
}

fn valid_image_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn bounded_set(values: &[String], field: &str) -> Result<BTreeSet<String>, EnterpriseWorkerError> {
    if values.len() > 128 {
        return Err(EnterpriseWorkerError::Invalid(format!(
            "too many {field} values"
        )));
    }
    values
        .iter()
        .map(|value| {
            if value != value.trim()
                || value.is_empty()
                || value.len() > 256
                || value.chars().any(char::is_control)
            {
                Err(EnterpriseWorkerError::Invalid(format!(
                    "invalid {field} value"
                )))
            } else {
                Ok(value.to_string())
            }
        })
        .collect()
}

fn parse_classification(value: &str) -> Result<DataClassification, EnterpriseWorkerError> {
    match value.trim() {
        "public" => Ok(DataClassification::Public),
        "internal" => Ok(DataClassification::Internal),
        "confidential" => Ok(DataClassification::Confidential),
        "restricted" => Ok(DataClassification::Restricted),
        _ => Err(EnterpriseWorkerError::Invalid(
            "data classification must be public|internal|confidential|restricted".to_string(),
        )),
    }
}

fn now_unix() -> Result<i64, EnterpriseWorkerError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| EnterpriseWorkerError::Transport(error.to_string()))?
        .as_secs();
    i64::try_from(seconds)
        .map_err(|_| EnterpriseWorkerError::Transport("system time overflow".to_string()))
}

fn heartbeat_sequence_seed() -> Result<u64, EnterpriseWorkerError> {
    let nanoseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| EnterpriseWorkerError::Transport(error.to_string()))?
        .as_nanos();
    let sequence = u64::try_from(nanoseconds).map_err(|_| {
        EnterpriseWorkerError::Transport("heartbeat sequence clock overflow".to_string())
    })?;
    if sequence == 0 || sequence > i64::MAX.unsigned_abs() {
        return Err(EnterpriseWorkerError::Transport(
            "heartbeat sequence is outside the durable range".to_string(),
        ));
    }
    Ok(sequence)
}

fn executor_error_code(error: &EnterpriseWorkerError) -> &'static str {
    match error {
        EnterpriseWorkerError::Invalid(_) => "executor_invalid",
        EnterpriseWorkerError::Transport(_) => "executor_transport_failure",
        EnterpriseWorkerError::Executor(_) => "executor_failure",
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = terminate.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{clear_executor_work_root, heartbeat_sequence_seed, remove_workspace_entry};

    #[test]
    fn heartbeat_sequence_seed_fits_the_durable_monotonic_range() {
        let sequence = heartbeat_sequence_seed().expect("heartbeat sequence");
        assert!(sequence > 0);
        assert!(sequence <= i64::MAX.unsigned_abs());
    }

    #[test]
    fn workspace_cleanup_removes_nested_content() {
        let root = tempfile::tempdir().expect("root");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&workspace).expect("workspace");
        std::fs::create_dir(workspace.join("nested")).expect("nested");
        std::fs::write(workspace.join("nested/output"), b"result").expect("output");

        remove_workspace_entry(&workspace).expect("cleanup");

        assert!(!workspace.exists());
    }

    #[test]
    fn work_root_cleanup_removes_stale_workspaces() {
        let root = tempfile::tempdir().expect("root");
        for name in ["old-a", "old-b"] {
            let workspace = root.path().join(name);
            std::fs::create_dir(&workspace).expect("workspace");
            std::fs::write(workspace.join("output"), b"result").expect("output");
        }

        clear_executor_work_root(root.path()).expect("cleanup");

        assert_eq!(
            std::fs::read_dir(root.path())
                .expect("root entries")
                .count(),
            0
        );
    }

    #[cfg(unix)]
    #[test]
    fn workspace_cleanup_does_not_follow_a_symlink() {
        let root = tempfile::tempdir().expect("root");
        let target = root.path().join("target");
        let workspace = root.path().join("workspace");
        std::fs::create_dir(&target).expect("target");
        std::fs::write(target.join("retained"), b"state").expect("state");
        std::os::unix::fs::symlink(&target, &workspace).expect("symlink");

        remove_workspace_entry(&workspace).expect("cleanup");

        assert!(!workspace.exists());
        assert!(target.join("retained").is_file());
    }
}
