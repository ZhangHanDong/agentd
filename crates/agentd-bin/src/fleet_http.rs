//! Authenticated HTTP transport for the outbound-only enterprise worker fleet.

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agentd_core::ports::{
    ArtifactUploadAckRequest, FleetCancelRequest, FleetCompletionReport, FleetFailureReport,
    FleetHeartbeatRequest, FleetPullRequest, FleetReapRequest, FleetRenewRequest,
    FleetSchedulerPort, FleetSideEffectRequest, FleetSubmitRequest, SecurityError,
    WorkloadIdentityPort,
};
use agentd_core::types::{
    AuthenticatedWorkload, WorkerId, WorkerIncarnationId, WorkloadIdentityRequest, WorkloadRole,
};
use agentd_surface::http::AuthConfig;
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde_json::json;
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use agentd_security::identity::RustlsWorkloadIdentityAdapter;
use agentd_store::SqliteStore;
use agentd_store::enrollment_repo::{WorkerWorkloadEnrollment, enroll_worker_workload_identity};
use agentd_store::security_repo::{WorkloadIdentityBindingCreate, revoke_workload_identity};
use agentd_store::worker_repo::{WorkerCreate, WorkerRegistration};
use thiserror::Error;

use crate::cli::DaemonConfig;
use crate::clock::SystemClock;
use crate::fleet::{
    EnterpriseFleetProviders, EnterpriseFleetService, FleetServiceError,
    build_enterprise_fleet_service,
};

const MAX_CERTIFICATE_CHAIN_HEADER_BYTES: usize = 64 * 1024;
const MAX_CERTIFICATES: usize = 8;
const MAX_CERTIFICATE_BYTES: usize = 16 * 1024;
const MAX_PROXY_AUTHORIZATION_BYTES: u64 = 16 * 1024;
const MAX_FLEET_BODY_BYTES: usize = 1024 * 1024;
const MAX_ENROLLMENT_METADATA_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum FleetHttpStartupError {
    #[error("invalid enterprise fleet HTTP configuration: {0}")]
    Invalid(String),
    #[error("enterprise fleet HTTP composition failed: {0}")]
    Composition(String),
}

#[derive(Clone)]
pub struct FleetHttpState {
    service: Arc<EnterpriseFleetService>,
    operator_auth: AuthConfig,
    proxy_authorization: Arc<Zeroizing<String>>,
    store: SqliteStore,
    enrollment_identity: Arc<RustlsWorkloadIdentityAdapter>,
}

impl std::fmt::Debug for FleetHttpState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FleetHttpState")
            .field("service", &"[CONFIGURED]")
            .field("operator_auth", &"[REDACTED]")
            .field("proxy_authorization", &"[REDACTED]")
            .finish()
    }
}

impl FleetHttpState {
    #[must_use]
    pub fn new(
        service: EnterpriseFleetService,
        operator_auth: AuthConfig,
        proxy_authorization: Zeroizing<String>,
        store: SqliteStore,
        enrollment_identity: Arc<RustlsWorkloadIdentityAdapter>,
    ) -> Self {
        Self {
            service: Arc::new(service),
            operator_auth,
            proxy_authorization: Arc::new(proxy_authorization),
            store,
            enrollment_identity,
        }
    }
}

pub fn compose(
    daemon: &DaemonConfig,
    store: &SqliteStore,
    scheduler: Arc<dyn FleetSchedulerPort>,
    operator_auth: AuthConfig,
) -> Result<FleetHttpState, FleetHttpStartupError> {
    let enterprise = &daemon.enterprise;
    if enterprise.workload_trust_root_der_files.is_empty() {
        return Err(FleetHttpStartupError::Invalid(
            "at least one workload trust root DER file is required".to_string(),
        ));
    }
    let trust_domain = enterprise
        .workload_trust_domain
        .as_deref()
        .map(str::trim)
        .filter(|value| valid_trust_domain(value))
        .ok_or_else(|| {
            FleetHttpStartupError::Invalid("workload trust domain is required".to_string())
        })?;
    let roots = enterprise
        .workload_trust_root_der_files
        .iter()
        .map(|path| read_bounded_file(path, MAX_CERTIFICATE_BYTES as u64, "trust root"))
        .collect::<Result<Vec<_>, _>>()?;
    let identity = Arc::new(
        RustlsWorkloadIdentityAdapter::new(store.pool().clone(), roots, trust_domain)
            .map_err(|error| FleetHttpStartupError::Composition(error.to_string()))?,
    );
    let proxy_path = enterprise
        .workload_proxy_authorization_file
        .as_deref()
        .ok_or_else(|| {
            FleetHttpStartupError::Invalid(
                "workload proxy authorization file is required".to_string(),
            )
        })?;
    let proxy_authorization = String::from_utf8(read_bounded_file(
        proxy_path,
        MAX_PROXY_AUTHORIZATION_BYTES,
        "workload proxy authorization",
    )?)
    .map_err(|_| {
        FleetHttpStartupError::Invalid("workload proxy authorization must be UTF-8".to_string())
    })?;
    let proxy_authorization = proxy_authorization.trim();
    if proxy_authorization.is_empty()
        || proxy_authorization
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    {
        return Err(FleetHttpStartupError::Invalid(
            "workload proxy authorization is invalid".to_string(),
        ));
    }
    let service = build_enterprise_fleet_service(EnterpriseFleetProviders::new(
        Arc::clone(&identity) as Arc<dyn WorkloadIdentityPort>,
        scheduler,
        Arc::new(SystemClock),
    ))
    .map_err(|error| FleetHttpStartupError::Composition(error.to_string()))?;
    Ok(FleetHttpState::new(
        service,
        operator_auth,
        Zeroizing::new(proxy_authorization.to_string()),
        store.clone(),
        identity,
    ))
}

fn read_bounded_file(
    path: &Path,
    maximum_bytes: u64,
    field: &str,
) -> Result<Vec<u8>, FleetHttpStartupError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        FleetHttpStartupError::Invalid(format!("{field} file {}: {error}", path.display()))
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum_bytes {
        return Err(FleetHttpStartupError::Invalid(format!(
            "{field} file must be a non-empty regular file <= {maximum_bytes} bytes"
        )));
    }
    std::fs::read(path).map_err(|error| {
        FleetHttpStartupError::Invalid(format!("cannot read {field} file: {error}"))
    })
}

pub fn router(state: FleetHttpState) -> Router {
    Router::new()
        .route("/api/enterprise/fleet/tasks", post(submit_task))
        .route("/api/enterprise/fleet/reap", post(reap))
        .route("/api/enterprise/fleet/outbox", get(outbox))
        .route("/api/enterprise/fleet/workers/enroll", post(enroll_worker))
        .route(
            "/api/enterprise/fleet/workers/identities/revoke",
            post(revoke_worker_identity),
        )
        .route("/api/enterprise/fleet/heartbeat", post(heartbeat))
        .route("/api/enterprise/fleet/pull", post(pull))
        .route("/api/enterprise/fleet/renew", post(renew))
        .route("/api/enterprise/fleet/complete", post(complete))
        .route("/api/enterprise/fleet/fail", post(fail))
        .route("/api/enterprise/fleet/cancel", post(cancel))
        .route(
            "/api/enterprise/fleet/artifacts/ack",
            post(acknowledge_artifact),
        )
        .route(
            "/api/enterprise/fleet/side-effects/admit",
            post(admit_side_effect),
        )
        .layer(DefaultBodyLimit::max(MAX_FLEET_BODY_BYTES))
        .with_state(state)
}

async fn submit_task(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(request): Json<FleetSubmitRequest>,
) -> Response {
    if !operator_authorized(&state, &headers) {
        return unauthorized();
    }
    fleet_response(state.service.submit_task(request).await)
}

async fn reap(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(request): Json<FleetReapRequest>,
) -> Response {
    if !operator_authorized(&state, &headers) {
        return unauthorized();
    }
    fleet_response(state.service.reap(request).await)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutboxQuery {
    after: Option<String>,
    #[serde(default = "default_outbox_limit")]
    limit: u32,
}

const fn default_outbox_limit() -> u32 {
    100
}

async fn outbox(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Query(query): Query<OutboxQuery>,
) -> Response {
    if !operator_authorized(&state, &headers) {
        return unauthorized();
    }
    if query.limit == 0 || query.limit > 1_000 {
        return bad_request("outbox limit must be 1..=1000");
    }
    let after = query
        .after
        .as_deref()
        .map(agentd_core::types::FleetOutboxId::from_string);
    fleet_response(
        state
            .service
            .outbox_after(after.as_ref(), query.limit)
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerEnrollmentBody {
    worker_id: String,
    worker_incarnation_id: String,
    labels: serde_json::Value,
    daemon_version: String,
    host_name: String,
    network_zone: Option<String>,
    capabilities: serde_json::Value,
    certificate_chain_der_base64: Vec<String>,
}

async fn enroll_worker(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(body): Json<WorkerEnrollmentBody>,
) -> Response {
    if !operator_authorized(&state, &headers) {
        return unauthorized();
    }
    if !valid_id(&body.worker_id, "wk_")
        || !valid_id(&body.worker_incarnation_id, "wi_")
        || !bounded_text(&body.daemon_version, 256)
        || !bounded_text(&body.host_name, 256)
        || body
            .network_zone
            .as_deref()
            .is_some_and(|value| !bounded_text(value, 256))
    {
        return bad_request("invalid worker enrollment fields");
    }
    if !body.labels.is_object()
        || !body.capabilities.is_object()
        || serde_json::to_vec(&body.labels)
            .ok()
            .is_none_or(|value| value.len() > MAX_ENROLLMENT_METADATA_BYTES)
        || serde_json::to_vec(&body.capabilities)
            .ok()
            .is_none_or(|value| value.len() > MAX_ENROLLMENT_METADATA_BYTES)
    {
        return bad_request("worker enrollment metadata is invalid or exceeds 64 KiB");
    }
    if !valid_worker_attestation(&body.labels) {
        return bad_request("worker enrollment requires a valid agentd_attestation label");
    }
    let certificates = match decode_certificate_chain(&body.certificate_chain_der_base64) {
        Ok(certificates) => certificates,
        Err(error) => return bad_request(error),
    };
    let observed_at = match now_unix() {
        Ok(observed_at) => observed_at,
        Err(response) => return response,
    };
    let verified =
        match state
            .enrollment_identity
            .verify_enrollment_certificate(&WorkloadIdentityRequest {
                peer_certificates_der: certificates,
                observed_at,
            }) {
            Ok(verified) => verified,
            Err(error) => return security_response(error),
        };
    let worker_id = WorkerId::from_string(body.worker_id);
    let incarnation_id = WorkerIncarnationId::from_string(body.worker_incarnation_id);
    if verified.worker_incarnation_id != incarnation_id {
        return bad_request("certificate SPIFFE identity does not match worker incarnation");
    }
    let result = enroll_worker_workload_identity(
        state.store.pool(),
        WorkerWorkloadEnrollment {
            worker: WorkerCreate {
                id: worker_id.clone(),
                trust_domain: verified.trust_domain.clone(),
                labels: body.labels,
            },
            incarnation: WorkerRegistration {
                id: incarnation_id.clone(),
                daemon_version: body.daemon_version,
                host_name: body.host_name,
                network_zone: body.network_zone,
                capabilities: body.capabilities,
            },
            binding: WorkloadIdentityBindingCreate {
                certificate_sha256: verified.certificate_sha256,
                spiffe_uri: verified.spiffe_uri,
                role: WorkloadRole::Worker,
                trust_domain: verified.trust_domain,
                worker_id: Some(worker_id),
                worker_incarnation_id: Some(incarnation_id),
                not_before: verified.not_before,
                not_after: verified.not_after,
                created_at: observed_at,
            },
        },
    )
    .await;
    match result {
        Ok(record) => Json(json!({
            "worker_id": record.worker.id,
            "worker_status": record.worker.status,
            "worker_record_version": record.worker.record_version,
            "worker_incarnation_id": record.incarnation.id,
            "is_current": record.incarnation.is_current,
            "certificate_sha256": record.identity.binding.certificate_sha256,
            "certificate_not_after": record.identity.binding.not_after,
        }))
        .into_response(),
        Err(error) => security_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokeWorkerIdentityBody {
    certificate_sha256: String,
    reason: String,
}

async fn revoke_worker_identity(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(body): Json<RevokeWorkerIdentityBody>,
) -> Response {
    if !operator_authorized(&state, &headers) {
        return unauthorized();
    }
    if !valid_sha256(&body.certificate_sha256) || !bounded_text(&body.reason, 512) {
        return bad_request("invalid workload identity revocation");
    }
    let revoked_at = match now_unix() {
        Ok(revoked_at) => revoked_at,
        Err(response) => return response,
    };
    match revoke_workload_identity(
        state.store.pool(),
        &body.certificate_sha256,
        revoked_at,
        body.reason.trim(),
    )
    .await
    {
        Ok(record) => Json(json!({
            "certificate_sha256": record.binding.certificate_sha256,
            "worker_id": record.binding.worker_id,
            "worker_incarnation_id": record.binding.worker_incarnation_id,
            "revoked_at": record.revoked_at,
            "revocation_reason": record.revocation_reason,
        }))
        .into_response(),
        Err(error) => security_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HeartbeatBody {
    availability: agentd_core::ports::WorkerAvailability,
}

async fn heartbeat(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(body): Json<HeartbeatBody>,
) -> Response {
    let identity = match worker_identity(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    fleet_response(
        state
            .service
            .heartbeat(
                identity,
                FleetHeartbeatRequest {
                    workload: placeholder_workload(),
                    availability: body.availability,
                    observed_at: 0,
                },
            )
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PullBody {
    protocol_version: u32,
    heartbeat_max_age_seconds: u32,
    lease_expires_at: i64,
}

async fn pull(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(body): Json<PullBody>,
) -> Response {
    let identity = match worker_identity(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    fleet_response(
        state
            .service
            .pull(
                identity,
                FleetPullRequest {
                    workload: placeholder_workload(),
                    protocol_version: body.protocol_version,
                    observed_at: 0,
                    heartbeat_max_age_seconds: body.heartbeat_max_age_seconds,
                    lease_expires_at: body.lease_expires_at,
                },
            )
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RenewBody {
    claim: agentd_core::types::TaskLeaseClaim,
    snapshot_ref: agentd_core::types::ProjectExecutionSnapshotRef,
    pinned_revocation_epoch: u64,
    expires_at: i64,
}

async fn renew(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(body): Json<RenewBody>,
) -> Response {
    let identity = match worker_identity(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    fleet_response(
        state
            .service
            .renew(
                identity,
                FleetRenewRequest {
                    workload: placeholder_workload(),
                    claim: body.claim,
                    snapshot_ref: body.snapshot_ref,
                    pinned_revocation_epoch: body.pinned_revocation_epoch,
                    observed_at: 0,
                    expires_at: body.expires_at,
                },
            )
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompleteBody {
    claim: agentd_core::types::TaskLeaseClaim,
    idempotency_key: String,
    outcome_sha256: String,
}

async fn complete(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(body): Json<CompleteBody>,
) -> Response {
    let identity = match worker_identity(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    fleet_response(
        state
            .service
            .complete(
                identity,
                FleetCompletionReport {
                    workload: placeholder_workload(),
                    claim: body.claim,
                    idempotency_key: body.idempotency_key,
                    outcome_sha256: body.outcome_sha256,
                    observed_at: 0,
                },
            )
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FailBody {
    claim: agentd_core::types::TaskLeaseClaim,
    idempotency_key: String,
    failure_code: String,
    retryable: bool,
}

async fn fail(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(body): Json<FailBody>,
) -> Response {
    let identity = match worker_identity(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    fleet_response(
        state
            .service
            .fail(
                identity,
                FleetFailureReport {
                    workload: placeholder_workload(),
                    claim: body.claim,
                    idempotency_key: body.idempotency_key,
                    failure_code: body.failure_code,
                    retryable: body.retryable,
                    observed_at: 0,
                },
            )
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CancelBody {
    claim: agentd_core::types::TaskLeaseClaim,
    idempotency_key: String,
    reason_code: String,
}

async fn cancel(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(body): Json<CancelBody>,
) -> Response {
    let identity = match worker_identity(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    fleet_response(
        state
            .service
            .cancel(
                identity,
                FleetCancelRequest {
                    workload: placeholder_workload(),
                    claim: body.claim,
                    idempotency_key: body.idempotency_key,
                    reason_code: body.reason_code,
                    observed_at: 0,
                },
            )
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArtifactAckBody {
    claim: agentd_core::types::TaskLeaseClaim,
    upload_id: agentd_core::types::ArtifactUploadId,
    execution_artifact_id: agentd_core::types::ExecutionArtifactId,
    idempotency_key: String,
    artifact_sha256: String,
    upload_attempt: u32,
    part_count: u32,
}

async fn acknowledge_artifact(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(body): Json<ArtifactAckBody>,
) -> Response {
    let identity = match worker_identity(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    fleet_response(
        state
            .service
            .acknowledge_artifact_upload(
                identity,
                ArtifactUploadAckRequest {
                    workload: placeholder_workload(),
                    claim: body.claim,
                    upload_id: body.upload_id,
                    execution_artifact_id: body.execution_artifact_id,
                    idempotency_key: body.idempotency_key,
                    artifact_sha256: body.artifact_sha256,
                    upload_attempt: body.upload_attempt,
                    part_count: body.part_count,
                    observed_at: 0,
                },
            )
            .await,
    )
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SideEffectBody {
    claim: agentd_core::types::TaskLeaseClaim,
    checkpoint: agentd_core::types::SecurityCheckpoint,
    action: agentd_core::types::ProtectedAction,
    idempotency_key: String,
}

async fn admit_side_effect(
    State(state): State<FleetHttpState>,
    headers: HeaderMap,
    Json(body): Json<SideEffectBody>,
) -> Response {
    let identity = match worker_identity(&state, &headers) {
        Ok(identity) => identity,
        Err(response) => return response,
    };
    fleet_response(
        state
            .service
            .admit_side_effect(
                identity,
                FleetSideEffectRequest {
                    workload: placeholder_workload(),
                    claim: body.claim,
                    checkpoint: body.checkpoint,
                    action: body.action,
                    idempotency_key: body.idempotency_key,
                    observed_at: 0,
                },
            )
            .await,
    )
}

fn worker_identity(
    state: &FleetHttpState,
    headers: &HeaderMap,
) -> Result<WorkloadIdentityRequest, Response> {
    let provided = headers
        .get("x-agentd-workload-proxy-authorization")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or_default();
    if provided.len() != state.proxy_authorization.len()
        || !bool::from(
            provided
                .as_bytes()
                .ct_eq(state.proxy_authorization.as_bytes()),
        )
    {
        return Err(unauthorized());
    }
    let certificates = if let Some(encoded) = headers
        .get("x-agentd-peer-certificate-chain")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.len() <= MAX_CERTIFICATE_CHAIN_HEADER_BYTES)
    {
        let encoded = encoded.split(',').map(str::to_string).collect::<Vec<_>>();
        decode_certificate_chain(&encoded).map_err(|_| unauthorized())?
    } else {
        let xfcc = headers
            .get("x-forwarded-client-cert")
            .and_then(|value| value.to_str().ok())
            .ok_or_else(unauthorized)?;
        decode_xfcc_chain(xfcc).map_err(|_| unauthorized())?
    };
    Ok(WorkloadIdentityRequest {
        peer_certificates_der: certificates,
        observed_at: 0,
    })
}

fn decode_certificate_chain(encoded: &[String]) -> Result<Vec<Vec<u8>>, &'static str> {
    if encoded.is_empty() || encoded.len() > MAX_CERTIFICATES {
        return Err("invalid certificate chain");
    }
    let encoded_bytes = encoded
        .iter()
        .map(String::len)
        .try_fold(0_usize, usize::checked_add)
        .filter(|total| *total <= MAX_CERTIFICATE_CHAIN_HEADER_BYTES)
        .ok_or("certificate chain exceeds limit")?;
    if encoded_bytes == 0 {
        return Err("invalid certificate chain");
    }
    let certificates = encoded
        .iter()
        .map(|part| {
            STANDARD
                .decode(part.trim())
                .map_err(|_| "invalid certificate encoding")
        })
        .collect::<Result<Vec<_>, _>>()?;
    if certificates
        .iter()
        .any(|certificate| certificate.is_empty() || certificate.len() > MAX_CERTIFICATE_BYTES)
    {
        return Err("invalid certificate size");
    }
    Ok(certificates)
}

fn decode_xfcc_chain(value: &str) -> Result<Vec<Vec<u8>>, &'static str> {
    if value.is_empty()
        || value.len() > MAX_CERTIFICATE_CHAIN_HEADER_BYTES
        || value.as_bytes().contains(&b',')
    {
        return Err("invalid XFCC header");
    }
    let encoded = value
        .split(';')
        .filter_map(|field| field.trim().split_once('='))
        .find_map(|(name, value)| name.eq_ignore_ascii_case("Chain").then_some(value.trim()))
        .ok_or("XFCC chain is missing")?;
    let encoded = encoded
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .ok_or("XFCC chain must be quoted")?;
    let pem = percent_decode(encoded.as_bytes())?;
    let pem = std::str::from_utf8(&pem).map_err(|_| "XFCC chain is not UTF-8 PEM")?;
    decode_pem_certificates(pem)
}

fn percent_decode(value: &[u8]) -> Result<Vec<u8>, &'static str> {
    let mut decoded = Vec::with_capacity(value.len());
    let mut index = 0;
    while index < value.len() {
        if value[index] == b'%' {
            let high = value.get(index + 1).and_then(|byte| hex_nibble(*byte));
            let low = value.get(index + 2).and_then(|byte| hex_nibble(*byte));
            let (Some(high), Some(low)) = (high, low) else {
                return Err("invalid XFCC percent encoding");
            };
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            if !value[index].is_ascii_graphic() && value[index] != b' ' {
                return Err("invalid XFCC byte");
            }
            decoded.push(value[index]);
            index += 1;
        }
    }
    Ok(decoded)
}

const fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

fn decode_pem_certificates(mut pem: &str) -> Result<Vec<Vec<u8>>, &'static str> {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    let mut certificates = Vec::new();
    while !pem.trim().is_empty() {
        pem = pem.trim_start();
        let body = pem.strip_prefix(BEGIN).ok_or("invalid certificate PEM")?;
        let end = body.find(END).ok_or("unterminated certificate PEM")?;
        let encoded = body[..end]
            .bytes()
            .filter(|byte| !byte.is_ascii_whitespace())
            .collect::<Vec<_>>();
        let certificate = STANDARD
            .decode(encoded)
            .map_err(|_| "invalid certificate PEM base64")?;
        if certificate.is_empty() || certificate.len() > MAX_CERTIFICATE_BYTES {
            return Err("invalid certificate size");
        }
        certificates.push(certificate);
        if certificates.len() > MAX_CERTIFICATES {
            return Err("certificate chain is too long");
        }
        pem = &body[end + END.len()..];
    }
    if certificates.is_empty() {
        return Err("certificate chain is empty");
    }
    Ok(certificates)
}

fn valid_id(value: &str, prefix: &str) -> bool {
    value.len() == 29
        && value.starts_with(prefix)
        && value
            .as_bytes()
            .get(prefix.len())
            .is_some_and(|byte| matches!(byte, b'0'..=b'7'))
        && value[prefix.len()..].bytes().all(|byte| {
            matches!(
                byte,
                b'0'..=b'9'
                    | b'A'..=b'H'
                    | b'J'..=b'K'
                    | b'M'..=b'N'
                    | b'P'..=b'T'
                    | b'V'..=b'Z'
            )
        })
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_trust_domain(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn valid_worker_attestation(labels: &serde_json::Value) -> bool {
    let Some(attestation) = labels.get("agentd_attestation") else {
        return false;
    };
    let text = |field: &str| {
        attestation
            .get(field)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| bounded_text(value, 512))
    };
    let digest = |field: &str| {
        attestation
            .get(field)
            .and_then(serde_json::Value::as_str)
            .is_some_and(valid_sha256)
    };
    attestation.is_object()
        && attestation
            .get("rollout_id")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| valid_id(value, "ir_"))
        && attestation
            .get("image_digest")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value.strip_prefix("sha256:").is_some_and(valid_sha256))
        && digest("signature_bundle_sha256")
        && digest("signature_policy_sha256")
        && text("region")
        && text("zone")
        && text("resource_class")
}

fn bounded_text(value: &str, maximum_bytes: usize) -> bool {
    value == value.trim()
        && !value.is_empty()
        && value.len() <= maximum_bytes
        && !value.chars().any(char::is_control)
}

fn now_unix() -> Result<i64, Response> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| service_unavailable("system clock is before Unix epoch"))?
        .as_secs();
    i64::try_from(seconds).map_err(|_| service_unavailable("system clock exceeds i64 range"))
}

fn operator_authorized(state: &FleetHttpState, headers: &HeaderMap) -> bool {
    let Some(expected) = state
        .operator_auth
        .api_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return false;
    };
    let provided = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().strip_prefix("Bearer "))
        .map(str::trim)
        .unwrap_or_default();
    provided.len() == expected.len() && bool::from(provided.as_bytes().ct_eq(expected.as_bytes()))
}

fn placeholder_workload() -> AuthenticatedWorkload {
    AuthenticatedWorkload {
        spiffe_uri: "spiffe://untrusted.invalid/worker/placeholder".to_string(),
        role: WorkloadRole::Worker,
        trust_domain: "untrusted.invalid".to_string(),
        certificate_sha256: "0".repeat(64),
        not_before: 0,
        not_after: 0,
        worker_id: None,
        worker_incarnation_id: None,
    }
}

fn fleet_response<T: serde::Serialize>(result: Result<T, FleetServiceError>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            let status = match error.code {
                "worker_identity_denied" | "worker_identity_mismatch" => StatusCode::FORBIDDEN,
                "worker_identity_invalid" | "fleet_request_invalid" => StatusCode::BAD_REQUEST,
                "fleet_resource_not_found" => StatusCode::NOT_FOUND,
                "fleet_state_conflict" | "stale_fencing_token" | "task_terminal" => {
                    StatusCode::CONFLICT
                }
                code if code.ends_with("_unavailable") => StatusCode::SERVICE_UNAVAILABLE,
                _ => StatusCode::FORBIDDEN,
            };
            (status, Json(json!({ "error": error.code }))).into_response()
        }
    }
}

fn security_response(error: SecurityError) -> Response {
    let (status, code) = match error {
        SecurityError::Denied(_) => (StatusCode::FORBIDDEN, "workload_identity_denied"),
        SecurityError::Invalid(_) => (StatusCode::CONFLICT, "workload_identity_conflict"),
        SecurityError::Unavailable(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "workload_identity_unavailable",
        ),
    };
    (status, Json(json!({ "error": code }))).into_response()
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "authenticated workload identity required" })),
    )
        .into_response()
}

fn bad_request(message: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
}

fn service_unavailable(message: &str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({ "error": message })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::{STANDARD, decode_xfcc_chain};
    use base64::Engine as _;

    #[test]
    fn decodes_envoy_sanitize_set_xfcc_certificate_chain() {
        let first = STANDARD.encode([1_u8, 2, 3]);
        let second = STANDARD.encode([4_u8, 5, 6]);
        let pem = format!(
            "-----BEGIN CERTIFICATE-----\n{first}\n-----END CERTIFICATE-----\n\
             -----BEGIN CERTIFICATE-----\n{second}\n-----END CERTIFICATE-----\n"
        );
        let encoded = pem
            .bytes()
            .map(|byte| match byte {
                b'\n' => "%0A".to_string(),
                b'+' => "%2B".to_string(),
                b'=' => "%3D".to_string(),
                _ => char::from(byte).to_string(),
            })
            .collect::<String>();
        let header = format!("By=spiffe://proxy;Hash=00;Chain=\"{encoded}\"");

        assert_eq!(
            decode_xfcc_chain(&header).expect("XFCC chain"),
            vec![vec![1, 2, 3], vec![4, 5, 6]]
        );
    }

    #[test]
    fn rejects_forwarded_xfcc_hops() {
        assert!(decode_xfcc_chain("By=first;Chain=\"bad\",By=second;Chain=\"bad\"").is_err());
    }
}
